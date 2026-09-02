import Foundation

public enum ProtocolConstants {
    public static let version = 1
    public static let maximumCursor: Int64 = .max
    public static let maximumWebSocketFrameBytes = 270_336
    public static let maximumBootstrapBytes = 16 * 1_024 * 1_024
    public static let maximumCommandResponseBytes = 1_024 * 1_024
    public static let maximumHealthResponseBytes = 64 * 1_024
}

public enum ProtocolModelError: Error, Equatable, Sendable {
    case incompatibleVersion
    case invalidUUIDv7
    case invalidCursor
    case invalidEnvelope
    case unknownEventType
}

public struct ProtocolVersion: Codable, Hashable, Sendable {
    public let value: Int

    public init() { value = ProtocolConstants.version }

    public init(from decoder: Decoder) throws {
        let decoded = try decoder.singleValueContainer().decode(Int.self)
        guard decoded == ProtocolConstants.version else {
            throw ProtocolModelError.incompatibleVersion
        }
        value = decoded
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(value)
    }
}

public struct ProtocolID: RawRepresentable, Codable, Hashable, Comparable, Sendable,
    CustomStringConvertible
{
    public let rawValue: String

    public init?(rawValue: String) {
        guard Self.isCanonicalUUIDv7(rawValue) else { return nil }
        self.rawValue = rawValue
    }

    public init(validating rawValue: String) throws {
        guard Self.isCanonicalUUIDv7(rawValue) else {
            throw ProtocolModelError.invalidUUIDv7
        }
        self.rawValue = rawValue
    }

    public init(from decoder: Decoder) throws {
        try self.init(validating: decoder.singleValueContainer().decode(String.self))
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(rawValue)
    }

    public var description: String { rawValue }

    public static func < (lhs: Self, rhs: Self) -> Bool { lhs.rawValue < rhs.rawValue }

    public static func isCanonicalUUIDv7(_ value: String) -> Bool {
        guard value.count == 36, value == value.lowercased(), UUID(uuidString: value) != nil else {
            return false
        }
        let bytes = Array(value.utf8)
        guard bytes[8] == 45, bytes[13] == 45, bytes[18] == 45, bytes[23] == 45,
              bytes[14] == Character("7").asciiValue,
              ["8", "9", "a", "b"].contains(String(UnicodeScalar(bytes[19])))
        else { return false }
        return bytes.enumerated().allSatisfy { index, byte in
            [8, 13, 18, 23].contains(index) || (48...57).contains(byte) || (97...102).contains(byte)
        }
    }
}

public typealias CraxiiID = ProtocolID
public typealias ConversationID = ProtocolID
public typealias MessageID = ProtocolID
public typealias ClientMessageID = ProtocolID
public typealias WorkID = ProtocolID
public typealias ClientCommandID = ProtocolID
public typealias EventID = ProtocolID
public typealias InvocationID = ProtocolID
public typealias DraftID = ProtocolID
public typealias ToolExecutionID = ProtocolID

public struct Cursor: RawRepresentable, Codable, Hashable, Comparable, Sendable {
    public let rawValue: Int64

    public init?(rawValue: Int64) {
        guard rawValue >= 0 else { return nil }
        self.rawValue = rawValue
    }

    public init(validating value: Int64) throws {
        guard value >= 0 else { throw ProtocolModelError.invalidCursor }
        rawValue = value
    }

    public init(from decoder: Decoder) throws {
        try self.init(validating: decoder.singleValueContainer().decode(Int64.self))
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(rawValue)
    }

    public static func < (lhs: Self, rhs: Self) -> Bool { lhs.rawValue < rhs.rawValue }
    public static let start = Cursor(rawValue: 0)!
}

public enum ContentType: String, Codable, Sendable { case text }
public enum MessageRole: String, Codable, Sendable { case user, assistant, system }
public enum ConversationLifecycle: String, Codable, Sendable { case active }
public enum WorkState: String, Codable, Sendable {
    case queued, running, waitingOnModel = "waiting_on_model", waitingOnTool = "waiting_on_tool"
    case cancelRequested = "cancel_requested", completed, failed, cancelled, interrupted

    public var isTerminal: Bool {
        switch self {
        case .completed, .failed, .cancelled, .interrupted: true
        default: false
        }
    }
}

public enum WorkTerminalReason: String, Codable, Sendable {
    case answered, refused, definiteNormalizedError = "definite_normalized_error"
    case providerExhausted = "provider_exhausted", invalidModelOutput = "invalid_model_output"
    case lifecycleLimit = "lifecycle_limit", userRequest = "user_request"
    case gracefulShutdown = "graceful_shutdown", runtimeOwnershipLost = "runtime_ownership_lost"
    case providerOutcomeUnknown = "provider_outcome_unknown"
    case toolInterruptedBeforeDispatch = "tool_interrupted_before_dispatch"
    case toolOutcomeUnknown = "tool_outcome_unknown", cleanupUnconfirmed = "cleanup_unconfirmed"
}

public enum UnresolvedOutcomeKind: String, Codable, Sendable {
    case providerOutcomeUnknown = "provider_outcome_unknown"
    case toolOutcomeUnknown = "tool_outcome_unknown"
    case cleanupUnconfirmed = "cleanup_unconfirmed"
}

public struct ContentBlock: Codable, Equatable, Sendable {
    public let type: ContentType
    public let text: String

    public init(type: ContentType = .text, text: String) {
        self.type = type
        self.text = text
    }
}

public struct MessageRequest: Codable, Equatable, Sendable {
    public let protocolVersion: ProtocolVersion
    public let clientMessageID: ClientMessageID
    public let content: [ContentBlock]

    public init(clientMessageID: ClientMessageID, content: [ContentBlock]) {
        protocolVersion = ProtocolVersion()
        self.clientMessageID = clientMessageID
        self.content = content
    }

    enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version", clientMessageID = "client_message_id", content
    }
}

public struct CancellationRequest: Codable, Equatable, Sendable {
    public let protocolVersion: ProtocolVersion
    public let clientCommandID: ClientCommandID

    public init(clientCommandID: ClientCommandID) {
        protocolVersion = ProtocolVersion()
        self.clientCommandID = clientCommandID
    }

    enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version", clientCommandID = "client_command_id"
    }
}

public struct MessageReceipt: Codable, Equatable, Sendable {
    public let protocolVersion: ProtocolVersion
    public let messageID: MessageID
    public let workID: WorkID
    public let workState: WorkState
    public let conversationWorkOrdinal: Int64
    public let committedCursor: Cursor
    public let duplicate: Bool

    enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version", messageID = "message_id", workID = "work_id"
        case workState = "work_state", conversationWorkOrdinal = "conversation_work_ordinal"
        case committedCursor = "committed_cursor", duplicate
    }
}

public struct CancellationReceipt: Codable, Equatable, Sendable {
    public let protocolVersion: ProtocolVersion
    public let workID: WorkID
    public let workState: WorkState
    public let committedCursor: Cursor
    public let duplicate: Bool
    public let cleanupPending: Bool

    enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version", workID = "work_id", workState = "work_state"
        case committedCursor = "committed_cursor", duplicate, cleanupPending = "cleanup_pending"
    }
}

public struct BackendErrorEnvelope: Codable, Equatable, Sendable {
    public struct Detail: Codable, Equatable, Sendable {
        public let code: String
        public let message: String
        public let retryable: Bool
        public let requestID: ProtocolID
        enum CodingKeys: String, CodingKey { case code, message, retryable, requestID = "request_id" }
    }
    public let protocolVersion: ProtocolVersion
    public let error: Detail
    enum CodingKeys: String, CodingKey { case protocolVersion = "protocol_version", error }
}

public enum HealthStatus: String, Codable, Sendable {
    case live, ready, liveUnready = "live_unready", draining, fatal
}

public struct HealthResponse: Codable, Equatable, Sendable {
    public let protocolVersion: ProtocolVersion
    public let status: HealthStatus
    enum CodingKeys: String, CodingKey { case protocolVersion = "protocol_version", status }
}

public struct CraxiiProjectionDTO: Codable, Equatable, Sendable {
    public let craxiiID: CraxiiID
    public let displayName: String
    public let ownerLabel: String
    enum CodingKeys: String, CodingKey {
        case craxiiID = "craxii_id", displayName = "display_name", ownerLabel = "owner_label"
    }
}

public struct ConversationDTO: Codable, Equatable, Sendable {
    public let conversationID: ConversationID
    public let kind: String
    public let lifecycle: ConversationLifecycle
    public let createdAt: String
    enum CodingKeys: String, CodingKey {
        case conversationID = "conversation_id", kind, lifecycle, createdAt = "created_at"
    }
}

public struct MessageDTO: Codable, Equatable, Sendable {
    public let messageID: MessageID
    public let conversationID: ConversationID
    public let conversationSequence: Int64
    public let role: MessageRole
    public let content: [ContentBlock]
    public let clientMessageID: ClientMessageID?
    public let workID: WorkID?
    public let committedAt: String
    enum CodingKeys: String, CodingKey {
        case messageID = "message_id", conversationID = "conversation_id"
        case conversationSequence = "conversation_sequence", role, content
        case clientMessageID = "client_message_id", workID = "work_id", committedAt = "committed_at"
    }
}

public struct ToolSummaryDTO: Codable, Equatable, Sendable {
    public let toolExecutionID: ToolExecutionID
    public let toolName: String
    public let status: String
    public let resultClass: String?
    public let requestedAt: String
    public let startedAt: String?
    public let finishedAt: String?
    public let outcomeUnknown: Bool
    enum CodingKeys: String, CodingKey {
        case toolExecutionID = "tool_execution_id", toolName = "tool_name", status
        case resultClass = "result_class", requestedAt = "requested_at", startedAt = "started_at"
        case finishedAt = "finished_at", outcomeUnknown = "outcome_unknown"
    }
}

public struct WorkItemDTO: Codable, Equatable, Sendable {
    public let workID: WorkID
    public let conversationID: ConversationID
    public let conversationWorkOrdinal: Int64
    public let state: WorkState
    public let triggerMessageID: MessageID
    public let createdAt: String
    public let queuedAt: String
    public let startedAt: String?
    public let cancelRequestedAt: String?
    public let terminalAt: String?
    public let terminalReason: WorkTerminalReason?
    public let cleanupPending: Bool
    public let toolSummaries: [ToolSummaryDTO]
    enum CodingKeys: String, CodingKey {
        case workID = "work_id", conversationID = "conversation_id"
        case conversationWorkOrdinal = "conversation_work_ordinal", state
        case triggerMessageID = "trigger_message_id", createdAt = "created_at", queuedAt = "queued_at"
        case startedAt = "started_at", cancelRequestedAt = "cancel_requested_at"
        case terminalAt = "terminal_at", terminalReason = "terminal_reason"
        case cleanupPending = "cleanup_pending", toolSummaries = "tool_summaries"
    }
}

public struct UnresolvedOutcomeDTO: Codable, Equatable, Sendable {
    public let kind: UnresolvedOutcomeKind
    public let workID: WorkID
    public let toolExecutionID: ToolExecutionID?

    public init(
        kind: UnresolvedOutcomeKind, workID: WorkID, toolExecutionID: ToolExecutionID?
    ) {
        self.kind = kind
        self.workID = workID
        self.toolExecutionID = toolExecutionID
    }

    enum CodingKeys: String, CodingKey {
        case kind, workID = "work_id", toolExecutionID = "tool_execution_id"
    }
}

public struct BootstrapResponse: Codable, Equatable, Sendable {
    public let protocolVersion: ProtocolVersion
    public let snapshotCursor: Cursor
    public let craxii: CraxiiProjectionDTO
    public let primaryConversation: ConversationDTO
    public let messages: [MessageDTO]
    public let workItems: [WorkItemDTO]
    public let unresolvedOutcomes: [UnresolvedOutcomeDTO]
    enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version", snapshotCursor = "snapshot_cursor", craxii
        case primaryConversation = "primary_conversation", messages, workItems = "work_items"
        case unresolvedOutcomes = "unresolved_outcomes"
    }
}
