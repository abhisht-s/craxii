import Foundation
import Observation
import AppKit
import CraxiiProtocol
import CraxiiClientCore
import CraxiiAppleAdapters

@MainActor
@Observable
final class ConversationStore {
    var endpoint: String
    var credentialInput = ""
    var snapshot: ClientSnapshot
    var localActionError: ClientError?

    private var session: ClientSession?
    private var launched = false
    private var sleepObserver: NSObjectProtocol?
    private var wakeObserver: NSObjectProtocol?

    init() {
        endpoint = ConversationStore.defaultEndpoint
        snapshot = ClientSnapshot(
            credentialStatus: .required, connectionState: .disconnected,
            projection: CanonicalProjection(), drafts: [], pendingCommandCount: 0,
            generation: 0, lastError: nil)
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
        if ProcessInfo.processInfo.environment["CRAXII_STAGE21_UI_SMOKE"] == "1" {
            return
        }
        do {
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
            let session = ClientSession(
                profile: profile,
                allowDebugLocalhostHTTP: Self.debugLocalhostAllowed,
                credentialStore: KeychainDeviceCredentialStore(),
                localStore: stateStore,
                http: URLSessionHTTPExecutor(),
                streams: URLSessionEventStreamOpener(),
                identifiers: generator)
            await session.setSnapshotHandler { [weak self] snapshot in
                Task { @MainActor [weak self] in self?.snapshot = snapshot }
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

    func shutdown() async {
        removeLifecycleObservers()
        await session?.shutdown()
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

    private static var debugLocalhostAllowed: Bool {
        #if DEBUG
        true
        #else
        false
        #endif
    }
}
