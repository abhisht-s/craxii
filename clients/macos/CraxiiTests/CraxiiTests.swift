import Foundation
import XCTest
import CraxiiProtocol
import CraxiiClientCore
@testable import Craxii

private let preparedID = ProtocolID(
    rawValue: "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c20")!

private func appSnapshot(
    revision: UInt64 = 1, state: ClientConnectionState = .live
) throws -> ClientSnapshot {
    let data = Data("""
    {
      "protocol_version": 1,
      "snapshot_cursor": 4,
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
      "messages": [],
      "work_items": [],
      "unresolved_outcomes": []
    }
    """.utf8)
    let bootstrap = try JSONDecoder().decode(BootstrapResponse.self, from: data)
    return ClientSnapshot(
        credentialStatus: .installed, connectionState: state,
        projection: try CanonicalProjection.bootstrap(bootstrap), drafts: [],
        pendingCommandCount: 0, generation: 1,
        presentationRevision: revision, lastError: nil)
}

private actor FakeConversationSession: ConversationSession {
    private var handler: (@Sendable (ClientSnapshot) -> Void)?
    private let initial: ClientSnapshot
    private var preparationStarted = false
    private var preparationContinuation: CheckedContinuation<PreparedMessageCommand, Error>?
    private var shouldSuspendPreparation = false
    private var preparationError: ClientError?
    private(set) var preparedTexts: [String] = []
    private(set) var sentIDs: [ClientMessageID] = []

    init(initial: ClientSnapshot) { self.initial = initial }

    func setSnapshotHandler(_ handler: (@Sendable (ClientSnapshot) -> Void)?) {
        self.handler = handler
    }
    func currentSnapshot() -> ClientSnapshot { initial }
    func start() { handler?(initial) }
    func retryConnection() {}
    func installCredential(_ text: String) throws {}
    func deleteCredential() throws {}
    func resetDisposableState() throws {}
    func changeEndpoint(to input: String) throws {}

    func prepareMessage(text: String) async throws -> PreparedMessageCommand {
        preparedTexts.append(text)
        preparationStarted = true
        if let preparationError { throw preparationError }
        if shouldSuspendPreparation {
            return try await withCheckedThrowingContinuation {
                preparationContinuation = $0
            }
        }
        return PreparedMessageCommand(clientMessageID: preparedID)
    }

    func sendPreparedMessage(_ prepared: PreparedMessageCommand) throws -> MessageReceipt {
        sentIDs.append(prepared.clientMessageID)
        throw ClientError.serverUnavailable
    }

    func prepareCancellation(workID: WorkID) throws -> PreparedCancellationCommand {
        PreparedCancellationCommand(clientCommandID: preparedID, workID: workID)
    }
    func sendPreparedCancellation(
        _ prepared: PreparedCancellationCommand
    ) throws -> CancellationReceipt {
        throw ClientError.cancellationTransportFailure
    }
    func suspendTransport() {}
    func resumeTransport() {}
    func shutdown() {}

    func configureSuspendedPreparation() { shouldSuspendPreparation = true }
    func configurePreparationError(_ error: ClientError) { preparationError = error }
    func hasStartedPreparation() -> Bool { preparationStarted }
    func capturedPreparedTexts() -> [String] { preparedTexts }
    func capturedSentIDs() -> [ClientMessageID] { sentIDs }
    func releasePreparation() {
        preparationContinuation?.resume(returning: PreparedMessageCommand(
            clientMessageID: preparedID))
        preparationContinuation = nil
    }
    func emit(_ snapshot: ClientSnapshot) { handler?(snapshot) }
}

@MainActor
final class CraxiiTests: XCTestCase {
    func testMessageTextClearsOnlyAfterPreparationPersistenceCompletes() async throws {
        let fake = FakeConversationSession(initial: try appSnapshot())
        await fake.configureSuspendedPreparation()
        let store = ConversationStore(
            session: fake, endpoint: "http://127.0.0.1:8080/",
            initialSnapshot: try appSnapshot(revision: 0))
        await store.launch()
        await Task.yield()
        store.composerText = "  exact\nbytes  "
        XCTAssertTrue(store.canSend, "gate=\(store.presentation.gate) state=\(store.snapshot.connectionState) endpoint=\(store.endpoint)")

        let send = Task { @MainActor in await store.sendComposer() }
        for _ in 0..<100 where !(await fake.hasStartedPreparation()) { await Task.yield() }
        XCTAssertEqual(store.composerText, "  exact\nbytes  ")
        XCTAssertTrue(store.isPreparingMessage)

        await fake.releasePreparation()
        await send.value
        XCTAssertEqual(store.composerText, "")
        XCTAssertFalse(store.isPreparingMessage)
        let preparedTexts = await fake.capturedPreparedTexts()
        XCTAssertEqual(preparedTexts, ["  exact\nbytes  "])
        for _ in 0..<100 where (await fake.capturedSentIDs()).isEmpty { await Task.yield() }
        let sentIDs = await fake.capturedSentIDs()
        XCTAssertEqual(sentIDs, [preparedID])
    }

    func testPreparationFailurePreservesComposerAndFocusesIt() async throws {
        let fake = FakeConversationSession(initial: try appSnapshot())
        await fake.configurePreparationError(.cacheCorrupt)
        let store = ConversationStore(
            session: fake, endpoint: "http://127.0.0.1:8080/",
            initialSnapshot: try appSnapshot(revision: 0))
        await store.launch()
        await Task.yield()
        store.composerText = "keep this"
        XCTAssertTrue(store.canSend, "gate=\(store.presentation.gate) state=\(store.snapshot.connectionState) endpoint=\(store.endpoint)")
        let focusBefore = store.composerFocusRevision

        await store.sendComposer()

        XCTAssertEqual(store.composerText, "keep this")
        XCTAssertEqual(store.localActionError, .cacheCorrupt)
        XCTAssertGreaterThan(store.composerFocusRevision, focusBefore)
    }

    func testWhitespaceAndOverLimitNeverReachSession() async throws {
        let fake = FakeConversationSession(initial: try appSnapshot())
        let store = ConversationStore(
            session: fake, endpoint: "http://127.0.0.1:8080/",
            initialSnapshot: try appSnapshot(revision: 0))
        await store.launch()
        await Task.yield()
        store.composerText = " \n "
        XCTAssertFalse(store.canSend)
        await store.sendComposer()
        store.composerText = String(repeating: "é", count: 32_769)
        XCTAssertFalse(store.canSend)
        await store.sendComposer()
        let preparedTexts = await fake.capturedPreparedTexts()
        XCTAssertTrue(preparedTexts.isEmpty)
    }

    func testOlderSnapshotRevisionCannotReplaceNewerMainActorState() async throws {
        let fake = FakeConversationSession(initial: try appSnapshot(revision: 1))
        let store = ConversationStore(
            session: fake, endpoint: "http://127.0.0.1:8080/",
            initialSnapshot: try appSnapshot(revision: 0))
        await store.launch()
        await Task.yield()
        await fake.emit(try appSnapshot(revision: 3, state: .reconnecting))
        await fake.emit(try appSnapshot(revision: 2, state: .live))
        for _ in 0..<100 where store.snapshot.presentationRevision != 3 { await Task.yield() }
        XCTAssertEqual(store.snapshot.presentationRevision, 3)
        XCTAssertEqual(store.snapshot.connectionState, .reconnecting)
    }
}
