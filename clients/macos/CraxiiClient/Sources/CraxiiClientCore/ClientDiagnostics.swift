import Foundation
import CraxiiProtocol

/// Closed event vocabulary for client diagnostics. No field can carry message, token, body,
/// endpoint, draft, or arbitrary error text.
public enum ClientDiagnosticKind: String, Sendable {
    case sessionStarted = "session_started"
    case sessionStopped = "session_stopped"
    case profileGenerationChanged = "profile_generation_changed"
    case bootstrapStarted = "bootstrap_started"
    case bootstrapFinished = "bootstrap_finished"
    case reconnectScheduled = "reconnect_scheduled"
    case reconnectFinished = "reconnect_finished"
    case replayStarted = "replay_started"
    case replayFinished = "replay_finished"
    case staleGenerationSuppressed = "stale_generation_suppressed"
    case commandPrepared = "command_prepared"
    case commandPersisted = "command_persisted"
    case commandSent = "command_sent"
    case commandRetried = "command_retried"
    case commandReconciled = "command_reconciled"
    case commandFailed = "command_failed"
    case outboxRecovered = "outbox_recovered"
    case projectionAdvanced = "projection_advanced"
    case fatalConfiguration = "fatal_configuration"
    case fatalProtocol = "fatal_protocol"
}

public enum ClientDiagnosticResult: String, Sendable {
    case started, succeeded, failed, scheduled, suppressed, persisted, accepted, reconciled
}

public enum ClientDiagnosticErrorClass: String, Sendable {
    case credentialRequired = "credential_required"
    case credentialMalformed = "credential_malformed"
    case keychainFailure = "keychain_failure"
    case networkOffline = "network_offline"
    case timeout
    case authentication
    case serverUnavailable = "server_unavailable"
    case serverNotReady = "server_not_ready"
    case incompatibleProtocol = "incompatible_protocol"
    case malformedPayload = "malformed_payload"
    case projectionInvariant = "projection_invariant"
    case commandRejected = "command_rejected"
    case backend
    case cancellationTransportFailure = "cancellation_transport_failure"
    case cacheCorrupt = "cache_corrupt"
    case outboxCorrupt = "outbox_corrupt"
    case configurationMismatch = "configuration_mismatch"
    case superseded
    case unknown
}

public struct ClientDiagnosticEvent: Equatable, Sendable {
    public let kind: ClientDiagnosticKind
    public let result: ClientDiagnosticResult?
    public let errorClass: ClientDiagnosticErrorClass?
    public let profileID: ProtocolID?
    public let generation: UInt64?
    public let projectionRevision: UInt64?
    public let commandKind: CommandKind?
    public let commandID: ProtocolID?
    public let workID: WorkID?
    public let requestID: ProtocolID?
    public let cursorFrom: Cursor?
    public let cursorThrough: Cursor?
    public let count: Int?
    public let attempt: Int?
    public let delayMilliseconds: UInt64?

    public init(
        kind: ClientDiagnosticKind, result: ClientDiagnosticResult? = nil,
        errorClass: ClientDiagnosticErrorClass? = nil, profileID: ProtocolID? = nil,
        generation: UInt64? = nil, projectionRevision: UInt64? = nil,
        commandKind: CommandKind? = nil, commandID: ProtocolID? = nil,
        workID: WorkID? = nil, requestID: ProtocolID? = nil,
        cursorFrom: Cursor? = nil, cursorThrough: Cursor? = nil,
        count: Int? = nil, attempt: Int? = nil, delayMilliseconds: UInt64? = nil
    ) {
        self.kind = kind; self.result = result; self.errorClass = errorClass
        self.profileID = profileID; self.generation = generation
        self.projectionRevision = projectionRevision; self.commandKind = commandKind
        self.commandID = commandID; self.workID = workID; self.requestID = requestID
        self.cursorFrom = cursorFrom; self.cursorThrough = cursorThrough; self.count = count
        self.attempt = attempt; self.delayMilliseconds = delayMilliseconds
    }
}

public protocol ClientDiagnosticRecording: Sendable {
    func record(_ event: ClientDiagnosticEvent)
}

public struct NoopClientDiagnosticRecorder: ClientDiagnosticRecording {
    public init() {}
    public func record(_ event: ClientDiagnosticEvent) {}
}

/// Deterministic typed recorder used by tests; it never renders events to strings.
public final class InMemoryClientDiagnosticRecorder: ClientDiagnosticRecording, @unchecked Sendable {
    private let lock = NSLock()
    private var recorded: [ClientDiagnosticEvent] = []

    public init() {}

    public func record(_ event: ClientDiagnosticEvent) {
        lock.lock()
        defer { lock.unlock() }
        recorded.append(event)
    }

    public func events() -> [ClientDiagnosticEvent] {
        lock.lock()
        defer { lock.unlock() }
        return recorded
    }
}
