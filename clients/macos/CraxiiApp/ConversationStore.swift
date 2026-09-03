import Foundation
import Observation
import AppKit
import CraxiiProtocol
import CraxiiClientCore
import CraxiiPresentation
import CraxiiAppleAdapters

@MainActor
@Observable
final class ConversationStore {
    var endpoint: String
    var credentialInput = ""
    var composerText = ""
    private(set) var snapshot: ClientSnapshot
    private(set) var localActionError: ClientError?
    private(set) var isPreparingMessage = false
    private(set) var cancellingWorkIDs: Set<WorkID> = []
    private(set) var composerFocusRevision: UInt64 = 0
    private(set) var transcriptScrollRequest: UInt64 = 0

    private var session: (any ConversationSession)?
    private let injectedSession: (any ConversationSession)?
    private var launched = false
    private var lastAppliedPresentationRevision: UInt64
    private var sleepObserver: NSObjectProtocol?
    private var wakeObserver: NSObjectProtocol?

    init(
        session: (any ConversationSession)? = nil,
        endpoint: String = ConversationStore.defaultEndpoint,
        initialSnapshot: ClientSnapshot? = nil
    ) {
        self.endpoint = endpoint
        injectedSession = session
        self.session = session
        let initial = initialSnapshot ?? ClientSnapshot(
            credentialStatus: .required, connectionState: .disconnected,
            projection: CanonicalProjection(), drafts: [], pendingCommandCount: 0,
            generation: 0, lastError: nil)
        snapshot = initial
        lastAppliedPresentationRevision = initial.presentationRevision
    }

    var presentation: ConversationPresentation {
        ConversationPresenter.present(
            snapshot: snapshot,
            endpointConfigured: !endpoint.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
            localError: localActionError)
    }

    var composerValidation: ComposerValidation { ComposerPolicy.validate(composerText) }

    var canSend: Bool {
        ComposerPolicy.canSend(
            composerText, mutationsAllowed: presentation.mutationsAllowed,
            isPreparing: isPreparingMessage)
    }

    static var defaultEndpoint: String {
        #if DEBUG
        "http://127.0.0.1:8080/"
        #else
        ""
        #endif
    }

    func launch() async {
        guard !launched else { return }
        launched = true
        let smokeMode = ProcessInfo.processInfo.environment["CRAXII_STAGE22_UI_SMOKE"]
        if smokeMode == "setup"
            || ProcessInfo.processInfo.environment["CRAXII_STAGE21_UI_SMOKE"] == "1" {
            return
        }
        if smokeMode == "conversation" {
            configureConversationSmokeState()
            return
        }
        do {
            let session: any ConversationSession
            if let injectedSession {
                session = injectedSession
            } else {
                let stateStore = try AtomicFileStateStore()
                let persisted = try? await stateStore.load()
                let generator = UUIDv7Generator()
                let profile: BackendProfile
                if let stored = persisted?.profile {
                    profile = stored
                    endpoint = stored.endpoint
                } else {
                    profile = BackendProfile(
                        profileID: try await generator.next(), endpoint: Self.defaultEndpoint)
                }
                session = ClientSession(
                    profile: profile,
                    allowDebugLocalhostHTTP: Self.debugLocalhostAllowed,
                    credentialStore: KeychainDeviceCredentialStore(),
                    localStore: stateStore,
                    http: URLSessionHTTPExecutor(),
                    streams: URLSessionEventStreamOpener(),
                    identifiers: generator)
            }
            await session.setSnapshotHandler { [weak self] snapshot in
                Task { @MainActor [weak self] in self?.apply(snapshot) }
            }
            self.session = session
            installLifecycleObservers()
            await session.start()
        } catch let error as ClientError {
            localActionError = error
        } catch {
            localActionError = .cacheCorrupt
        }
    }

    func sendComposer() async {
        guard canSend, let session else { return }
        let acceptedText = composerText
        isPreparingMessage = true
        localActionError = nil
        do {
            let prepared = try await session.prepareMessage(text: acceptedText)
            composerText = ""
            isPreparingMessage = false
            composerFocusRevision &+= 1
            transcriptScrollRequest &+= 1
            Task { @MainActor [weak self] in
                do {
                    _ = try await session.sendPreparedMessage(prepared)
                } catch let error as ClientError {
                    self?.localActionError = error
                } catch {
                    self?.localActionError = .serverUnavailable
                }
            }
        } catch let error as ClientError {
            isPreparingMessage = false
            localActionError = error
            composerFocusRevision &+= 1
        } catch {
            isPreparingMessage = false
            localActionError = .cacheCorrupt
            composerFocusRevision &+= 1
        }
    }

    func cancel(workID: WorkID) async {
        guard !cancellingWorkIDs.contains(workID), let session else { return }
        cancellingWorkIDs.insert(workID)
        localActionError = nil
        do {
            let prepared = try await session.prepareCancellation(workID: workID)
            cancellingWorkIDs.remove(workID)
            Task { @MainActor [weak self] in
                do {
                    _ = try await session.sendPreparedCancellation(prepared)
                } catch let error as ClientError {
                    self?.localActionError = error
                } catch {
                    self?.localActionError = .cancellationTransportFailure
                }
            }
        } catch let error as ClientError {
            cancellingWorkIDs.remove(workID)
            localActionError = error
        } catch {
            cancellingWorkIDs.remove(workID)
            localActionError = .cacheCorrupt
        }
    }

    func installCredential() async {
        guard let session else { return }
        do {
            try await session.installCredential(credentialInput)
            credentialInput = ""
            localActionError = nil
        } catch let error as ClientError { localActionError = error }
        catch { localActionError = .keychainFailure(-1) }
    }

    func deleteCredential() async {
        guard let session else { return }
        do { try await session.deleteCredential(); localActionError = nil }
        catch let error as ClientError { localActionError = error }
        catch { localActionError = .keychainFailure(-1) }
    }

    func applyEndpoint() async {
        guard let session else { return }
        do { try await session.changeEndpoint(to: endpoint); localActionError = nil }
        catch let error as ClientError { localActionError = error }
        catch { localActionError = .configurationMismatch }
    }

    func connect() async { await session?.retryConnection() }

    func reset() async {
        do { try await session?.resetDisposableState(); localActionError = nil }
        catch let error as ClientError { localActionError = error }
        catch { localActionError = .cacheCorrupt }
    }

    func dismissError() { localActionError = nil }

    func shutdown() async {
        removeLifecycleObservers()
        await session?.shutdown()
    }

    private func apply(_ candidate: ClientSnapshot) {
        guard candidate.presentationRevision > lastAppliedPresentationRevision else { return }
        let wasUsable = snapshot.projection.primaryConversation != nil
        snapshot = candidate
        lastAppliedPresentationRevision = candidate.presentationRevision
        if !wasUsable, candidate.projection.primaryConversation != nil,
           candidate.projection.messages.isEmpty {
            composerFocusRevision &+= 1
        }
    }

    private func installLifecycleObservers() {
        guard sleepObserver == nil, wakeObserver == nil else { return }
        let center = NSWorkspace.shared.notificationCenter
        sleepObserver = center.addObserver(
            forName: NSWorkspace.willSleepNotification, object: nil, queue: .main
        ) { [weak self] _ in
            Task { @MainActor [weak self] in await self?.session?.suspendTransport() }
        }
        wakeObserver = center.addObserver(
            forName: NSWorkspace.didWakeNotification, object: nil, queue: .main
        ) { [weak self] _ in
            Task { @MainActor [weak self] in await self?.session?.resumeTransport() }
        }
    }

    private func removeLifecycleObservers() {
        let center = NSWorkspace.shared.notificationCenter
        if let sleepObserver { center.removeObserver(sleepObserver) }
        if let wakeObserver { center.removeObserver(wakeObserver) }
        sleepObserver = nil
        wakeObserver = nil
    }

    private func configureConversationSmokeState() {
        let data = Data("""
        {
          "protocol_version": 1,
          "snapshot_cursor": 0,
          "craxii": {
            "craxii_id": "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c01",
            "display_name": "Craxii",
            "owner_label": "Owner"
          },
          "primary_conversation": {
            "conversation_id": "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c02",
            "kind": "primary",
            "lifecycle": "active",
            "created_at": "2026-09-03T00:00:00Z"
          },
          "messages": [
            {
              "message_id": "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c03",
              "conversation_id": "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c02",
              "conversation_sequence": 1,
              "role": "user",
              "content": [{"type": "text", "text": "Stage 22 smoke message"}],
              "client_message_id": "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c04",
              "work_id": null,
              "committed_at": "2026-09-03T00:00:00Z"
            }
          ],
          "work_items": [
            {
              "work_id": "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c05",
              "conversation_id": "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c02",
              "conversation_work_ordinal": 1,
              "state": "queued",
              "trigger_message_id": "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c03",
              "created_at": "2026-09-03T00:00:00Z",
              "queued_at": "2026-09-03T00:00:00Z",
              "started_at": null,
              "cancel_requested_at": null,
              "terminal_at": null,
              "terminal_reason": null,
              "cleanup_pending": false,
              "tool_summaries": []
            }
          ],
          "unresolved_outcomes": []
        }
        """.utf8)
        guard let bootstrap = try? JSONDecoder().decode(BootstrapResponse.self, from: data),
              let projection = try? CanonicalProjection.bootstrap(bootstrap) else { return }
        snapshot = ClientSnapshot(
            credentialStatus: .installed, connectionState: .live,
            projection: projection, drafts: [], pendingCommandCount: 0,
            generation: 1, presentationRevision: 1, lastError: nil)
        lastAppliedPresentationRevision = 1
        composerFocusRevision &+= 1
    }

    private static var debugLocalhostAllowed: Bool {
        #if DEBUG
        true
        #else
        false
        #endif
    }
}
