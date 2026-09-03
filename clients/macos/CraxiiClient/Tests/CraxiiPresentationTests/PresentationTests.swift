import Foundation
import Testing
@testable import CraxiiProtocol
@testable import CraxiiClientCore
@testable import CraxiiPresentation

private func id(_ suffix: String) -> ProtocolID {
    ProtocolID(rawValue: "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c\(suffix)")!
}

private func projection(
    messages: [ProjectedMessage] = [], works: [WorkProjection] = [],
    unresolved: [UnresolvedOutcomeDTO] = []
) -> CanonicalProjection {
    CanonicalProjection(
        craxii: CraxiiProjectionDTO(
            craxiiID: id("01"), displayName: "Craxii", ownerLabel: "Owner"),
        primaryConversation: ConversationDTO(
            conversationID: id("02"), kind: "primary", lifecycle: .active,
            createdAt: "2026-09-03T00:00:00Z"),
        messages: messages, works: works, unresolvedOutcomes: unresolved,
        lastAppliedCursor: Cursor(rawValue: 10)!)
}

private func snapshot(
    state: ClientConnectionState = .live,
    credential: CredentialStatus = .installed,
    projection value: CanonicalProjection? = projection(),
    drafts: [DraftProjection] = [], pending: [PendingCommandProjection] = [],
    error: ClientError? = nil, backend: SafeBackendError? = nil,
    revision: UInt64 = 1
) -> ClientSnapshot {
    ClientSnapshot(
        credentialStatus: credential, connectionState: state,
        projection: value ?? CanonicalProjection(), drafts: drafts,
        pendingCommandCount: pending.count, pendingCommands: pending,
        generation: 1, presentationRevision: revision, lastError: error,
        lastBackendError: backend)
}

private func work(
    state: WorkState, reason: WorkTerminalReason? = nil,
    tools: [SafeToolProjection] = []
) -> WorkProjection {
    WorkProjection(
        workID: id("05"), conversationID: id("02"), conversationWorkOrdinal: 1,
        state: state, triggerMessageID: id("03"), createdAt: nil, queuedAt: nil,
        startedAt: nil, cancelRequestedAt: nil, terminalAt: nil,
        terminalReason: reason, cleanupPending: false, tools: tools)
}

@Test func emptyConnectedConversationIsUsableAndQuiet() {
    let value = ConversationPresenter.present(snapshot: snapshot())
    #expect(value.gate == .usable)
    #expect(value.mutationsAllowed)
    #expect(value.transcript.isEmpty)
    #expect(value.banner == nil)
    #expect(value.craxiiName == "Craxii")
}

@Test func setupConnectingReconnectOfflineAuthenticationAndFatalStatesStayDistinct() {
    #expect(ConversationPresenter.present(snapshot: snapshot(credential: .required)).gate == .setup)
    #expect(ConversationPresenter.present(
        snapshot: snapshot(state: .bootstrapping, projection: nil)).gate == .connecting)
    #expect(ConversationPresenter.present(
        snapshot: snapshot(state: .reconnecting)).banner?.kind == .reconnecting)
    let offline = ConversationPresenter.present(snapshot: snapshot(state: .disconnected))
    #expect(offline.banner?.kind == .offline)
    #expect(offline.mutationsAllowed)
    #expect(ConversationPresenter.present(
        snapshot: snapshot(state: .authenticationFailed)).gate == .credentialRepair)
    #expect(ConversationPresenter.present(
        snapshot: snapshot(state: .fatalProtocolError, error: .incompatibleProtocol)).gate == .fatalProtocol)
    #expect(ConversationPresenter.present(
        snapshot: snapshot(state: .fatalProtocolError, error: .configurationMismatch)).gate == .configurationMismatch)
}

@Test func canonicalTranscriptUsesServerIdentityOrderAndSuppressesMatchingOptimisticRow() {
    let clientID = id("04")
    let message = ProjectedMessage(
        messageID: id("03"), conversationID: id("02"), canonicalOrder: 1,
        role: .user, content: [ContentBlock(text: "exact\ntext")],
        clientMessageID: clientID, workID: nil, committedAt: "now")
    let matching = PendingCommandProjection(
        commandID: clientID, kind: .message, conversationID: id("02"), workID: nil,
        clientMessageID: clientID, visibleMessageText: "exact\ntext",
        deliveryState: .deliveryNotConfirmed)
    let otherID = id("06")
    let other = PendingCommandProjection(
        commandID: otherID, kind: .message, conversationID: id("02"), workID: nil,
        clientMessageID: otherID, visibleMessageText: "again",
        deliveryState: .waitingForConnection)
    let rows = ConversationPresenter.present(snapshot: snapshot(
        projection: projection(messages: [message]), pending: [matching, other])).transcript
    #expect(rows.map(\.id) == [id("03").rawValue, "local-\(otherID.rawValue)"])
    #expect(rows[0].isCanonical)
    #expect(rows[0].text == "exact\ntext")
    #expect(rows[1].status == "Waiting for connection")
    #expect(!rows[1].isCanonical)
}

@Test func draftIsOneStablePresentationOnlyRowWithSeparateRefusal() {
    let draft = DraftProjection(
        conversationID: id("02"), workID: id("05"), invocationID: id("07"),
        draftID: id("08"), greatestSequence: 3, text: "partial", refusal: "declined")
    let row = ConversationPresenter.present(snapshot: snapshot(drafts: [draft])).transcript[0]
    #expect(row.id == id("08").rawValue)
    #expect(row.kind == .draft)
    #expect(row.text == "partial")
    #expect(row.refusal == "declined")
    #expect(!row.isCanonical)
}

@Test func workCancellationAndUnknownOutcomeWordingNeverOverclaim() {
    let cancellation = PendingCommandProjection(
        commandID: id("09"), kind: .cancellation, conversationID: id("02"),
        workID: id("05"), clientMessageID: nil, visibleMessageText: nil,
        deliveryState: .deliveryNotConfirmed)
    let active = ConversationPresenter.present(snapshot: snapshot(
        projection: projection(works: [work(state: .waitingOnModel)]),
        pending: [cancellation])).works[0]
    #expect(!active.canCancel)
    #expect(active.title == "Waiting on model")
    #expect(active.cancellationStatus == "Cancellation delivery is not confirmed")

    let completed = ConversationPresenter.present(snapshot: snapshot(
        projection: projection(works: [work(state: .completed, reason: .answered)]),
        pending: [cancellation])).works[0]
    #expect(completed.title == "Completed")
    #expect(completed.cancellationStatus == nil)

    let unknownWork = work(state: .interrupted, reason: .toolOutcomeUnknown)
    let unknown = ConversationPresenter.present(snapshot: snapshot(projection: projection(
        works: [unknownWork], unresolved: [UnresolvedOutcomeDTO(
            kind: .toolOutcomeUnknown, workID: id("05"), toolExecutionID: nil)]))).works[0]
    #expect(unknown.title == "Outcome unknown")
    #expect(unknown.tone == .warning)
}

@Test func liveToolFallbackAndBootstrapToolNameAreHonest() {
    let unnamed = SafeToolProjection(
        executionID: id("10"), toolName: nil, status: "dispatching", resultClass: nil,
        requestedAt: nil, startedAt: "now", finishedAt: nil, outcomeUnknown: false)
    let named = SafeToolProjection(
        executionID: id("11"), toolName: "read_file", status: "completed",
        resultClass: "future_result", requestedAt: nil, startedAt: "now",
        finishedAt: "later", outcomeUnknown: false)
    let presentation = ConversationPresenter.present(snapshot: snapshot(projection: projection(
        works: [work(state: .waitingOnTool, tools: [unnamed, named])]))).works[0]
    #expect(presentation.title == "Using a tool")
    #expect(presentation.tools[0].title == "Using a tool")
    #expect(presentation.tools[1].title == "Used read_file")
    #expect(presentation.tools[1].detail == nil)
}

@Test func lifecycleLimitIsGenericAndSafeBackendDetailsArePreserved() {
    let limited = ConversationPresenter.present(snapshot: snapshot(projection: projection(
        works: [work(state: .failed, reason: .lifecycleLimit)]))).works[0]
    #expect(limited.title == "Craxii reached a work limit")
    #expect(limited.detail == nil)

    let backend = SafeBackendError(
        code: "safe_code", message: "Safe public message", retryable: true,
        requestID: id("12"))
    let error = ConversationPresenter.present(snapshot: snapshot(backend: backend)).error
    #expect(error?.message == "Safe public message")
    #expect(error?.code == "safe_code")
    #expect(error?.requestID == id("12").rawValue)
    #expect(error?.retryable == true)
}

@Test func composerValidationUsesTrimOnlyForEligibilityAndCountsUTF8Bytes() {
    #expect(ComposerPolicy.validate(" \n\t") == .whitespaceOnly)
    #expect(ComposerPolicy.validate("  keep me  ").isValid)
    #expect(ComposerPolicy.validate(String(repeating: "é", count: 32_768)).isValid)
    #expect(ComposerPolicy.validate(String(repeating: "é", count: 32_769))
        == .overLimit(byteCount: 65_538, limit: 65_536))
    #expect(!ComposerPolicy.canSend("hello", mutationsAllowed: true, isPreparing: true))
    #expect(ComposerPolicy.canSend("hello", mutationsAllowed: true, isPreparing: false))
    #expect(ComposerPolicy.returnAction(commandModifier: false) == .insertNewline)
    #expect(ComposerPolicy.returnAction(commandModifier: true) == .send)
}

@Test func scrollPolicyProtectsOlderReadingAndUsesEightyPointThreshold() {
    #expect(TranscriptScrollPolicy.isNearBottom(distance: 80))
    #expect(!TranscriptScrollPolicy.isNearBottom(distance: 80.1))
    #expect(TranscriptScrollPolicy.action(
        initialLoad: true, userSubmitted: false, activityChanged: false,
        isNearBottom: false) == .scrollToBottom)
    #expect(TranscriptScrollPolicy.action(
        initialLoad: false, userSubmitted: false, activityChanged: true,
        isNearBottom: false) == .recordUnseenActivity)
    #expect(TranscriptScrollPolicy.action(
        initialLoad: false, userSubmitted: false, activityChanged: true,
        isNearBottom: true) == .scrollToBottom)
}
