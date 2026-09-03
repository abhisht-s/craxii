import Foundation
import Testing
@testable import CraxiiProtocol
@testable import CraxiiClientCore

private let repositoryRoot: URL = {
    var url = URL(fileURLWithPath: #filePath)
    for _ in 0..<6 { url.deleteLastPathComponent() }
    return url
}()
private let fixtures = repositoryRoot.appendingPathComponent("backend/tests/fixtures/protocol-v1")
private func fixture(_ name: String) throws -> Data { try Data(contentsOf: fixtures.appendingPathComponent(name)) }
private func bootstrap() throws -> BootstrapResponse {
    try JSONDecoder().decode(BootstrapResponse.self, from: fixture("bootstrap-snapshot.json"))
}
private func durableEvents() throws -> [DurableEventEnvelope] {
    try JSONDecoder().decode([DurableEventEnvelope].self, from: fixture("durable-events.json"))
}
private func draftEvents() throws -> [EphemeralDraftEnvelope] {
    try JSONDecoder().decode([EphemeralDraftEnvelope].self, from: fixture("ephemeral-drafts.json"))
}
private let profileID = ProtocolID(rawValue: "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c30")!
private let profile = BackendProfile(profileID: profileID, endpoint: "http://127.0.0.1:8080/")
private let token = try! BearerToken(validating: String(repeating: "a", count: 64))

private struct FixedClock: UUIDv7Clock { let value: UInt64; func millisecondsSince1970() -> UInt64 { value } }
private struct FixedRandom: UUIDv7RandomSource {
    let value: [UInt8]
    func bytes(count: Int) throws -> [UInt8] { Array(value.prefix(count)) }
}

@Test func deterministicUUIDv7GenerationMatchesCrossLanguageLayout() async throws {
    let generator = UUIDv7Generator(
        clock: FixedClock(value: 0x01890f6c7b3a),
        random: FixedRandom(value: [0x0c, 0xc0, 0x18, 0xf1, 0x2e, 0x6f, 0x7a, 0x8b, 0x9c, 0x0d]))
    #expect(try await generator.next().rawValue == "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d")
}

@Test func endpointAndProfileRulesRejectAuthorityConfusion() throws {
    #expect(try EndpointPolicy.validate("http://127.0.0.1:8080/", allowDebugLocalhostHTTP: true).host == "127.0.0.1")
    #expect(try EndpointPolicy.validate("https://example.com/", allowDebugLocalhostHTTP: false).scheme == "https")
    for invalid in [
        "http://example.com/", "https://user@example.com/", "https://example.com/base",
        "https://example.com/?x=1", "https://example.com/#x", "https://example.com/%2e%2e/",
    ] {
        #expect(throws: ClientError.configurationMismatch) {
            _ = try EndpointPolicy.validate(invalid, allowDebugLocalhostHTTP: true)
        }
    }
}

@Test func bootstrapAtomicallyCreatesOrderedCanonicalProjection() throws {
    let projection = try CanonicalProjection.bootstrap(bootstrap())
    #expect(projection.messages.map(\.canonicalOrder) == [1])
    #expect(projection.works.map(\.conversationWorkOrdinal) == [1])
    #expect(projection.lastAppliedCursor.rawValue == 4)
}

@Test func durableApplyAdvancesOnlyAfterSuccessAllowsFilteredJumpAndDeduplicatesExactly() throws {
    let initial = try CanonicalProjection.bootstrap(bootstrap())
    let started = try #require(durableEvents().last)
    let applied = try DurableReducer().applying(started, to: initial)
    #expect(applied.lastAppliedCursor.rawValue == 8)
    #expect(applied.works[0].state == .running)
    #expect(try DurableReducer().applying(started, to: applied) == applied)

    var object = try #require(JSONSerialization.jsonObject(with: JSONEncoder().encode(started)) as? [String: Any])
    object["payload"] = ["state": "waiting_on_model", "transitioned_at": "2026-08-28T00:00:02.000000Z"]
    let conflict = try JSONDecoder().decode(
        DurableEventEnvelope.self, from: JSONSerialization.data(withJSONObject: object))
    #expect(throws: ClientError.projectionInvariant) {
        _ = try DurableReducer().applying(conflict, to: applied)
    }
    #expect(applied.lastAppliedCursor.rawValue == 8)
}

@Test func unknownDurableEventDoesNotAdvanceCursor() throws {
    let initial = try CanonicalProjection.bootstrap(bootstrap())
    let last = try #require(durableEvents().last)
    var object = try #require(
        JSONSerialization.jsonObject(with: JSONEncoder().encode(last)) as? [String: Any])
    object["cursor"] = 9
    object["event_id"] = "01890f3e-7b2c-7cc1-8c23-5b8f7b3aa099"
    object["event_type"] = "future.required_event"
    let unknown = try JSONDecoder().decode(
        DurableEventEnvelope.self, from: JSONSerialization.data(withJSONObject: object))
    #expect(throws: ProtocolModelError.unknownEventType) {
        _ = try DurableReducer().applying(unknown, to: initial)
    }
    #expect(initial.lastAppliedCursor.rawValue == 4)
}

@Test func draftStartDeltaGapDuplicateRefusalAbandonAndOrphanSemantics() throws {
    let projection = try CanonicalProjection.bootstrap(bootstrap())
    let events = try draftEvents()
    var reducer = DraftReducer()
    try reducer.apply(events[1], projection: projection)
    #expect(reducer.drafts.isEmpty)
    try reducer.apply(events[0], projection: projection)
    try reducer.apply(events[1], projection: projection)
    try reducer.apply(events[1], projection: projection)
    #expect(reducer.drafts.values.first?.text == "A safe preliminary answer.")
    var gapObject = try #require(
        JSONSerialization.jsonObject(with: JSONEncoder().encode(events[1])) as? [String: Any])
    gapObject["delta_sequence"] = 3
    gapObject["event_id"] = "01890f3e-7b2c-7cc1-8c23-5b8f7b3aa090"
    let gap = try JSONDecoder().decode(EphemeralDraftEnvelope.self, from: JSONSerialization.data(withJSONObject: gapObject))
    try reducer.apply(gap, projection: projection)
    #expect(reducer.drafts.values.first?.greatestSequence == 3)
    var supersedingObject = try #require(
        JSONSerialization.jsonObject(with: JSONEncoder().encode(events[0])) as? [String: Any])
    supersedingObject["event_id"] = testID("68").rawValue
    supersedingObject["draft_id"] = testID("69").rawValue
    supersedingObject["invocation_id"] = testID("70").rawValue
    let superseding = try JSONDecoder().decode(
        EphemeralDraftEnvelope.self,
        from: JSONSerialization.data(withJSONObject: supersedingObject))
    try reducer.apply(superseding, projection: projection)
    #expect(reducer.drafts.count == 1)
    #expect(reducer.drafts.values.first?.draftID == testID("69"))
    try reducer.apply(events[2], projection: projection)
    #expect(reducer.drafts.count == 1)
    try reducer.apply(events[3], projection: projection)
    try reducer.apply(events[4], projection: projection)
    let refusal = try #require(reducer.drafts.values.first)
    #expect(refusal.text.isEmpty)
    #expect(refusal.refusal == "I cannot help with that.")
    reducer.clear(workID: refusal.workID)
    #expect(reducer.drafts.isEmpty)
    reducer.clearAll()
    #expect(reducer.drafts.isEmpty)
}

@Test func outboxValidatesStableExactMaterialAndDetectsCorruption() throws {
    let id = ProtocolID(rawValue: "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c31")!
    let body = try JSONEncoder().encode(CancellationRequest(clientCommandID: id))
    let path = "/v1/work-items/\(id.rawValue)/cancel"
    let hash = CommandMaterial.hash(method: "POST", path: path, idempotencyKey: id.rawValue, body: body)
    let command = PendingCommand(
        kind: .cancellation, commandID: id, path: path, body: body,
        idempotencyKey: id.rawValue, materialHash: hash, profileID: profileID,
        craxiiID: nil, credentialGeneration: 0)
    #expect(try DisposableClientState(profile: profile, outbox: [command]).validated().outbox == [command])
    let corrupt = PendingCommand(
        kind: .cancellation, commandID: id, path: path, body: body,
        idempotencyKey: id.rawValue, materialHash: String(repeating: "0", count: 64),
        profileID: profileID, craxiiID: nil, credentialGeneration: 0)
    #expect(throws: ClientError.outboxCorrupt) {
        _ = try DisposableClientState(profile: profile, outbox: [corrupt]).validated()
    }
}

private actor FakeCredentialStore: CredentialStoring {
    var stored: BearerToken?
    init(_ token: BearerToken? = token) { stored = token }
    func add(token: BearerToken, profileID: ProtocolID) throws { guard stored == nil else { throw ClientError.keychainFailure(-1) }; stored = token }
    func update(token: BearerToken, profileID: ProtocolID) throws { guard stored != nil else { throw ClientError.credentialRequired }; stored = token }
    func read(profileID: ProtocolID) throws -> BearerToken { guard let stored else { throw ClientError.credentialRequired }; return stored }
    func delete(profileID: ProtocolID) { stored = nil }
}

private actor FakeLocalStore: LocalStateStoring {
    var state: DisposableClientState
    init(_ state: DisposableClientState = DisposableClientState(profile: profile)) { self.state = state }
    func load() -> DisposableClientState { state }
    func save(_ state: DisposableClientState) { self.state = state }
    func reset() { state = DisposableClientState() }
}

private actor FailingSaveLocalStore: LocalStateStoring {
    var state = DisposableClientState(profile: profile)
    var failSaves = false
    func load() -> DisposableClientState { state }
    func save(_ state: DisposableClientState) throws {
        if failSaves { throw ClientError.cacheCorrupt }
        self.state = state
    }
    func reset() { state = DisposableClientState() }
    func setFailSaves(_ value: Bool) { failSaves = value }
}

private actor SuspendedLoadLocalStore: LocalStateStoring {
    private var state: DisposableClientState
    private var didSuspendFirstLoad = false
    private var suspendedValue: DisposableClientState?
    private var suspendedLoad: CheckedContinuation<DisposableClientState, Error>?

    init(_ state: DisposableClientState) { self.state = state }

    func load() async throws -> DisposableClientState {
        guard !didSuspendFirstLoad else { return state }
        didSuspendFirstLoad = true
        suspendedValue = state
        return try await withCheckedThrowingContinuation { suspendedLoad = $0 }
    }

    func save(_ state: DisposableClientState) { self.state = state }
    func reset() { state = DisposableClientState() }
    func hasSuspendedLoad() -> Bool { suspendedLoad != nil }
    func storedState() -> DisposableClientState { state }

    func releaseSuspendedLoad() {
        guard let suspendedLoad, let suspendedValue else { return }
        self.suspendedLoad = nil
        self.suspendedValue = nil
        suspendedLoad.resume(returning: suspendedValue)
    }
}

private enum FakeHTTPAction: Sendable { case response(HTTPResponse), failure(ClientError) }
private actor FakeHTTP: HTTPExecuting {
    var actions: [FakeHTTPAction]
    var requests: [HTTPRequest] = []
    init(_ actions: [FakeHTTPAction]) { self.actions = actions }
    func execute(_ request: HTTPRequest) throws -> HTTPResponse {
        requests.append(request)
        guard !actions.isEmpty else { throw ClientError.serverUnavailable }
        switch actions.removeFirst() {
        case let .response(value): return value
        case let .failure(error): throw error
        }
    }
    func captured() -> [HTTPRequest] { requests }
}

private actor SuspendedHTTP: HTTPExecuting {
    private struct Pending {
        let id: Int
        let request: HTTPRequest
        let continuation: CheckedContinuation<HTTPResponse, Error>
    }

    private var nextID = 1
    private var pending: [Pending] = []

    func execute(_ request: HTTPRequest) async throws -> HTTPResponse {
        try await withCheckedThrowingContinuation { continuation in
            pending.append(Pending(id: nextID, request: request, continuation: continuation))
            nextID += 1
        }
    }

    func pendingID(port: Int) -> Int? {
        pending.first(where: { $0.request.url.port == port })?.id
    }

    func pendingIDs(port: Int) -> [Int] {
        pending.filter { $0.request.url.port == port }.map(\.id)
    }

    func release(id: Int, response: HTTPResponse) {
        guard let index = pending.firstIndex(where: { $0.id == id }) else { return }
        pending.remove(at: index).continuation.resume(returning: response)
    }
}

private actor RoutingHTTP: HTTPExecuting {
    let responses: [Int: HTTPResponse]
    private var requests: [HTTPRequest] = []

    init(responses: [Int: HTTPResponse]) { self.responses = responses }

    func execute(_ request: HTTPRequest) throws -> HTTPResponse {
        requests.append(request)
        guard let port = request.url.port, let response = responses[port] else {
            throw ClientError.serverUnavailable
        }
        return response
    }

    func captured() -> [HTTPRequest] { requests }
}

private actor FakeConnection: EventStreamConnection {
    enum Item: Sendable { case frame(Data), failure(ClientError) }
    var items: [Item] = []
    var waiters: [CheckedContinuation<Item, Never>] = []
    var closed = false
    func receive() async throws -> EventStreamMessage {
        let item: Item
        if items.isEmpty {
            item = await withCheckedContinuation { waiters.append($0) }
        } else { item = items.removeFirst() }
        switch item { case let .frame(data): return .text(data); case let .failure(error): throw error }
    }
    func ping() throws {}
    func close() { closed = true; for waiter in waiters { waiter.resume(returning: .failure(.networkOffline)) }; waiters.removeAll() }
    func feed(_ item: Item) { if waiters.isEmpty { items.append(item) } else { waiters.removeFirst().resume(returning: item) } }
    func isClosed() -> Bool { closed }
}

private actor FakeOpener: EventStreamOpening {
    var connections: [FakeConnection]
    var opens = 0
    var failAfterConnections = false
    init(_ connections: [FakeConnection]) { self.connections = connections }
    func open(url: URL, authorization: BearerToken) throws -> any EventStreamConnection {
        opens += 1
        if !connections.isEmpty { return connections.removeFirst() }
        throw ClientError.serverUnavailable
    }
    func count() -> Int { opens }
}

private actor SuspendedOpener: EventStreamOpening {
    private struct Pending {
        let id: Int
        let url: URL
        let continuation: CheckedContinuation<any EventStreamConnection, Error>
    }

    private var nextID = 1
    private var immediate: [any EventStreamConnection]
    private var pending: [Pending] = []

    init(immediate: [any EventStreamConnection] = []) { self.immediate = immediate }

    func open(url: URL, authorization: BearerToken) async throws -> any EventStreamConnection {
        if !immediate.isEmpty { return immediate.removeFirst() }
        return try await withCheckedThrowingContinuation { continuation in
            pending.append(Pending(id: nextID, url: url, continuation: continuation))
            nextID += 1
        }
    }

    func pendingID(port: Int) -> Int? { pending.first(where: { $0.url.port == port })?.id }
    func pendingIDs() -> [Int] { pending.map(\.id) }

    func release(id: Int, connection: any EventStreamConnection) {
        guard let index = pending.firstIndex(where: { $0.id == id }) else { return }
        pending.remove(at: index).continuation.resume(returning: connection)
    }
}

private actor ShutdownJoinConnection: EventStreamConnection {
    private var waiter: CheckedContinuation<EventStreamMessage, Error>?
    private var closed = false

    func receive() async throws -> EventStreamMessage {
        try await withCheckedThrowingContinuation { waiter = $0 }
    }

    func ping() throws {}
    func close() { closed = true }
    func hasReceiveWaiter() -> Bool { waiter != nil }
    func isClosed() -> Bool { closed }

    func releaseReceive() {
        waiter?.resume(throwing: ClientError.networkOffline)
        waiter = nil
    }
}

private actor CompletionFlag {
    private var complete = false
    func markComplete() { complete = true }
    func isComplete() -> Bool { complete }
}

private struct NoSleep: ClientSleeping {
    func sleep(for duration: Duration) async throws {
        if duration >= .seconds(30) { try await Task.sleep(for: .seconds(3_600)) }
    }
}
private struct ZeroJitter: RetryJitterSource { func milliseconds(upperBound: UInt64) -> UInt64 { 0 } }
private actor FixedIDs: UUIDv7Generating {
    var values: [ProtocolID]
    init(_ values: [ProtocolID]) { self.values = values }
    func next() throws -> ProtocolID { try #require(values.isEmpty == false); return values.removeFirst() }
}

private func response<T: Encodable>(_ value: T, status: Int) throws -> HTTPResponse {
    HTTPResponse(statusCode: status, body: try JSONEncoder().encode(value))
}

private func pendingFixtureCommand(
    profile: BackendProfile, craxiiID: ProtocolID?,
    disposition: OutboxDisposition = .sendable
) throws -> PendingCommand {
    let request = try JSONDecoder().decode(
        MessageRequest.self, from: fixture("message-request.json"))
    let path = "/v1/conversations/01890f3e-7b2c-7cc1-8c23-5b8f7b3aa007/messages"
    let body = try JSONEncoder().encode(request)
    return PendingCommand(
        kind: .message, commandID: request.clientMessageID, path: path, body: body,
        idempotencyKey: request.clientMessageID.rawValue,
        materialHash: CommandMaterial.hash(
            method: "POST", path: path, idempotencyKey: request.clientMessageID.rawValue,
            body: body),
        profileID: profile.profileID, craxiiID: craxiiID,
        credentialGeneration: profile.credentialGeneration, disposition: disposition)
}

private func bootstrapResponse() throws -> HTTPResponse { HTTPResponse(statusCode: 200, body: try fixture("bootstrap-snapshot.json")) }
private func bootstrapResponse(craxiiID: ProtocolID, cursor: Int64) throws -> HTTPResponse {
    var object = try #require(
        JSONSerialization.jsonObject(with: fixture("bootstrap-snapshot.json")) as? [String: Any])
    var craxii = try #require(object["craxii"] as? [String: Any])
    craxii["craxii_id"] = craxiiID.rawValue
    object["craxii"] = craxii
    object["snapshot_cursor"] = cursor
    return HTTPResponse(
        statusCode: 200, body: try JSONSerialization.data(withJSONObject: object))
}
private func syncData(cursor: Int64 = 4) throws -> Data {
    var object = try #require(JSONSerialization.jsonObject(with: fixture("sync-complete.json")) as? [String: Any])
    object["through_cursor"] = cursor
    return try JSONSerialization.data(withJSONObject: object)
}
private func testID(_ suffix: String) -> ProtocolID {
    ProtocolID(rawValue: "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c\(suffix)")!
}

private func eventually(_ predicate: @escaping @Sendable () async -> Bool) async -> Bool {
    for _ in 0..<1_000 {
        if await predicate() { return true }
        await Task.yield()
    }
    return false
}

private func durableFrame(
    eventType: String, cursor: Int64, eventID: ProtocolID,
    payload: [String: Any]
) throws -> Data {
    let base = try #require(durableEvents().last)
    let template = try #require(
        JSONSerialization.jsonObject(with: JSONEncoder().encode(base))
            as? [String: Any])
    var object = template
    object["event_type"] = eventType
    object["cursor"] = cursor
    object["event_id"] = eventID.rawValue
    object["payload"] = payload
    return try JSONSerialization.data(withJSONObject: object)
}

@Test func lostResponseRetryUsesIdenticalMessageIdentityPathBodyHeaderAndHash() async throws {
    let commandID = testID("40")
    let receipt = try JSONDecoder().decode(MessageReceipt.self, from: fixture("message-response.json"))
    let http = FakeHTTP([.response(try bootstrapResponse()), .failure(.serverUnavailable), .response(try response(receipt, status: 202))])
    let connection = FakeConnection()
    let session = ClientSession(
        profile: profile, allowDebugLocalhostHTTP: true,
        credentialStore: FakeCredentialStore(), localStore: FakeLocalStore(), http: http,
        streams: FakeOpener([connection]), identifiers: FixedIDs([commandID]),
        sleeper: NoSleep(), jitter: ZeroJitter())
    await session.start()
    await connection.feed(.frame(try syncData()))
    await Task.yield()
    _ = try await session.submitMessage(text: "retry me")
    let requests = await http.captured().filter { $0.method == "POST" }
    #expect(requests.count == 2)
    #expect(requests[0].idempotencyKey == commandID.rawValue)
    #expect(requests[0].url.path == requests[1].url.path)
    #expect(requests[0].body == requests[1].body)
}

@Test func messagePreparationPersistsExactVisibleMaterialBeforeAnySendAndReconcilesOnEvent() async throws {
    let commandID = testID("60")
    let receipt = try JSONDecoder().decode(MessageReceipt.self, from: fixture("message-response.json"))
    let http = FakeHTTP([
        .response(try bootstrapResponse()), .response(try response(receipt, status: 202)),
    ])
    let local = FakeLocalStore()
    let connection = FakeConnection()
    let session = ClientSession(
        profile: profile, allowDebugLocalhostHTTP: true,
        credentialStore: FakeCredentialStore(), localStore: local, http: http,
        streams: FakeOpener([connection]), identifiers: FixedIDs([commandID]),
        sleeper: NoSleep(), jitter: ZeroJitter())
    await session.start()
    await connection.feed(.frame(try syncData()))
    #expect(await eventually { await session.currentSnapshot().connectionState == .live })

    let text = "  exact\nbytes  "
    let before = await session.currentSnapshot().presentationRevision
    let prepared = try await session.prepareMessage(text: text)
    let afterPrepare = await session.currentSnapshot()
    #expect(prepared.clientMessageID == commandID)
    #expect(await http.captured().filter { $0.method == "POST" }.isEmpty)
    #expect(await local.state.outbox.count == 1)
    #expect(afterPrepare.presentationRevision > before)
    #expect(afterPrepare.pendingCommands == [PendingCommandProjection(
        commandID: commandID, kind: .message,
        conversationID: afterPrepare.projection.primaryConversation?.conversationID,
        workID: nil, clientMessageID: commandID, visibleMessageText: text,
        deliveryState: .notSent)])

    _ = try await session.sendPreparedMessage(prepared)
    let accepted = await session.currentSnapshot()
    #expect(await local.state.outbox.isEmpty)
    #expect(accepted.pendingCommands.first?.deliveryState == .acceptedAwaitingProjection)
    #expect(accepted.projection.messages.count == 1)

    let message = try durableFrame(
        eventType: "message.accepted", cursor: 5, eventID: testID("61"),
        payload: [
            "message_id": testID("62").rawValue,
            "role": "user",
            "content": [["type": "text", "text": text]],
            "client_message_id": commandID.rawValue,
            "committed_at": "2026-09-03T00:00:00Z",
        ])
    await connection.feed(.frame(message))
    #expect(await eventually { await session.currentSnapshot().projection.messages.count == 2 })
    #expect(await session.currentSnapshot().pendingCommands.isEmpty)
}

@Test func failedMessagePreparationRollsBackMemoryAndLeavesNoPreparedCommand() async throws {
    let commandID = testID("63")
    let local = FailingSaveLocalStore()
    let connection = FakeConnection()
    let session = ClientSession(
        profile: profile, allowDebugLocalhostHTTP: true,
        credentialStore: FakeCredentialStore(), localStore: local,
        http: FakeHTTP([.response(try bootstrapResponse())]),
        streams: FakeOpener([connection]), identifiers: FixedIDs([commandID]),
        sleeper: NoSleep())
    await session.start()
    await connection.feed(.frame(try syncData()))
    #expect(await eventually { await session.currentSnapshot().connectionState == .live })
    await local.setFailSaves(true)

    await #expect(throws: ClientError.cacheCorrupt) {
        _ = try await session.prepareMessage(text: "must remain in composer")
    }
    #expect(await session.currentSnapshot().pendingCommands.isEmpty)
    #expect(await local.state.outbox.isEmpty)
}

@Test func repeatedIdenticalPreparedMessagesKeepDistinctClientIdentities() async throws {
    let firstID = testID("64")
    let secondID = testID("65")
    let local = FakeLocalStore()
    let connection = FakeConnection()
    let session = ClientSession(
        profile: profile, allowDebugLocalhostHTTP: true,
        credentialStore: FakeCredentialStore(), localStore: local,
        http: FakeHTTP([.response(try bootstrapResponse())]),
        streams: FakeOpener([connection]), identifiers: FixedIDs([firstID, secondID]),
        sleeper: NoSleep())
    await session.start()
    await connection.feed(.frame(try syncData()))
    #expect(await eventually { await session.currentSnapshot().connectionState == .live })

    let first = try await session.prepareMessage(text: "same")
    let second = try await session.prepareMessage(text: "same")
    #expect(first.clientMessageID == firstID)
    #expect(second.clientMessageID == secondID)
    #expect(await local.state.outbox.map(\.commandID) == [firstID, secondID])
    #expect(await session.currentSnapshot().pendingCommands.map(\.visibleMessageText) == ["same", "same"])
}

@Test func ambiguousAndDefiniteCommandFailuresHaveDifferentSafeDeliveryStates() async throws {
    let ambiguousID = testID("66")
    let ambiguousConnection = FakeConnection()
    let ambiguous = ClientSession(
        profile: profile, allowDebugLocalhostHTTP: true,
        credentialStore: FakeCredentialStore(), localStore: FakeLocalStore(),
        http: FakeHTTP([
            .response(try bootstrapResponse()), .failure(.serverUnavailable),
            .failure(.serverUnavailable), .failure(.serverUnavailable),
        ]), streams: FakeOpener([ambiguousConnection]), identifiers: FixedIDs([ambiguousID]),
        sleeper: NoSleep(), jitter: ZeroJitter())
    await ambiguous.start()
    await ambiguousConnection.feed(.frame(try syncData()))
    #expect(await eventually { await ambiguous.currentSnapshot().connectionState == .live })
    await #expect(throws: ClientError.serverUnavailable) {
        _ = try await ambiguous.submitMessage(text: "ambiguous")
    }
    #expect(await ambiguous.currentSnapshot().pendingCommands.first?.deliveryState
        == .deliveryNotConfirmed)

    var object = try #require(
        JSONSerialization.jsonObject(with: fixture("error-envelope.json")) as? [String: Any])
    var detail = try #require(object["error"] as? [String: Any])
    detail["code"] = "definite_rejection"
    detail["message"] = "Safe definite rejection"
    detail["retryable"] = false
    object["error"] = detail
    let definiteConnection = FakeConnection()
    let definite = ClientSession(
        profile: profile, allowDebugLocalhostHTTP: true,
        credentialStore: FakeCredentialStore(), localStore: FakeLocalStore(),
        http: FakeHTTP([
            .response(try bootstrapResponse()),
            .response(HTTPResponse(
                statusCode: 422,
                body: try JSONSerialization.data(withJSONObject: object))),
        ]), streams: FakeOpener([definiteConnection]), identifiers: FixedIDs([testID("67")]),
        sleeper: NoSleep(), jitter: ZeroJitter())
    await definite.start()
    await definiteConnection.feed(.frame(try syncData()))
    #expect(await eventually { await definite.currentSnapshot().connectionState == .live })
    do {
        _ = try await definite.submitMessage(text: "definite")
        Issue.record("expected definite rejection")
    } catch let ClientError.backend(error) {
        #expect(error.code == "definite_rejection")
        #expect(error.message == "Safe definite rejection")
    }
    let definiteSnapshot = await definite.currentSnapshot()
    #expect(definiteSnapshot.pendingCommands.first?.deliveryState == .notSent)
    #expect(definiteSnapshot.lastBackendError?.message == "Safe definite rejection")
}

@Test func nonretryableAuthenticationStopsCommandAndSuppressesReconnectHammering() async throws {
    let commandID = testID("41")
    let error = try fixture("error-envelope.json")
    let http = FakeHTTP([.response(try bootstrapResponse()), .response(HTTPResponse(statusCode: 401, body: error))])
    let connection = FakeConnection()
    let opener = FakeOpener([connection])
    let session = ClientSession(
        profile: profile, allowDebugLocalhostHTTP: true,
        credentialStore: FakeCredentialStore(), localStore: FakeLocalStore(), http: http,
        streams: opener, identifiers: FixedIDs([commandID]), sleeper: NoSleep(), jitter: ZeroJitter())
    await session.start()
    await connection.feed(.frame(try syncData()))
    await Task.yield()
    await #expect(throws: ClientError.authentication) { _ = try await session.submitMessage(text: "no retry") }
    #expect(await http.captured().filter { $0.method == "POST" }.count == 1)
    await connection.feed(.failure(.authentication))
    for _ in 0..<10 { await Task.yield() }
    #expect(await opener.count() == 1)
    #expect(await session.currentSnapshot().connectionState == .authenticationFailed)
}

@Test func explicitlyNonretryableServerErrorNeverFallsIntoTransportRetry() async throws {
    var object = try #require(
        JSONSerialization.jsonObject(with: fixture("error-envelope.json")) as? [String: Any])
    var detail = try #require(object["error"] as? [String: Any])
    detail["code"] = "service_unavailable"
    detail["retryable"] = false
    object["error"] = detail
    let http = FakeHTTP([
        .response(try bootstrapResponse()),
        .response(HTTPResponse(
            statusCode: 500, body: try JSONSerialization.data(withJSONObject: object))),
    ])
    let local = FakeLocalStore()
    let session = ClientSession(
        profile: profile, allowDebugLocalhostHTTP: true,
        credentialStore: FakeCredentialStore(), localStore: local, http: http,
        streams: FakeOpener([FakeConnection()]), identifiers: FixedIDs([testID("49")]),
        sleeper: NoSleep(), jitter: ZeroJitter())
    await session.start()
    await #expect(throws: ClientError.serverUnavailable) {
        _ = try await session.submitMessage(text: "definitive rejection")
    }
    #expect(await http.captured().filter { $0.method == "POST" }.count == 1)
    #expect(await local.state.outbox.first?.disposition == .reconciliationOnly)
}

@Test func currentGenerationSyncGatesDraftAndSupersededSocketCannotMutateState() async throws {
    let first = FakeConnection(); let second = FakeConnection()
    let http = FakeHTTP([.response(try bootstrapResponse()), .response(try bootstrapResponse())])
    let session = ClientSession(
        profile: profile, allowDebugLocalhostHTTP: true,
        credentialStore: FakeCredentialStore(), localStore: FakeLocalStore(), http: http,
        streams: FakeOpener([first, second]), identifiers: FixedIDs([]), sleeper: NoSleep(), jitter: ZeroJitter())
    await session.start()
    let drafts = try draftEvents()
    await first.feed(.frame(try JSONEncoder().encode(drafts[0])))
    await Task.yield()
    #expect(await session.currentSnapshot().drafts.isEmpty)
    await first.feed(.frame(try syncData()))
    await first.feed(.frame(try JSONEncoder().encode(drafts[0])))
    for _ in 0..<100 {
        if await session.currentSnapshot().drafts.count == 1 { break }
        await Task.yield()
    }
    #expect(await session.currentSnapshot().drafts.count == 1)
    await session.retryConnection()
    #expect(await session.currentSnapshot().drafts.isEmpty)
    await first.feed(.frame(try JSONEncoder().encode(drafts[1])))
    await second.feed(.frame(try syncData()))
    for _ in 0..<100 { await Task.yield() }
    #expect(await session.currentSnapshot().drafts.isEmpty)
}

@Test func relaunchBootstrapReconcilesPendingMessageWithoutResendAndNeverRestoresDraft() async throws {
    let request = try JSONDecoder().decode(MessageRequest.self, from: fixture("message-request.json"))
    let path = "/v1/conversations/01890f3e-7b2c-7cc1-8c23-5b8f7b3aa007/messages"
    let body = try JSONEncoder().encode(request)
    let snapshot = try bootstrap()
    let pending = PendingCommand(
        kind: .message, commandID: request.clientMessageID, path: path, body: body,
        idempotencyKey: request.clientMessageID.rawValue,
        materialHash: CommandMaterial.hash(method: "POST", path: path, idempotencyKey: request.clientMessageID.rawValue, body: body),
        profileID: profileID, craxiiID: snapshot.craxii.craxiiID, credentialGeneration: 0)
    let local = FakeLocalStore(DisposableClientState(
        profile: profile, boundCraxiiID: snapshot.craxii.craxiiID,
        lastAppliedCursor: Cursor(rawValue: 1)!, outbox: [pending]))
    let http = FakeHTTP([.response(try bootstrapResponse())])
    let session = ClientSession(
        profile: profile, allowDebugLocalhostHTTP: true,
        credentialStore: FakeCredentialStore(), localStore: local, http: http,
        streams: FakeOpener([FakeConnection()]), identifiers: FixedIDs([]), sleeper: NoSleep())
    await session.start()
    #expect(await http.captured().count == 1)
    #expect(await session.currentSnapshot().pendingCommandCount == 0)
    #expect(await session.currentSnapshot().drafts.isEmpty)
}

@Test func cancellationUsesFreshStableIdentityAndReceiptDoesNotFabricateCanonicalState() async throws {
    let commandID = testID("42")
    let receipt = try JSONDecoder().decode(
        CancellationReceipt.self, from: fixture("cancellation-response.json"))
    let http = FakeHTTP([
        .response(try bootstrapResponse()), .response(try response(receipt, status: 202)),
    ])
    let connection = FakeConnection()
    let local = FakeLocalStore()
    let session = ClientSession(
        profile: profile, allowDebugLocalhostHTTP: true,
        credentialStore: FakeCredentialStore(), localStore: local, http: http,
        streams: FakeOpener([connection]), identifiers: FixedIDs([commandID]),
        sleeper: NoSleep(), jitter: ZeroJitter())
    await session.start()
    await connection.feed(.frame(try syncData()))
    for _ in 0..<100 {
        if await session.currentSnapshot().connectionState == .live { break }
        await Task.yield()
    }
    let workID = try #require((try bootstrap()).workItems.first?.workID)
    let prepared = try await session.prepareCancellation(workID: workID)
    #expect(prepared.clientCommandID == commandID)
    #expect(await http.captured().filter { $0.method == "POST" }.isEmpty)
    #expect(await local.state.outbox.count == 1)
    #expect(await session.currentSnapshot().pendingCommands.first?.deliveryState == .notSent)
    _ = try await session.sendPreparedCancellation(prepared)
    let request = try #require(await http.captured().last)
    #expect(request.idempotencyKey == commandID.rawValue)
    #expect(request.url.path == "/v1/work-items/\(workID.rawValue)/cancel")
    #expect(try JSONDecoder().decode(CancellationRequest.self, from: try #require(request.body)).clientCommandID == commandID)
    #expect(await session.currentSnapshot().projection.works.first?.state == .queued)
    #expect(await local.state.outbox.isEmpty)
    #expect(await session.currentSnapshot().pendingCommands.first?.deliveryState
        == .acceptedAwaitingProjection)
}

@Test func durableCompletionWinsCancellationRaceAndClearsCancellationTruth() throws {
    var projection = try CanonicalProjection.bootstrap(bootstrap())
    let workID = try #require(projection.works.first?.workID)
    let requested = try JSONDecoder().decode(
        DurableEventEnvelope.self,
        from: durableFrame(
            eventType: "work.cancel_requested", cursor: 5, eventID: testID("71"),
            payload: [
                "state": "cancel_requested",
                "transitioned_at": "2026-09-03T00:00:01Z",
            ]))
    projection = try DurableReducer().applying(requested, to: projection)
    #expect(projection.works.first?.state == .cancelRequested)
    #expect(projection.works.first?.cleanupPending == true)

    let completed = try JSONDecoder().decode(
        DurableEventEnvelope.self,
        from: durableFrame(
            eventType: "work.completed", cursor: 6, eventID: testID("72"),
            payload: [
                "state": "completed",
                "terminal_reason": "answered",
                "transitioned_at": "2026-09-03T00:00:02Z",
            ]))
    projection = try DurableReducer().applying(completed, to: projection)
    #expect(projection.works.first?.workID == workID)
    #expect(projection.works.first?.state == .completed)
    #expect(projection.works.first?.cancelRequestedAt == nil)
    #expect(projection.works.first?.cleanupPending == false)
}

@Test func safeToolProjectionContainsOnlyPublicFactsAndTracksUnknownOutcome() throws {
    var projection = try CanonicalProjection.bootstrap(bootstrap())
    let executionID = testID("43")
    let started = try JSONDecoder().decode(DurableEventEnvelope.self, from: durableFrame(
        eventType: "tool.execution_started", cursor: 5, eventID: testID("44"),
        payload: [
            "tool_execution_id": executionID.rawValue,
            "status": "dispatching",
            "observed_at": "2026-08-28T00:00:02.000000Z",
        ]))
    projection = try DurableReducer().applying(started, to: projection)
    let finished = try JSONDecoder().decode(DurableEventEnvelope.self, from: durableFrame(
        eventType: "tool.execution_finished", cursor: 8, eventID: testID("45"),
        payload: [
            "tool_execution_id": executionID.rawValue,
            "status": "outcome_unknown",
            "outcome_unknown": true,
            "observed_at": "2026-08-28T00:00:03.000000Z",
        ]))
    projection = try DurableReducer().applying(finished, to: projection)
    let tool = try #require(projection.works.first?.tools.first)
    #expect(tool.executionID == executionID)
    #expect(tool.toolName == nil)
    #expect(tool.status == "outcome_unknown")
    #expect(tool.resultClass == nil)
    #expect(tool.outcomeUnknown)
    #expect(projection.unresolvedOutcomes == [UnresolvedOutcomeDTO(
        kind: .toolOutcomeUnknown, workID: projection.works[0].workID,
        toolExecutionID: executionID)])
}

@Test func authoritativeAssistantCommitClearsEveryDraftForItsWork() async throws {
    let connection = FakeConnection()
    let session = ClientSession(
        profile: profile, allowDebugLocalhostHTTP: true,
        credentialStore: FakeCredentialStore(), localStore: FakeLocalStore(),
        http: FakeHTTP([.response(try bootstrapResponse())]),
        streams: FakeOpener([connection]), identifiers: FixedIDs([]), sleeper: NoSleep())
    await session.start()
    await connection.feed(.frame(try syncData()))
    await connection.feed(.frame(try JSONEncoder().encode(try draftEvents()[0])))
    for _ in 0..<100 {
        if await session.currentSnapshot().drafts.count == 1 { break }
        await Task.yield()
    }
    #expect(await session.currentSnapshot().drafts.count == 1)
    let workID = try #require((try bootstrap()).workItems.first?.workID)
    let messageID = testID("46")
    let committed = try durableFrame(
        eventType: "assistant.message_committed", cursor: 6, eventID: testID("47"),
        payload: [
            "message_id": messageID.rawValue,
            "role": "assistant",
            "content": [["type": "text", "text": "authoritative"]],
            "work_id": workID.rawValue,
            "committed_at": "2026-08-28T00:00:03.000000Z",
        ])
    await connection.feed(.frame(committed))
    for _ in 0..<100 {
        if await session.currentSnapshot().projection.messages.count == 2 { break }
        await Task.yield()
    }
    #expect(await session.currentSnapshot().drafts.isEmpty)
    #expect(await session.currentSnapshot().projection.messages.last?.messageID == messageID)
}

@Test func unknownDurableEventBootstrapsOnceThenBecomesFatalIfStillIncompatible() async throws {
    let first = FakeConnection()
    let second = FakeConnection()
    let opener = FakeOpener([first, second])
    let session = ClientSession(
        profile: profile, allowDebugLocalhostHTTP: true,
        credentialStore: FakeCredentialStore(), localStore: FakeLocalStore(),
        http: FakeHTTP([.response(try bootstrapResponse()), .response(try bootstrapResponse())]),
        streams: opener, identifiers: FixedIDs([]), sleeper: NoSleep())
    await session.start()
    await first.feed(.frame(try syncData()))
    let unknown = try durableFrame(
        eventType: "future.required_event", cursor: 9, eventID: testID("48"), payload: [:])
    await first.feed(.frame(unknown))
    for _ in 0..<500 {
        if await opener.count() == 2 { break }
        await Task.yield()
    }
    #expect(await opener.count() == 2)
    #expect(await session.currentSnapshot().projection.lastAppliedCursor.rawValue == 4)
    await second.feed(.frame(unknown))
    for _ in 0..<100 {
        if await session.currentSnapshot().connectionState == .fatalProtocolError { break }
        await Task.yield()
    }
    #expect(await session.currentSnapshot().connectionState == .fatalProtocolError)
    #expect(await session.currentSnapshot().projection.lastAppliedCursor.rawValue == 4)
}

@Test func unavailableReconnectEpisodeIsBoundedToEightAutomaticAttempts() async throws {
    let connection = FakeConnection()
    let opener = FakeOpener([connection])
    let session = ClientSession(
        profile: profile, allowDebugLocalhostHTTP: true,
        credentialStore: FakeCredentialStore(), localStore: FakeLocalStore(),
        http: FakeHTTP([.response(try bootstrapResponse())]),
        streams: opener, identifiers: FixedIDs([]), sleeper: NoSleep(), jitter: ZeroJitter())
    await session.start()
    await connection.feed(.frame(try syncData()))
    await connection.feed(.failure(.serverUnavailable))
    for _ in 0..<1_000 {
        if await opener.count() == 1 + ClientSession.maximumReconnectAttempts,
           await session.currentSnapshot().connectionState == .disconnected { break }
        await Task.yield()
    }
    #expect(await opener.count() == 1 + ClientSession.maximumReconnectAttempts)
    #expect(await session.currentSnapshot().connectionState == .disconnected)
}

@Test func staleLocalStateLoadAfterProfileSwitchCannotInstallOrFailNewSession() async throws {
    let staleCraxiiID = testID("54")
    let profileBID = testID("55")
    let craxiiBID = testID("56")
    let pending = try pendingFixtureCommand(profile: profile, craxiiID: staleCraxiiID)
    let local = SuspendedLoadLocalStore(DisposableClientState(
        profile: profile, boundCraxiiID: staleCraxiiID,
        lastAppliedCursor: Cursor(rawValue: 9)!, outbox: [pending]))
    let http = RoutingHTTP(responses: [
        8081: try bootstrapResponse(craxiiID: craxiiBID, cursor: 40),
    ])
    let connectionB = FakeConnection()
    let opener = FakeOpener([connectionB])
    let session = ClientSession(
        profile: profile, allowDebugLocalhostHTTP: true,
        credentialStore: FakeCredentialStore(), localStore: local, http: http,
        streams: opener, identifiers: FixedIDs([profileBID]), sleeper: NoSleep())

    let startA = Task { await session.start() }
    #expect(await eventually { await local.hasSuspendedLoad() })
    try await session.changeEndpoint(to: "http://127.0.0.1:8081/")
    let profileB = try #require((await local.storedState()).profile)
    let pendingB = try pendingFixtureCommand(
        profile: profileB, craxiiID: nil, disposition: .reconciliationOnly)
    await local.save(DisposableClientState(profile: profileB, outbox: [pendingB]))
    await session.retryConnection()
    await connectionB.feed(.frame(try syncData(cursor: 40)))
    #expect(await eventually { await session.currentSnapshot().connectionState == .live })

    await local.releaseSuspendedLoad()
    await startA.value

    let snapshot = await session.currentSnapshot()
    let stored = await local.storedState()
    #expect(snapshot.connectionState == .live)
    #expect(snapshot.lastError == nil)
    #expect(snapshot.projection.craxii?.craxiiID == craxiiBID)
    #expect(snapshot.projection.lastAppliedCursor.rawValue == 40)
    #expect(stored.profile?.profileID == profileBID)
    #expect(stored.boundCraxiiID == craxiiBID)
    #expect(stored.lastAppliedCursor.rawValue == 40)
    #expect(stored.outbox.count == 1)
    #expect(stored.outbox.first?.profileID == profileBID)
    #expect(stored.outbox.first?.craxiiID == nil)
    #expect(stored.outbox.first?.disposition == .reconciliationOnly)
    #expect(await http.captured().map(\.url.port) == [8081])
    #expect(await opener.count() == 1)
    await session.shutdown()
}

@Test func staleLocalStateLoadAfterCredentialReplacementCannotInstallOldNamespace() async throws {
    let staleCraxiiID = testID("57")
    let craxiiBID = testID("58")
    let pending = try pendingFixtureCommand(profile: profile, craxiiID: staleCraxiiID)
    let local = SuspendedLoadLocalStore(DisposableClientState(
        profile: profile, boundCraxiiID: staleCraxiiID,
        lastAppliedCursor: Cursor(rawValue: 9)!, outbox: [pending]))
    let http = RoutingHTTP(responses: [
        8080: try bootstrapResponse(craxiiID: craxiiBID, cursor: 45),
    ])
    let connectionB = FakeConnection()
    let opener = FakeOpener([connectionB])
    let session = ClientSession(
        profile: profile, allowDebugLocalhostHTTP: true,
        credentialStore: FakeCredentialStore(), localStore: local, http: http,
        streams: opener, identifiers: FixedIDs([]), sleeper: NoSleep())

    let startA = Task { await session.start() }
    #expect(await eventually { await local.hasSuspendedLoad() })
    try await session.installCredential(String(repeating: "e", count: 64))
    await session.retryConnection()
    await connectionB.feed(.frame(try syncData(cursor: 45)))
    #expect(await eventually { await session.currentSnapshot().connectionState == .live })

    await local.releaseSuspendedLoad()
    await startA.value

    let snapshot = await session.currentSnapshot()
    let stored = await local.storedState()
    #expect(snapshot.connectionState == .live)
    #expect(snapshot.lastError == nil)
    #expect(snapshot.projection.craxii?.craxiiID == craxiiBID)
    #expect(snapshot.projection.lastAppliedCursor.rawValue == 45)
    #expect(stored.profile?.profileID == profile.profileID)
    #expect(stored.profile?.credentialGeneration == 1)
    #expect(stored.boundCraxiiID == craxiiBID)
    #expect(stored.lastAppliedCursor.rawValue == 45)
    #expect(stored.outbox.isEmpty)
    #expect(await http.captured().count == 1)
    #expect(await opener.count() == 1)
    await session.shutdown()
}

@Test func currentLocalStateProfileMismatchRemainsFatal() async throws {
    let mismatchedProfile = BackendProfile(
        profileID: testID("59"), endpoint: "http://127.0.0.1:8081/")
    let http = FakeHTTP([])
    let opener = FakeOpener([])
    let session = ClientSession(
        profile: profile, allowDebugLocalhostHTTP: true,
        credentialStore: FakeCredentialStore(),
        localStore: FakeLocalStore(DisposableClientState(profile: mismatchedProfile)),
        http: http, streams: opener, identifiers: FixedIDs([]), sleeper: NoSleep())

    await session.start()

    let snapshot = await session.currentSnapshot()
    #expect(snapshot.connectionState == .fatalProtocolError)
    #expect(snapshot.lastError == .configurationMismatch)
    #expect(await http.captured().isEmpty)
    #expect(await opener.count() == 0)
}

@Test func staleBootstrapAfterProfileSwitchCannotCommitOldNamespace() async throws {
    let profileBID = testID("50")
    let craxiiBID = testID("51")
    let endpointB = "http://127.0.0.1:8081/"
    let http = SuspendedHTTP()
    let local = FakeLocalStore()
    let opener = FakeOpener([FakeConnection()])
    let session = ClientSession(
        profile: profile, allowDebugLocalhostHTTP: true,
        credentialStore: FakeCredentialStore(), localStore: local, http: http,
        streams: opener, identifiers: FixedIDs([profileBID]), sleeper: NoSleep())

    let startA = Task { await session.start() }
    #expect(await eventually { await http.pendingID(port: 8080) != nil })
    let requestA = try #require(await http.pendingID(port: 8080))

    try await session.changeEndpoint(to: endpointB)
    let startB = Task { await session.retryConnection() }
    #expect(await eventually { await http.pendingID(port: 8081) != nil })
    let requestB = try #require(await http.pendingID(port: 8081))
    await http.release(id: requestB, response: try bootstrapResponse(craxiiID: craxiiBID, cursor: 40))
    #expect(await eventually { await opener.count() == 1 })
    await http.release(id: requestA, response: try bootstrapResponse())
    await startA.value
    await startB.value

    let snapshot = await session.currentSnapshot()
    #expect(snapshot.projection.craxii?.craxiiID == craxiiBID)
    #expect(snapshot.projection.lastAppliedCursor.rawValue == 40)
    #expect(await local.state.profile?.profileID == profileBID)
    #expect(await local.state.boundCraxiiID == craxiiBID)
    #expect(await opener.count() == 1)
    await session.shutdown()
}

@Test func staleWebSocketOpenAfterProfileSwitchIsClosedBeforeInstallation() async throws {
    let profileBID = testID("52")
    let craxiiBID = testID("53")
    let endpointB = "http://127.0.0.1:8081/"
    let http = RoutingHTTP(responses: [
        8080: try bootstrapResponse(),
        8081: try bootstrapResponse(craxiiID: craxiiBID, cursor: 40),
    ])
    let opener = SuspendedOpener()
    let session = ClientSession(
        profile: profile, allowDebugLocalhostHTTP: true,
        credentialStore: FakeCredentialStore(), localStore: FakeLocalStore(), http: http,
        streams: opener, identifiers: FixedIDs([profileBID]), sleeper: NoSleep())

    let startA = Task { await session.start() }
    #expect(await eventually { await opener.pendingID(port: 8080) != nil })
    let openA = try #require(await opener.pendingID(port: 8080))
    try await session.changeEndpoint(to: endpointB)

    let startB = Task { await session.retryConnection() }
    #expect(await eventually { await opener.pendingID(port: 8081) != nil })
    let openB = try #require(await opener.pendingID(port: 8081))
    let connectionB = FakeConnection()
    await opener.release(id: openB, connection: connectionB)
    await startB.value
    await connectionB.feed(.frame(try syncData(cursor: 40)))
    #expect(await eventually { await session.currentSnapshot().connectionState == .live })

    let connectionA = FakeConnection()
    await opener.release(id: openA, connection: connectionA)
    await startA.value
    #expect(await connectionA.isClosed())
    #expect(!(await connectionB.isClosed()))
    #expect(await session.currentSnapshot().projection.craxii?.craxiiID == craxiiBID)
    #expect(await session.currentSnapshot().connectionState == .live)
    await session.shutdown()
}

@Test func newerReconnectWinsWhenOlderSocketOpenCompletesLate() async throws {
    let initial = FakeConnection()
    let opener = SuspendedOpener(immediate: [initial])
    let session = ClientSession(
        profile: profile, allowDebugLocalhostHTTP: true,
        credentialStore: FakeCredentialStore(), localStore: FakeLocalStore(),
        http: FakeHTTP([.response(try bootstrapResponse())]), streams: opener,
        identifiers: FixedIDs([]), sleeper: NoSleep())
    await session.start()
    await initial.feed(.frame(try syncData()))
    #expect(await eventually { await session.currentSnapshot().connectionState == .live })

    let olderReconnect = Task { await session.resumeTransport() }
    #expect(await eventually { await opener.pendingIDs().count == 1 })
    let olderOpen = try #require(await opener.pendingIDs().first)
    let newerReconnect = Task { await session.resumeTransport() }
    #expect(await eventually { await opener.pendingIDs().count == 2 })
    let newerOpen = try #require(await opener.pendingIDs().last)

    let newerConnection = FakeConnection()
    await opener.release(id: newerOpen, connection: newerConnection)
    await newerReconnect.value
    await newerConnection.feed(.frame(try syncData()))
    #expect(await eventually { await session.currentSnapshot().connectionState == .live })

    let olderConnection = FakeConnection()
    await opener.release(id: olderOpen, connection: olderConnection)
    await olderReconnect.value
    #expect(await olderConnection.isClosed())
    #expect(!(await newerConnection.isClosed()))
    #expect(await session.currentSnapshot().connectionState == .live)
    await session.shutdown()
}

@Test func credentialGenerationReplacementRejectsPendingOldScopeBootstrap() async throws {
    let http = SuspendedHTTP()
    let opener = FakeOpener([FakeConnection()])
    let local = FakeLocalStore()
    let session = ClientSession(
        profile: profile, allowDebugLocalhostHTTP: true,
        credentialStore: FakeCredentialStore(), localStore: local, http: http,
        streams: opener, identifiers: FixedIDs([]), sleeper: NoSleep())

    let oldStart = Task { await session.start() }
    #expect(await eventually { await http.pendingIDs(port: 8080).count == 1 })
    let oldRequest = try #require(await http.pendingIDs(port: 8080).first)
    try await session.installCredential(String(repeating: "d", count: 64))
    #expect(await local.state.profile?.credentialGeneration == 1)

    await http.release(id: oldRequest, response: try bootstrapResponse())
    await oldStart.value
    #expect(await session.currentSnapshot().projection.craxii == nil)
    #expect(await local.state.boundCraxiiID == nil)
    #expect(await opener.count() == 0)

    let currentStart = Task { await session.retryConnection() }
    #expect(await eventually { await http.pendingIDs(port: 8080).count == 1 })
    let currentRequest = try #require(await http.pendingIDs(port: 8080).first)
    await http.release(id: currentRequest, response: try bootstrapResponse())
    await currentStart.value
    #expect(await session.currentSnapshot().projection.craxii != nil)
    #expect(await opener.count() == 1)
    await session.shutdown()
}

@Test func shutdownWaitsForOwnedReceiveTaskAndRejectsLateCallback() async throws {
    let connection = ShutdownJoinConnection()
    let opener = SuspendedOpener(immediate: [connection])
    let session = ClientSession(
        profile: profile, allowDebugLocalhostHTTP: true,
        credentialStore: FakeCredentialStore(), localStore: FakeLocalStore(),
        http: FakeHTTP([.response(try bootstrapResponse())]), streams: opener,
        identifiers: FixedIDs([]), sleeper: NoSleep())
    await session.start()
    #expect(await eventually { await connection.hasReceiveWaiter() })

    let completion = CompletionFlag()
    let shutdown = Task {
        await session.shutdown()
        await completion.markComplete()
    }
    #expect(await eventually { await connection.isClosed() })
    for _ in 0..<100 { await Task.yield() }
    #expect(!(await completion.isComplete()))

    await connection.releaseReceive()
    await shutdown.value
    #expect(await completion.isComplete())
    #expect(await session.currentSnapshot().connectionState == .disconnected)
    #expect(await session.currentSnapshot().drafts.isEmpty)
}
