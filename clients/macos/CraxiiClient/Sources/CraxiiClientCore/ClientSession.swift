import Foundation
import CraxiiProtocol

public enum ClientConnectionState: String, Sendable {
    case disconnected, bootstrapping, replaying, live, reconnecting
    case authenticationFailed, fatalProtocolError
}

public struct ClientSnapshot: Equatable, Sendable {
    public let credentialStatus: CredentialStatus
    public let connectionState: ClientConnectionState
    public let projection: CanonicalProjection
    public let drafts: [DraftProjection]
    public let pendingCommandCount: Int
    public let generation: UInt64
    public let lastError: ClientError?

    public init(
        credentialStatus: CredentialStatus, connectionState: ClientConnectionState,
        projection: CanonicalProjection, drafts: [DraftProjection], pendingCommandCount: Int,
        generation: UInt64, lastError: ClientError?
    ) {
        self.credentialStatus = credentialStatus; self.connectionState = connectionState
        self.projection = projection; self.drafts = drafts
        self.pendingCommandCount = pendingCommandCount; self.generation = generation
        self.lastError = lastError
    }
}

public protocol RetryJitterSource: Sendable { func milliseconds(upperBound: UInt64) -> UInt64 }
public struct SystemRetryJitter: RetryJitterSource {
    public init() {}
    public func milliseconds(upperBound: UInt64) -> UInt64 {
        upperBound == 0 ? 0 : UInt64.random(in: 0 ... upperBound)
    }
}

private enum SessionOperationError: Error {
    case superseded
}

private struct OperationAuthority {
    let generation: UInt64
    let profile: BackendProfile
}

private struct SessionOperationFailure: Error {
    let authority: OperationAuthority
    let underlying: Error
}

private struct InvalidatedOperations {
    let connection: (any EventStreamConnection)?
    let receiveTask: Task<Void, Never>?
    let pingTask: Task<Void, Never>?
    let reconnectTask: Task<Void, Never>?
}

public actor ClientSession {
    public static let maximumCommandAttempts = 3
    public static let maximumReconnectAttempts = 8
    public static let reconnectBaseMilliseconds: UInt64 = 500
    public static let reconnectCapMilliseconds: UInt64 = 30_000

    private var profile: BackendProfile
    private let allowDebugLocalhostHTTP: Bool
    private let credentialStore: any CredentialStoring
    private let localStore: any LocalStateStoring
    private let http: any HTTPExecuting
    private let streams: any EventStreamOpening
    private let identifiers: any UUIDv7Generating
    private let sleeper: any ClientSleeping
    private let network: any NetworkStatusProviding
    private let jitter: any RetryJitterSource
    private let decoder = JSONDecoder()
    private let encoder: JSONEncoder
    private let durableReducer = DurableReducer()

    private var persisted = DisposableClientState()
    private var projection = CanonicalProjection()
    private var drafts = DraftReducer()
    private var credentialStatus: CredentialStatus = .required
    private var connectionState: ClientConnectionState = .disconnected
    private var lastError: ClientError?
    private var generation: UInt64 = 0
    private var currentConnection: (any EventStreamConnection)?
    private var receiveTask: Task<Void, Never>?
    private var pingTask: Task<Void, Never>?
    private var reconnectTask: Task<Void, Never>?
    private var reconnectTaskGeneration: UInt64?
    private var reconnectAttempt = 0
    private var lastInbound = ContinuousClock.now
    private var unknownEventRecoveryPending = false
    private var isShuttingDown = false
    private var snapshotHandler: (@Sendable (ClientSnapshot) -> Void)?

    public init(
        profile: BackendProfile, allowDebugLocalhostHTTP: Bool,
        credentialStore: any CredentialStoring, localStore: any LocalStateStoring,
        http: any HTTPExecuting, streams: any EventStreamOpening,
        identifiers: any UUIDv7Generating = UUIDv7Generator(),
        sleeper: any ClientSleeping = ContinuousClientSleeper(),
        network: any NetworkStatusProviding = AlwaysOnlineNetworkStatus(),
        jitter: any RetryJitterSource = SystemRetryJitter()
    ) {
        self.profile = profile
        self.allowDebugLocalhostHTTP = allowDebugLocalhostHTTP
        self.credentialStore = credentialStore; self.localStore = localStore
        self.http = http; self.streams = streams; self.identifiers = identifiers
        self.sleeper = sleeper; self.network = network; self.jitter = jitter
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        self.encoder = encoder
    }

    public func setSnapshotHandler(_ handler: (@Sendable (ClientSnapshot) -> Void)?) {
        snapshotHandler = handler
        publish()
    }

    public func currentSnapshot() -> ClientSnapshot { makeSnapshot() }

    public func start() async {
        guard !isShuttingDown else { return }
        let authority = currentAuthority()
        do {
            try await loadAndBindLocalState(authority: authority)
            try await connectWithBootstrap(expectedAuthority: authority)
        } catch SessionOperationError.superseded {
            return
        } catch let failure as SessionOperationFailure {
            handleStartFailure(failure.underlying, authority: failure.authority)
        } catch {
            handleStartFailure(error, authority: authority)
        }
    }

    public func retryConnection() async {
        guard !isShuttingDown else { return }
        let invalidated = invalidateConnectionAuthority(cancelReconnect: true)
        reconnectAttempt = 0
        unknownEventRecoveryPending = false
        await closeInvalidatedConnection(invalidated)
        await start()
    }

    public func installCredential(_ text: String) async throws {
        let token = try BearerToken(validating: text)
        let invalidated = invalidateConnectionAuthority(cancelReconnect: true)
        let authority = currentAuthority()
        connectionState = .disconnected
        await closeInvalidatedConnection(invalidated)
        guard isCurrent(authority) else { return }
        do {
            _ = try await credentialStore.read(profileID: authority.profile.profileID)
            try requireCurrent(authority)
            try await credentialStore.update(token: token, profileID: authority.profile.profileID)
            try requireCurrent(authority)
        } catch ClientError.credentialRequired {
            try requireCurrent(authority)
            try await credentialStore.add(token: token, profileID: authority.profile.profileID)
            try requireCurrent(authority)
        } catch SessionOperationError.superseded {
            return
        }
        profile = profile.replacingCredential()
        persisted.profile = profile
        for index in persisted.outbox.indices { persisted.outbox[index].disposition = .reconciliationOnly }
        let committedAuthority = currentAuthority()
        try await localStore.save(persisted)
        guard isCurrent(committedAuthority) else { return }
        credentialStatus = .installed
        lastError = nil
        publish()
    }

    public func deleteCredential() async throws {
        let invalidated = invalidateConnectionAuthority(cancelReconnect: true)
        let authority = currentAuthority()
        connectionState = .disconnected
        await closeInvalidatedConnection(invalidated)
        guard isCurrent(authority) else { return }
        try await credentialStore.delete(profileID: authority.profile.profileID)
        guard isCurrent(authority) else { return }
        profile = profile.replacingCredential()
        persisted.profile = profile
        for index in persisted.outbox.indices { persisted.outbox[index].disposition = .reconciliationOnly }
        let committedAuthority = currentAuthority()
        try await localStore.save(persisted)
        guard isCurrent(committedAuthority) else { return }
        credentialStatus = .required
        connectionState = .disconnected
        publish()
    }

    public func resetDisposableState() async throws {
        let invalidated = invalidateConnectionAuthority(cancelReconnect: true)
        let authority = currentAuthority()
        connectionState = .disconnected
        await closeInvalidatedConnection(invalidated)
        guard isCurrent(authority) else { return }
        try await localStore.reset()
        guard isCurrent(authority) else { return }
        persisted = DisposableClientState(profile: profile)
        projection = CanonicalProjection()
        drafts.clearAll()
        connectionState = .disconnected
        lastError = nil
        try await localStore.save(persisted)
        publish()
    }

    public func changeEndpoint(to input: String) async throws {
        let endpoint = try EndpointPolicy.validate(input, allowDebugLocalhostHTTP: allowDebugLocalhostHTTP)
        guard endpoint.absoluteString != profile.endpoint else { return }
        let invalidated = invalidateConnectionAuthority(cancelReconnect: true)
        let authority = currentAuthority()
        connectionState = .disconnected
        await closeInvalidatedConnection(invalidated)
        guard isCurrent(authority) else { return }
        let profileID = try await identifiers.next()
        guard isCurrent(authority) else { return }
        let retained = persisted.outbox.map { command in
            var command = command
            command.disposition = .reconciliationOnly
            return command
        }
        profile = BackendProfile(profileID: profileID, endpoint: endpoint.absoluteString)
        persisted = DisposableClientState(profile: profile, outbox: retained)
        projection = CanonicalProjection()
        drafts.clearAll()
        credentialStatus = .required
        connectionState = .disconnected
        try await localStore.save(persisted)
        publish()
    }

    @discardableResult
    public func submitMessage(text: String) async throws -> MessageReceipt {
        guard let conversationID = projection.primaryConversation?.conversationID,
              !text.isEmpty, text.utf8.count <= 65_536 else { throw ClientError.commandRejected("invalid_request") }
        let authority = currentAuthority()
        let commandID = try await identifiers.next()
        try requireCurrent(authority)
        let path = "/v1/conversations/\(conversationID.rawValue)/messages"
        let body = try encoder.encode(MessageRequest(
            clientMessageID: commandID, content: [ContentBlock(text: text)]))
        guard body.count <= 512 * 1_024 else { throw ClientError.commandRejected("payload_too_large") }
        let pending = makePending(kind: .message, id: commandID, path: path, body: body)
        try await appendPending(pending)
        return try await sendMessageCommand(commandID)
    }

    @discardableResult
    public func cancel(workID: WorkID) async throws -> CancellationReceipt {
        let authority = currentAuthority()
        let commandID = try await identifiers.next()
        try requireCurrent(authority)
        let path = "/v1/work-items/\(workID.rawValue)/cancel"
        let body = try encoder.encode(CancellationRequest(clientCommandID: commandID))
        let pending = makePending(kind: .cancellation, id: commandID, path: path, body: body)
        try await appendPending(pending)
        do { return try await sendCancellationCommand(commandID) }
        catch ClientError.authentication { throw ClientError.authentication }
        catch let error as ClientError where isAmbiguousRetryable(error) {
            throw ClientError.cancellationTransportFailure
        }
        catch { throw error }
    }

    public func suspendTransport() async {
        let invalidated = invalidateConnectionAuthority(cancelReconnect: true)
        connectionState = .disconnected
        publish()
        await closeInvalidatedConnection(invalidated)
    }

    public func resumeTransport() async {
        guard !isShuttingDown else { return }
        guard projection.craxii != nil else { await retryConnection(); return }
        do {
            connectionState = .reconnecting
            publish()
            try await reconnectFromCursor()
        } catch SessionOperationError.superseded {
            return
        } catch { await reconnectFailed(error) }
    }

    public func shutdown() async {
        guard !isShuttingDown else { return }
        isShuttingDown = true
        let invalidated = invalidateConnectionAuthority(cancelReconnect: true)
        connectionState = .disconnected
        publish()
        await closeInvalidatedConnection(invalidated)
        await invalidated.receiveTask?.value
        await invalidated.pingTask?.value
        await invalidated.reconnectTask?.value
        try? await localStore.save(persisted)
        publish()
    }

    private func loadAndBindLocalState(authority: OperationAuthority) async throws {
        var candidate: DisposableClientState
        var recoveredCorruptState = false
        do {
            let loaded: DisposableClientState
            do {
                loaded = try await localStore.load()
            } catch {
                try requireCurrent(authority)
                throw error
            }
            try requireCurrent(authority)
            candidate = try loaded.validated()
        } catch ClientError.cacheCorrupt {
            try requireCurrent(authority)
            do {
                try await localStore.reset()
            } catch {
                try requireCurrent(authority)
                throw error
            }
            try requireCurrent(authority)
            candidate = DisposableClientState(profile: authority.profile)
            do {
                try await localStore.save(candidate)
            } catch {
                try requireCurrent(authority)
                throw error
            }
            try requireCurrent(authority)
            recoveredCorruptState = true
        }

        try requireCurrent(authority)
        if let storedProfile = candidate.profile {
            guard storedProfile == authority.profile else { throw ClientError.configurationMismatch }
        } else {
            candidate.profile = authority.profile
            do {
                try await localStore.save(candidate)
            } catch {
                try requireCurrent(authority)
                throw error
            }
            try requireCurrent(authority)
        }
        try requireCurrent(authority)
        persisted = candidate
        if recoveredCorruptState { lastError = .cacheCorrupt }
    }

    private func connectWithBootstrap(expectedAuthority: OperationAuthority? = nil) async throws {
        if let expectedAuthority { try requireCurrent(expectedAuthority) }
        let authority = try await beginConnectionOperation()
        do {
            let baseURL = try EndpointPolicy.validate(
                authority.profile.endpoint, allowDebugLocalhostHTTP: allowDebugLocalhostHTTP)
            connectionState = .bootstrapping
            lastError = nil
            publish()
            let token = try await readCredential(for: authority)
            try requireCurrent(authority)
            let response: HTTPResponse
            do {
                response = try await http.execute(HTTPRequest(
                    url: try EndpointPolicy.endpoint(baseURL: baseURL, exactPath: "/v1/bootstrap"),
                    method: "GET", authorization: token, timeout: 35,
                    maximumResponseBytes: ProtocolConstants.maximumBootstrapBytes))
            } catch {
                try requireCurrent(authority)
                throw error
            }
            try requireCurrent(authority)
            let bootstrap: BootstrapResponse = try decodeHTTP(response, success: [200])
            let replacement = try CanonicalProjection.bootstrap(bootstrap)
            try requireCurrent(authority)
            if let bound = persisted.boundCraxiiID, bound != bootstrap.craxii.craxiiID {
                for index in persisted.outbox.indices { persisted.outbox[index].disposition = .reconciliationOnly }
                try await localStore.save(persisted)
                try requireCurrent(authority)
                throw ClientError.configurationMismatch
            }
            projection = replacement
            drafts.clearAll()
            persisted.boundCraxiiID = bootstrap.craxii.craxiiID
            persisted.lastAppliedCursor = bootstrap.snapshotCursor
            try reconcileOutbox()
            try await localStore.save(persisted)
            try requireCurrent(authority)
            try await resendReconciledOutbox(authority: authority)
            try requireCurrent(authority)
            try await openStream(
                token: token, baseURL: baseURL, cursor: projection.lastAppliedCursor,
                authority: authority)
        } catch SessionOperationError.superseded {
            throw SessionOperationError.superseded
        } catch {
            throw SessionOperationFailure(authority: authority, underlying: error)
        }
    }

    private func reconnectFromCursor() async throws {
        let authority = try await beginConnectionOperation()
        guard await network.isOnline() else {
            try requireCurrent(authority)
            throw ClientError.networkOffline
        }
        try requireCurrent(authority)
        let baseURL = try EndpointPolicy.validate(
            authority.profile.endpoint, allowDebugLocalhostHTTP: allowDebugLocalhostHTTP)
        let token = try await readCredential(for: authority)
        try requireCurrent(authority)
        try await openStream(
            token: token, baseURL: baseURL, cursor: projection.lastAppliedCursor,
            authority: authority)
    }

    private func openStream(
        token: BearerToken, baseURL: URL, cursor: Cursor, authority: OperationAuthority
    ) async throws {
        try requireCurrent(authority)
        let url = try EndpointPolicy.webSocketURL(baseURL: baseURL, cursor: cursor)
        let connection: any EventStreamConnection
        do {
            connection = try await streams.open(url: url, authorization: token)
        } catch {
            try requireCurrent(authority)
            throw error
        }
        do {
            try requireCurrent(authority)
        } catch {
            await connection.close()
            throw error
        }
        currentConnection = connection
        connectionState = .replaying
        lastInbound = ContinuousClock.now
        publish()
        receiveTask = Task {
            await self.receiveLoop(connection: connection, generation: authority.generation)
        }
        pingTask = Task {
            await self.pingLoop(connection: connection, generation: authority.generation)
        }
    }

    private func receiveLoop(connection: any EventStreamConnection, generation: UInt64) async {
        do {
            while !Task.isCancelled {
                let message = try await connection.receive()
                guard generation == self.generation else { return }
                lastInbound = ContinuousClock.now
                switch message {
                case let .text(data): try await handleFrame(data, generation: generation)
                case .binary: throw ClientError.malformedPayload
                }
            }
        } catch is CancellationError { return }
        catch { await transportEnded(error, generation: generation) }
    }

    private func pingLoop(connection: any EventStreamConnection, generation: UInt64) async {
        do {
            while !Task.isCancelled {
                try await sleeper.sleep(for: .seconds(30))
                guard generation == self.generation else { return }
                if lastInbound.duration(to: ContinuousClock.now) >= .seconds(30) {
                    try await connection.ping()
                }
            }
        } catch is CancellationError { return }
        catch { await transportEnded(error, generation: generation) }
    }

    private func handleFrame(_ data: Data, generation: UInt64) async throws {
        guard generation == self.generation else { return }
        let frame: ServerFrame
        do { frame = try ServerFrame.decode(data, decoder: decoder) }
        catch ProtocolModelError.incompatibleVersion { throw ClientError.incompatibleProtocol }
        catch { throw ClientError.malformedPayload }
        switch frame {
        case let .durable(event):
            do {
                projection = try durableReducer.applying(event, to: projection)
            } catch ProtocolModelError.unknownEventType {
                await recoverUnknownDurableEvent(generation: generation)
                return
            }
            if event.eventType == "assistant.message_committed" || terminalEventTypes.contains(event.eventType),
               let workID = event.workID { drafts.clear(workID: workID) }
            persisted.lastAppliedCursor = projection.lastAppliedCursor
            try await localStore.save(persisted)
            publish()
        case let .syncComplete(sync):
            guard connectionState == .replaying,
                  sync.deliveryKind == .ephemeral,
                  sync.eventType == "sync.complete",
                  sync.throughCursor >= projection.lastAppliedCursor else {
                throw ClientError.projectionInvariant
            }
            connectionState = .live
            reconnectAttempt = 0
            unknownEventRecoveryPending = false
            publish()
        case let .draft(event):
            guard connectionState == .live else { return }
            try drafts.apply(event, projection: projection)
            publish()
        }
    }

    private func recoverUnknownDurableEvent(generation: UInt64) async {
        guard generation == self.generation else { return }
        if unknownEventRecoveryPending {
            connectionState = .fatalProtocolError
            lastError = .incompatibleProtocol
            let invalidated = invalidateConnectionAuthority()
            await closeInvalidatedConnection(invalidated)
            publish()
            return
        }
        unknownEventRecoveryPending = true
        let invalidated = invalidateConnectionAuthority()
        let recoveryGeneration = self.generation
        await closeInvalidatedConnection(invalidated)
        guard recoveryGeneration == self.generation, !isShuttingDown else { return }
        connectionState = .bootstrapping
        publish()
        reconnectTaskGeneration = recoveryGeneration
        reconnectTask = Task {
            do {
                try await self.connectWithBootstrap()
                self.clearReconnectTask(expectedGeneration: recoveryGeneration)
            } catch SessionOperationError.superseded {
                self.clearReconnectTask(expectedGeneration: recoveryGeneration)
            } catch let failure as SessionOperationFailure {
                guard self.clearReconnectTask(expectedGeneration: recoveryGeneration) else { return }
                self.handleStartFailure(failure.underlying, authority: failure.authority)
            } catch {
                guard self.clearReconnectTask(expectedGeneration: recoveryGeneration) else { return }
                self.handleStartFailure(error, authority: self.currentAuthority())
            }
        }
    }

    private func transportEnded(_ error: Error, generation: UInt64) async {
        guard generation == self.generation else { return }
        let invalidated = invalidateConnectionAuthority()
        let authority = currentAuthority()
        await closeInvalidatedConnection(invalidated)
        guard isCurrent(authority) else { return }
        let mapped = mapTransportError(error)
        lastError = mapped
        if mapped == .authentication {
            connectionState = .authenticationFailed
            publish()
            return
        }
        if mapped == .incompatibleProtocol || mapped == .malformedPayload || mapped == .projectionInvariant {
            connectionState = .fatalProtocolError
            publish()
            return
        }
        connectionState = .reconnecting
        publish()
        scheduleReconnect()
    }

    private func scheduleReconnect() {
        guard reconnectAttempt < Self.maximumReconnectAttempts, reconnectTask == nil else {
            if reconnectAttempt >= Self.maximumReconnectAttempts { connectionState = .disconnected; publish() }
            return
        }
        reconnectAttempt += 1
        let exponent = min(reconnectAttempt - 1, 16)
        let maximum = min(Self.reconnectCapMilliseconds, Self.reconnectBaseMilliseconds << exponent)
        let delay = jitter.milliseconds(upperBound: maximum)
        let authority = currentAuthority()
        reconnectTaskGeneration = authority.generation
        reconnectTask = Task {
            do {
                try await self.sleeper.sleep(for: .milliseconds(delay))
                try self.requireCurrent(authority)
                try await self.reconnectFromCursor()
                self.clearReconnectTask(expectedGeneration: authority.generation)
            } catch is CancellationError {
                self.clearReconnectTask(expectedGeneration: authority.generation)
            } catch SessionOperationError.superseded {
                self.clearReconnectTask(expectedGeneration: authority.generation)
            }
            catch {
                guard self.clearReconnectTask(expectedGeneration: authority.generation) else { return }
                await self.reconnectFailed(error)
            }
        }
    }

    @discardableResult
    private func clearReconnectTask(expectedGeneration: UInt64) -> Bool {
        guard reconnectTaskGeneration == expectedGeneration else { return false }
        reconnectTask = nil
        reconnectTaskGeneration = nil
        return true
    }

    private func reconnectFailed(_ error: Error) async {
        if error is SessionOperationError { return }
        let mapped = mapTransportError(error)
        lastError = mapped
        if mapped == .authentication {
            connectionState = .authenticationFailed
        } else if mapped == .incompatibleProtocol || mapped == .configurationMismatch {
            connectionState = .fatalProtocolError
        } else {
            connectionState = .reconnecting
            scheduleReconnect()
        }
        publish()
    }

    private func currentAuthority() -> OperationAuthority {
        OperationAuthority(generation: generation, profile: profile)
    }

    private func isCurrent(_ authority: OperationAuthority) -> Bool {
        !isShuttingDown && generation == authority.generation && profile == authority.profile
    }

    private func requireCurrent(_ authority: OperationAuthority) throws {
        guard isCurrent(authority) else {
            throw SessionOperationError.superseded
        }
    }

    private func invalidateConnectionAuthority(
        cancelReconnect: Bool = false
    ) -> InvalidatedOperations {
        generation &+= 1
        let invalidated = InvalidatedOperations(
            connection: currentConnection,
            receiveTask: receiveTask,
            pingTask: pingTask,
            reconnectTask: cancelReconnect ? reconnectTask : nil)
        invalidated.receiveTask?.cancel()
        invalidated.pingTask?.cancel()
        invalidated.reconnectTask?.cancel()
        currentConnection = nil
        receiveTask = nil
        pingTask = nil
        if cancelReconnect {
            reconnectTask = nil
            reconnectTaskGeneration = nil
        }
        drafts.clearAll()
        return invalidated
    }

    private func closeInvalidatedConnection(_ invalidated: InvalidatedOperations) async {
        if let connection = invalidated.connection { await connection.close() }
    }

    private func beginConnectionOperation() async throws -> OperationAuthority {
        let invalidated = invalidateConnectionAuthority()
        let authority = currentAuthority()
        await closeInvalidatedConnection(invalidated)
        try requireCurrent(authority)
        return authority
    }

    private func readCredential() async throws -> BearerToken {
        try await readCredential(for: currentAuthority())
    }

    private func readCredential(for authority: OperationAuthority) async throws -> BearerToken {
        do {
            let token = try await credentialStore.read(profileID: authority.profile.profileID)
            try requireCurrent(authority)
            credentialStatus = .installed
            return token
        } catch ClientError.credentialMalformed {
            try requireCurrent(authority)
            credentialStatus = .malformed
            throw ClientError.credentialMalformed
        } catch ClientError.credentialRequired {
            try requireCurrent(authority)
            credentialStatus = .required
            throw ClientError.credentialRequired
        } catch {
            try requireCurrent(authority)
            throw error
        }
    }

    private func makePending(kind: CommandKind, id: ProtocolID, path: String, body: Data) -> PendingCommand {
        PendingCommand(
            kind: kind, commandID: id, path: path, body: body, idempotencyKey: id.rawValue,
            materialHash: CommandMaterial.hash(
                method: "POST", path: path, idempotencyKey: id.rawValue, body: body),
            profileID: profile.profileID, craxiiID: projection.craxii?.craxiiID,
            credentialGeneration: profile.credentialGeneration)
    }

    private func appendPending(_ command: PendingCommand) async throws {
        guard persisted.outbox.count < 128 else { throw ClientError.outboxCorrupt }
        persisted.outbox.append(command)
        try await localStore.save(persisted)
        publish()
    }

    private func sendMessageCommand(_ id: ProtocolID) async throws -> MessageReceipt {
        let response = try await sendPending(id)
        let receipt: MessageReceipt
        do { receipt = try decodeHTTP(response, success: [200, 202]) }
        catch {
            try await stopAutomaticResend(id)
            throw error
        }
        try await removePending(id)
        return receipt
    }

    private func sendCancellationCommand(_ id: ProtocolID) async throws -> CancellationReceipt {
        let response = try await sendPending(id)
        let receipt: CancellationReceipt
        do { receipt = try decodeHTTP(response, success: [200, 202]) }
        catch {
            try await stopAutomaticResend(id)
            throw error
        }
        try await removePending(id)
        return receipt
    }

    private func sendPending(_ id: ProtocolID) async throws -> HTTPResponse {
        guard let initial = persisted.outbox.first(where: { $0.commandID == id }) else {
            throw ClientError.outboxCorrupt
        }
        guard initial.disposition == .sendable else { throw ClientError.configurationMismatch }
        let baseURL = try EndpointPolicy.validate(profile.endpoint, allowDebugLocalhostHTTP: allowDebugLocalhostHTTP)
        let token = try await readCredential()
        while let index = persisted.outbox.firstIndex(where: { $0.commandID == id }) {
            let pending = persisted.outbox[index]
            try validatePendingForSend(pending)
            guard pending.attempts < Self.maximumCommandAttempts else { throw ClientError.serverUnavailable }
            persisted.outbox[index].attempts += 1
            let attempt = persisted.outbox[index].attempts
            try await localStore.save(persisted)
            let response: HTTPResponse
            do {
                response = try await http.execute(HTTPRequest(
                    url: try EndpointPolicy.endpoint(baseURL: baseURL, exactPath: pending.path),
                    method: "POST", authorization: token, idempotencyKey: pending.idempotencyKey,
                    body: pending.body, timeout: 15,
                    maximumResponseBytes: ProtocolConstants.maximumCommandResponseBytes))
            } catch let error as ClientError {
                if isAmbiguousRetryable(error), attempt < Self.maximumCommandAttempts {
                    try await sleepForCommandRetry(attempt: attempt)
                    continue
                }
                throw error
            } catch {
                if attempt < Self.maximumCommandAttempts {
                    try await sleepForCommandRetry(attempt: attempt)
                    continue
                }
                throw mapTransportError(error)
            }
            if [502, 503, 504].contains(response.statusCode) {
                if attempt < Self.maximumCommandAttempts {
                    try await sleepForCommandRetry(attempt: attempt)
                    continue
                }
                throw ClientError.serverUnavailable
            }
            if ![200, 202].contains(response.statusCode) {
                let failure: BackendErrorEnvelope.Detail
                do { failure = try decodeBackendFailure(response) }
                catch {
                    try await stopAutomaticResend(id)
                    throw error
                }
                if response.statusCode != 409, response.statusCode != 401,
                   failure.retryable, attempt < Self.maximumCommandAttempts {
                    try await sleepForCommandRetry(attempt: attempt)
                    continue
                }
                try await stopAutomaticResend(id)
                throw mapBackendFailure(failure, status: response.statusCode)
            }
            return response
        }
        throw ClientError.outboxCorrupt
    }

    private func sleepForCommandRetry(attempt: Int) async throws {
        let cap = min(UInt64(4_000), UInt64(500) << min(attempt - 1, 4))
        try await sleeper.sleep(for: .milliseconds(jitter.milliseconds(upperBound: cap)))
    }

    private func validatePendingForSend(_ pending: PendingCommand) throws {
        guard pending.profileID == profile.profileID,
              pending.craxiiID == projection.craxii?.craxiiID,
              pending.credentialGeneration == profile.credentialGeneration,
              pending.idempotencyKey == pending.commandID.rawValue,
              CommandMaterial.hash(
                method: "POST", path: pending.path, idempotencyKey: pending.idempotencyKey,
                body: pending.body) == pending.materialHash else { throw ClientError.outboxCorrupt }
    }

    private func reconcileOutbox() throws {
        for index in persisted.outbox.indices {
            let pending = persisted.outbox[index]
            guard pending.profileID == profile.profileID,
                  pending.credentialGeneration == profile.credentialGeneration,
                  pending.craxiiID == projection.craxii?.craxiiID else {
                persisted.outbox[index].disposition = .reconciliationOnly
                continue
            }
            switch pending.kind {
            case .message:
                let request: MessageRequest
                do { request = try decoder.decode(MessageRequest.self, from: pending.body) }
                catch { throw ClientError.outboxCorrupt }
                if let server = projection.messages.first(where: {
                    $0.clientMessageID == request.clientMessageID
                }) {
                    guard server.content == request.content else { throw ClientError.outboxCorrupt }
                    persisted.outbox[index].disposition = .reconciliationOnly
                }
            case .cancellation:
                let workText = pending.path.dropFirst("/v1/work-items/".count).dropLast("/cancel".count)
                guard let workID = ProtocolID(rawValue: String(workText)) else { throw ClientError.outboxCorrupt }
                if let work = projection.works.first(where: { $0.workID == workID }),
                   work.state == .cancelRequested || work.state.isTerminal {
                    persisted.outbox[index].disposition = .reconciliationOnly
                }
            }
        }
        persisted.outbox.removeAll { pending in
            if pending.disposition != .reconciliationOnly { return false }
            guard pending.profileID == profile.profileID,
                  pending.credentialGeneration == profile.credentialGeneration,
                  pending.craxiiID == projection.craxii?.craxiiID else { return false }
            return true
        }
    }

    private func resendReconciledOutbox(authority: OperationAuthority) async throws {
        let pendingIDs = persisted.outbox.filter { $0.disposition == .sendable }.map(\.commandID)
        for id in pendingIDs {
            try requireCurrent(authority)
            guard let kind = persisted.outbox.first(where: { $0.commandID == id })?.kind else { continue }
            do {
                if kind == .message { _ = try await sendMessageCommand(id) }
                else { _ = try await sendCancellationCommand(id) }
                try requireCurrent(authority)
            } catch ClientError.authentication { throw ClientError.authentication }
            catch ClientError.outboxCorrupt { throw ClientError.outboxCorrupt }
            catch SessionOperationError.superseded { throw SessionOperationError.superseded }
            catch { continue }
        }
    }

    private func removePending(_ id: ProtocolID) async throws {
        persisted.outbox.removeAll { $0.commandID == id }
        try await localStore.save(persisted)
        publish()
    }

    private func stopAutomaticResend(_ id: ProtocolID) async throws {
        guard let index = persisted.outbox.firstIndex(where: { $0.commandID == id }) else { return }
        persisted.outbox[index].disposition = .reconciliationOnly
        try await localStore.save(persisted)
        publish()
    }

    private func decodeHTTP<T: Decodable>(_ response: HTTPResponse, success: Set<Int>) throws -> T {
        guard success.contains(response.statusCode) else {
            let failure = try decodeBackendFailure(response)
            throw mapBackendFailure(failure, status: response.statusCode)
        }
        do { return try decoder.decode(T.self, from: response.body) }
        catch ProtocolModelError.incompatibleVersion { throw ClientError.incompatibleProtocol }
        catch { throw ClientError.malformedPayload }
    }

    private func decodeBackendFailure(_ response: HTTPResponse) throws -> BackendErrorEnvelope.Detail {
        if response.statusCode == 401 { throw ClientError.authentication }
        do { return try decoder.decode(BackendErrorEnvelope.self, from: response.body).error }
        catch ProtocolModelError.incompatibleVersion { throw ClientError.incompatibleProtocol }
        catch { throw ClientError.malformedPayload }
    }

    private func mapBackendFailure(_ failure: BackendErrorEnvelope.Detail, status: Int) -> ClientError {
        if status == 401 || failure.code == "authentication_failed" { return .authentication }
        if failure.code == "service_unavailable" || failure.code == "overloaded" { return .serverUnavailable }
        if failure.code == "bootstrap_limit_exceeded" { return .serverNotReady }
        if failure.code == "command_timeout" { return .timeout }
        return .commandRejected(failure.code)
    }

    private func isAmbiguousRetryable(_ error: ClientError) -> Bool {
        switch error {
        case .networkOffline, .timeout, .serverUnavailable: true
        default: false
        }
    }

    private func mapTransportError(_ error: Error) -> ClientError {
        if let client = error as? ClientError { return client }
        if error is CancellationError { return .networkOffline }
        return .serverUnavailable
    }

    private func handleStartFailure(_ error: Error, authority: OperationAuthority) {
        guard isCurrent(authority) else { return }
        let mapped = mapTransportError(error)
        lastError = mapped
        switch mapped {
        case .authentication: connectionState = .authenticationFailed
        case .incompatibleProtocol, .malformedPayload, .projectionInvariant,
             .outboxCorrupt, .configurationMismatch: connectionState = .fatalProtocolError
        default: connectionState = .disconnected
        }
        publish()
    }

    private var terminalEventTypes: Set<String> {
        ["work.completed", "work.cancelled", "work.failed", "work.interrupted"]
    }

    private func makeSnapshot() -> ClientSnapshot {
        ClientSnapshot(
            credentialStatus: credentialStatus, connectionState: connectionState,
            projection: projection, drafts: drafts.drafts.values.sorted { $0.id < $1.id },
            pendingCommandCount: persisted.outbox.count, generation: generation, lastError: lastError)
    }

    private func publish() { snapshotHandler?(makeSnapshot()) }
}
