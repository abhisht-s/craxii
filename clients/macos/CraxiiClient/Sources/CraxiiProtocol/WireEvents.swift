import Foundation

public enum JSONValue: Codable, Equatable, Sendable {
    case object([String: JSONValue]), array([JSONValue]), string(String), integer(Int64)
    case number(Double), bool(Bool), null

    public init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() { self = .null }
        else if let value = try? container.decode([String: JSONValue].self) { self = .object(value) }
        else if let value = try? container.decode([JSONValue].self) { self = .array(value) }
        else if let value = try? container.decode(Bool.self) { self = .bool(value) }
        else if let value = try? container.decode(Int64.self) { self = .integer(value) }
        else if let value = try? container.decode(Double.self) { self = .number(value) }
        else if let value = try? container.decode(String.self) { self = .string(value) }
        else { throw ProtocolModelError.invalidEnvelope }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case let .object(value): try container.encode(value)
        case let .array(value): try container.encode(value)
        case let .string(value): try container.encode(value)
        case let .integer(value): try container.encode(value)
        case let .number(value): try container.encode(value)
        case let .bool(value): try container.encode(value)
        case .null: try container.encodeNil()
        }
    }
}

public enum DeliveryKind: String, Codable, Sendable { case durable, ephemeral }

public struct DurableEventEnvelope: Codable, Equatable, Sendable {
    public let protocolVersion: ProtocolVersion
    public let deliveryKind: DeliveryKind
    public let eventID: EventID
    public let cursor: Cursor
    public let eventType: String
    public let conversationID: ConversationID?
    public let workID: WorkID?
    public let recordedAt: String
    public let payload: JSONValue
    enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version", deliveryKind = "delivery_kind"
        case eventID = "event_id", cursor, eventType = "event_type"
        case conversationID = "conversation_id", workID = "work_id", recordedAt = "recorded_at", payload
    }
}

public enum DraftDeltaKind: String, Codable, Sendable { case text, refusal }
public enum DraftAbandonReason: String, Codable, Sendable {
    case toolContinuation = "tool_continuation", superseded, cancelled, failed, interrupted
    case deliveryLimit = "delivery_limit"
}

public struct EphemeralDraftEnvelope: Codable, Equatable, Sendable {
    public let protocolVersion: ProtocolVersion
    public let deliveryKind: DeliveryKind
    public let eventID: EventID
    public let cursor: Cursor?
    public let eventType: String
    public let conversationID: ConversationID
    public let workID: WorkID
    public let invocationID: InvocationID
    public let draftID: DraftID
    public let deltaSequence: UInt32?
    public let payload: JSONValue
    enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version", deliveryKind = "delivery_kind"
        case eventID = "event_id", cursor, eventType = "event_type"
        case conversationID = "conversation_id", workID = "work_id"
        case invocationID = "invocation_id", draftID = "draft_id"
        case deltaSequence = "delta_sequence", payload
    }
}

public struct SyncCompleteEnvelope: Codable, Equatable, Sendable {
    public let protocolVersion: ProtocolVersion
    public let deliveryKind: DeliveryKind
    public let eventType: String
    public let throughCursor: Cursor
    enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version", deliveryKind = "delivery_kind"
        case eventType = "event_type", throughCursor = "through_cursor"
    }
}

public enum ServerFrame: Equatable, Sendable {
    case durable(DurableEventEnvelope)
    case draft(EphemeralDraftEnvelope)
    case syncComplete(SyncCompleteEnvelope)

    public static func decode(_ data: Data, decoder: JSONDecoder = JSONDecoder()) throws -> Self {
        guard data.count <= ProtocolConstants.maximumWebSocketFrameBytes else {
            throw ProtocolModelError.invalidEnvelope
        }
        struct Kind: Decodable {
            let protocolVersion: ProtocolVersion
            let deliveryKind: DeliveryKind
            let eventType: String
            enum CodingKeys: String, CodingKey {
                case protocolVersion = "protocol_version", deliveryKind = "delivery_kind"
                case eventType = "event_type"
            }
        }
        let kind = try decoder.decode(Kind.self, from: data)
        if kind.deliveryKind == .durable {
            return .durable(try decoder.decode(DurableEventEnvelope.self, from: data))
        }
        if kind.eventType == "sync.complete" {
            return .syncComplete(try decoder.decode(SyncCompleteEnvelope.self, from: data))
        }
        guard ["assistant.draft_started", "assistant.draft_delta", "assistant.draft_abandoned"]
            .contains(kind.eventType)
        else { throw ProtocolModelError.unknownEventType }
        return .draft(try decoder.decode(EphemeralDraftEnvelope.self, from: data))
    }
}

public extension JSONValue {
    func decode<T: Decodable>(_ type: T.Type, decoder: JSONDecoder = JSONDecoder()) throws -> T {
        try decoder.decode(type, from: JSONEncoder().encode(self))
    }
}

public struct MessageEventPayload: Codable, Equatable, Sendable {
    public let messageID: MessageID
    public let role: MessageRole
    public let content: [ContentBlock]
    public let clientMessageID: ClientMessageID?
    public let workID: WorkID?
    public let committedAt: String
    enum CodingKeys: String, CodingKey {
        case messageID = "message_id", role, content, clientMessageID = "client_message_id"
        case workID = "work_id", committedAt = "committed_at"
    }
}

public struct WorkQueuedPayload: Codable, Equatable, Sendable {
    public let workID: WorkID
    public let conversationWorkOrdinal: Int64
    public let state: WorkState
    public let queuedAt: String
    enum CodingKeys: String, CodingKey {
        case workID = "work_id", conversationWorkOrdinal = "conversation_work_ordinal", state
        case queuedAt = "queued_at"
    }
}

public struct WorkTransitionPayload: Codable, Equatable, Sendable {
    public let state: WorkState
    public let terminalReason: WorkTerminalReason?
    public let transitionedAt: String
    enum CodingKeys: String, CodingKey {
        case state, terminalReason = "terminal_reason", transitionedAt = "transitioned_at"
    }
}

public struct ToolEventPayload: Codable, Equatable, Sendable {
    public let toolExecutionID: ToolExecutionID
    public let status: String
    public let resultClass: String?
    public let outcomeUnknown: Bool?
    public let observedAt: String
    enum CodingKeys: String, CodingKey {
        case toolExecutionID = "tool_execution_id", status, resultClass = "result_class"
        case outcomeUnknown = "outcome_unknown", observedAt = "observed_at"
    }
}

public struct DraftDeltaPayload: Codable, Equatable, Sendable {
    public let kind: DraftDeltaKind
    public let text: String
}

public struct DraftAbandonedPayload: Codable, Equatable, Sendable {
    public let reason: DraftAbandonReason
}
