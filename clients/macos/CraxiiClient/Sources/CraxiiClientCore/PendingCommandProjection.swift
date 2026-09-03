import Foundation
import CraxiiProtocol

public struct PreparedMessageCommand: Equatable, Sendable, Identifiable {
    public let clientMessageID: ClientMessageID
    public var id: String { clientMessageID.rawValue }

    public init(clientMessageID: ClientMessageID) {
        self.clientMessageID = clientMessageID
    }
}

public struct PreparedCancellationCommand: Equatable, Sendable, Identifiable {
    public let clientCommandID: ClientCommandID
    public let workID: WorkID
    public var id: String { clientCommandID.rawValue }

    public init(clientCommandID: ClientCommandID, workID: WorkID) {
        self.clientCommandID = clientCommandID
        self.workID = workID
    }
}

public enum PendingCommandDeliveryState: String, Equatable, Sendable {
    case notSent
    case sending
    case waitingForConnection
    case deliveryNotConfirmed
    case acceptedAwaitingProjection
}

/// A deliberately safe view of locally owned command presentation state.
/// It never exposes credentials, request bytes, idempotency material, or hashes.
public struct PendingCommandProjection: Equatable, Sendable, Identifiable {
    public let commandID: ProtocolID
    public let kind: CommandKind
    public let conversationID: ConversationID?
    public let workID: WorkID?
    public let clientMessageID: ClientMessageID?
    public let visibleMessageText: String?
    public let deliveryState: PendingCommandDeliveryState
    public var id: String { commandID.rawValue }

    public init(
        commandID: ProtocolID,
        kind: CommandKind,
        conversationID: ConversationID?,
        workID: WorkID?,
        clientMessageID: ClientMessageID?,
        visibleMessageText: String?,
        deliveryState: PendingCommandDeliveryState
    ) {
        self.commandID = commandID
        self.kind = kind
        self.conversationID = conversationID
        self.workID = workID
        self.clientMessageID = clientMessageID
        self.visibleMessageText = visibleMessageText
        self.deliveryState = deliveryState
    }
}
