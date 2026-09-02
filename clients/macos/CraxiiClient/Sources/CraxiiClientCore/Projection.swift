import Foundation
import CraxiiProtocol

public struct ProjectedMessage: Equatable, Sendable, Identifiable {
    public let messageID: MessageID
    public let conversationID: ConversationID
    public let canonicalOrder: Int64
    public let role: MessageRole
    public let content: [ContentBlock]
    public let clientMessageID: ClientMessageID?
    public let workID: WorkID?
    public let committedAt: String
    public var id: String { messageID.rawValue }
}

public struct SafeToolProjection: Equatable, Sendable, Identifiable {
    public let executionID: ToolExecutionID
    public var toolName: String?
    public var status: String
    public var resultClass: String?
    public var requestedAt: String?
    public var startedAt: String?
    public var finishedAt: String?
    public var outcomeUnknown: Bool
    public var id: String { executionID.rawValue }
}

public struct WorkProjection: Equatable, Sendable, Identifiable {
    public let workID: WorkID
    public let conversationID: ConversationID
    public let conversationWorkOrdinal: Int64
    public var state: WorkState
    public var triggerMessageID: MessageID?
    public var createdAt: String?
    public var queuedAt: String?
    public var startedAt: String?
    public var cancelRequestedAt: String?
    public var terminalAt: String?
    public var terminalReason: WorkTerminalReason?
    public var cleanupPending: Bool
    public var tools: [SafeToolProjection]
    public var id: String { workID.rawValue }
}

public struct DraftProjection: Equatable, Sendable, Identifiable {
    public let conversationID: ConversationID
    public let workID: WorkID
    public let invocationID: InvocationID
    public let draftID: DraftID
    public var greatestSequence: UInt32
    public var text: String
    public var refusal: String
    public var id: String { draftID.rawValue }
}

public struct CanonicalProjection: Equatable, Sendable {
    public var craxii: CraxiiProjectionDTO?
    public var primaryConversation: ConversationDTO?
    public var messages: [ProjectedMessage]
    public var works: [WorkProjection]
    public var unresolvedOutcomes: [UnresolvedOutcomeDTO]
    public var lastAppliedCursor: Cursor
    fileprivate var appliedEvents: [Cursor: DurableEventEnvelope]

    public init(
        craxii: CraxiiProjectionDTO? = nil, primaryConversation: ConversationDTO? = nil,
        messages: [ProjectedMessage] = [], works: [WorkProjection] = [],
        unresolvedOutcomes: [UnresolvedOutcomeDTO] = [], lastAppliedCursor: Cursor = .start
    ) {
        self.craxii = craxii; self.primaryConversation = primaryConversation
        self.messages = messages; self.works = works
        self.unresolvedOutcomes = unresolvedOutcomes; self.lastAppliedCursor = lastAppliedCursor
        appliedEvents = [:]
    }

    public static func bootstrap(_ response: BootstrapResponse) throws -> Self {
        guard response.primaryConversation.kind == "primary",
              response.messages.count <= 2_048,
              response.workItems.filter({ $0.state.isTerminal }).count <= 512,
              response.workItems.reduce(0, { $0 + $1.toolSummaries.count }) <= 2_048
        else { throw ClientError.projectionInvariant }

        var priorSequence: Int64 = 0
        var messageIDs = Set<MessageID>()
        var clientIDs = Set<ClientMessageID>()
        var workProducedMessages = Set<WorkID>()
        var messages: [ProjectedMessage] = []
        for message in response.messages {
            guard message.conversationID == response.primaryConversation.conversationID,
                  message.conversationSequence > priorSequence,
                  messageIDs.insert(message.messageID).inserted,
                  message.clientMessageID.map({ clientIDs.insert($0).inserted }) ?? true,
                  message.workID.map({ workProducedMessages.insert($0).inserted }) ?? true
            else { throw ClientError.projectionInvariant }
            priorSequence = message.conversationSequence
            messages.append(ProjectedMessage(
                messageID: message.messageID, conversationID: message.conversationID,
                canonicalOrder: message.conversationSequence, role: message.role,
                content: message.content, clientMessageID: message.clientMessageID,
                workID: message.workID, committedAt: message.committedAt))
        }

        var priorOrdinal: Int64 = 0
        var workIDs = Set<WorkID>()
        var toolIDs = Set<ToolExecutionID>()
        var works: [WorkProjection] = []
        for work in response.workItems {
            guard work.conversationID == response.primaryConversation.conversationID,
                  work.conversationWorkOrdinal > priorOrdinal,
                  workIDs.insert(work.workID).inserted,
                  messageIDs.contains(work.triggerMessageID)
            else { throw ClientError.projectionInvariant }
            priorOrdinal = work.conversationWorkOrdinal
            let tools = try work.toolSummaries.map { tool -> SafeToolProjection in
                guard toolIDs.insert(tool.toolExecutionID).inserted else {
                    throw ClientError.projectionInvariant
                }
                return SafeToolProjection(
                    executionID: tool.toolExecutionID, toolName: tool.toolName,
                    status: tool.status, resultClass: tool.resultClass,
                    requestedAt: tool.requestedAt, startedAt: tool.startedAt,
                    finishedAt: tool.finishedAt, outcomeUnknown: tool.outcomeUnknown)
            }
            works.append(WorkProjection(
                workID: work.workID, conversationID: work.conversationID,
                conversationWorkOrdinal: work.conversationWorkOrdinal, state: work.state,
                triggerMessageID: work.triggerMessageID, createdAt: work.createdAt,
                queuedAt: work.queuedAt, startedAt: work.startedAt,
                cancelRequestedAt: work.cancelRequestedAt, terminalAt: work.terminalAt,
                terminalReason: work.terminalReason, cleanupPending: work.cleanupPending, tools: tools))
        }
        guard response.unresolvedOutcomes.allSatisfy({ workIDs.contains($0.workID) }) else {
            throw ClientError.projectionInvariant
        }
        return Self(
            craxii: response.craxii, primaryConversation: response.primaryConversation,
            messages: messages, works: works, unresolvedOutcomes: response.unresolvedOutcomes,
            lastAppliedCursor: response.snapshotCursor)
    }
}

public struct DurableReducer: Sendable {
    public init() {}

    public func applying(_ event: DurableEventEnvelope, to projection: CanonicalProjection) throws
        -> CanonicalProjection
    {
        guard event.deliveryKind == .durable, event.cursor.rawValue > 0 else {
            throw ClientError.malformedPayload
        }
        if event.cursor <= projection.lastAppliedCursor {
            guard projection.appliedEvents[event.cursor] == event else {
                throw ClientError.projectionInvariant
            }
            return projection
        }

        var result = projection
        try applyKnown(event, to: &result)
        result.lastAppliedCursor = event.cursor
        result.appliedEvents[event.cursor] = event
        return result
    }

    private func applyKnown(_ event: DurableEventEnvelope, to projection: inout CanonicalProjection) throws {
        switch event.eventType {
        case "message.accepted", "assistant.message_committed":
            try applyMessage(event, to: &projection)
        case "work.queued":
            try applyQueued(event, to: &projection)
        case "work.started", "work.waiting_on_model", "work.waiting_on_tool",
             "work.cancel_requested", "work.cancelled", "work.completed", "work.failed",
             "work.interrupted":
            try applyTransition(event, to: &projection)
        case "tool.execution_started", "tool.execution_finished",
             "tool.execution_interrupted_before_dispatch":
            try applyTool(event, to: &projection)
        case "runtime.recovery_performed":
            break
        default:
            throw ProtocolModelError.unknownEventType
        }
    }

    private func applyMessage(_ event: DurableEventEnvelope, to projection: inout CanonicalProjection) throws {
        guard let conversationID = event.conversationID,
              conversationID == projection.primaryConversation?.conversationID else {
            throw ClientError.projectionInvariant
        }
        let payload: MessageEventPayload
        do { payload = try event.payload.decode(MessageEventPayload.self) }
        catch { throw ClientError.malformedPayload }
        let expectedRole: MessageRole = event.eventType == "message.accepted" ? .user : .assistant
        let producedWork = event.eventType == "assistant.message_committed" ? event.workID : nil
        guard payload.role == expectedRole,
              event.eventType != "assistant.message_committed" || payload.workID == producedWork,
              event.eventType != "message.accepted" || payload.clientMessageID != nil else {
            throw ClientError.projectionInvariant
        }
        let order = (projection.messages.map(\.canonicalOrder).max() ?? 0) + 1
        let candidate = ProjectedMessage(
            messageID: payload.messageID, conversationID: conversationID,
            canonicalOrder: order, role: payload.role, content: payload.content,
            clientMessageID: payload.clientMessageID, workID: payload.workID,
            committedAt: payload.committedAt)
        if let existing = projection.messages.first(where: { $0.messageID == candidate.messageID }) {
            guard existing == candidate else { throw ClientError.projectionInvariant }
            return
        }
        if let clientID = candidate.clientMessageID,
           projection.messages.contains(where: { $0.clientMessageID == clientID }) {
            throw ClientError.projectionInvariant
        }
        if let workID = candidate.workID,
           projection.messages.contains(where: { $0.workID == workID }) {
            throw ClientError.projectionInvariant
        }
        projection.messages.append(candidate)
    }

    private func applyQueued(_ event: DurableEventEnvelope, to projection: inout CanonicalProjection) throws {
        guard let conversationID = event.conversationID,
              conversationID == projection.primaryConversation?.conversationID,
              let workID = event.workID else { throw ClientError.projectionInvariant }
        let payload: WorkQueuedPayload
        do { payload = try event.payload.decode(WorkQueuedPayload.self) }
        catch { throw ClientError.malformedPayload }
        guard payload.workID == workID, payload.state == .queued,
              !projection.works.contains(where: { $0.workID == workID }),
              !projection.works.contains(where: {
                  $0.conversationWorkOrdinal == payload.conversationWorkOrdinal
              }) else { throw ClientError.projectionInvariant }
        let trigger = projection.messages.last(where: { $0.role == .user })?.messageID
        projection.works.append(WorkProjection(
            workID: workID, conversationID: conversationID,
            conversationWorkOrdinal: payload.conversationWorkOrdinal, state: .queued,
            triggerMessageID: trigger, createdAt: payload.queuedAt, queuedAt: payload.queuedAt,
            startedAt: nil, cancelRequestedAt: nil, terminalAt: nil, terminalReason: nil,
            cleanupPending: false, tools: []))
        projection.works.sort { $0.conversationWorkOrdinal < $1.conversationWorkOrdinal }
    }

    private func applyTransition(_ event: DurableEventEnvelope, to projection: inout CanonicalProjection) throws {
        guard let workID = event.workID,
              let index = projection.works.firstIndex(where: { $0.workID == workID }) else {
            throw ClientError.projectionInvariant
        }
        let payload: WorkTransitionPayload
        do { payload = try event.payload.decode(WorkTransitionPayload.self) }
        catch { throw ClientError.malformedPayload }
        let expectedState: WorkState
        switch event.eventType {
        case "work.started": expectedState = .running
        case "work.waiting_on_model": expectedState = .waitingOnModel
        case "work.waiting_on_tool": expectedState = .waitingOnTool
        case "work.cancel_requested": expectedState = .cancelRequested
        case "work.cancelled": expectedState = .cancelled
        case "work.completed": expectedState = .completed
        case "work.failed": expectedState = .failed
        case "work.interrupted": expectedState = .interrupted
        default: throw ClientError.projectionInvariant
        }
        var work = projection.works[index]
        guard payload.state == expectedState, !work.state.isTerminal else {
            throw ClientError.projectionInvariant
        }
        work.state = payload.state
        switch payload.state {
        case .running: work.startedAt = work.startedAt ?? payload.transitionedAt
        case .cancelRequested: work.cancelRequestedAt = payload.transitionedAt; work.cleanupPending = true
        case .completed, .failed, .cancelled, .interrupted:
            work.terminalAt = payload.transitionedAt
            work.terminalReason = payload.terminalReason
            work.cancelRequestedAt = nil
            work.cleanupPending = payload.terminalReason == .cleanupUnconfirmed
            projection.unresolvedOutcomes.removeAll {
                $0.workID == workID && $0.toolExecutionID == nil
            }
            if payload.terminalReason == .providerOutcomeUnknown {
                projection.unresolvedOutcomes.append(UnresolvedOutcomeDTO(
                    kind: .providerOutcomeUnknown, workID: workID, toolExecutionID: nil))
            } else if payload.terminalReason == .cleanupUnconfirmed {
                projection.unresolvedOutcomes.append(UnresolvedOutcomeDTO(
                    kind: .cleanupUnconfirmed, workID: workID, toolExecutionID: nil))
            }
        default: break
        }
        projection.works[index] = work
    }

    private func applyTool(_ event: DurableEventEnvelope, to projection: inout CanonicalProjection) throws {
        guard let workID = event.workID,
              let workIndex = projection.works.firstIndex(where: { $0.workID == workID }) else {
            throw ClientError.projectionInvariant
        }
        let payload: ToolEventPayload
        do { payload = try event.payload.decode(ToolEventPayload.self) }
        catch { throw ClientError.malformedPayload }
        var work = projection.works[workIndex]
        if let toolIndex = work.tools.firstIndex(where: { $0.executionID == payload.toolExecutionID }) {
            var tool = work.tools[toolIndex]
            tool.status = payload.status
            tool.resultClass = payload.resultClass
            tool.outcomeUnknown = payload.outcomeUnknown ?? tool.outcomeUnknown
            tool.finishedAt = event.eventType == "tool.execution_started" ? nil : payload.observedAt
            work.tools[toolIndex] = tool
        } else {
            work.tools.append(SafeToolProjection(
                executionID: payload.toolExecutionID, toolName: nil, status: payload.status,
                resultClass: payload.resultClass, requestedAt: nil,
                startedAt: event.eventType == "tool.execution_started" ? payload.observedAt : nil,
                finishedAt: event.eventType == "tool.execution_started" ? nil : payload.observedAt,
                outcomeUnknown: payload.outcomeUnknown ?? false))
        }
        projection.unresolvedOutcomes.removeAll {
            $0.workID == workID && $0.toolExecutionID == payload.toolExecutionID
        }
        if payload.outcomeUnknown == true {
            projection.unresolvedOutcomes.append(UnresolvedOutcomeDTO(
                kind: .toolOutcomeUnknown, workID: workID,
                toolExecutionID: payload.toolExecutionID))
        }
        projection.works[workIndex] = work
    }
}

public struct DraftReducer: Sendable {
    public private(set) var drafts: [DraftID: DraftProjection] = [:]
    private var tombstones: Set<DraftID> = []

    public init() {}

    public mutating func clearAll() { drafts.removeAll(); tombstones.removeAll() }

    public mutating func clear(workID: WorkID) {
        for draft in drafts.values where draft.workID == workID { tombstones.insert(draft.draftID) }
        drafts = drafts.filter { $0.value.workID != workID }
    }

    public mutating func apply(_ event: EphemeralDraftEnvelope, projection: CanonicalProjection) throws {
        guard event.deliveryKind == .ephemeral, event.cursor == nil,
              event.conversationID == projection.primaryConversation?.conversationID,
              let work = projection.works.first(where: { $0.workID == event.workID }),
              !work.state.isTerminal else { return }
        switch event.eventType {
        case "assistant.draft_started":
            guard event.deltaSequence == nil, !tombstones.contains(event.draftID),
                  drafts[event.draftID] == nil else { return }
            drafts[event.draftID] = DraftProjection(
                conversationID: event.conversationID, workID: event.workID,
                invocationID: event.invocationID, draftID: event.draftID,
                greatestSequence: 0, text: "", refusal: "")
        case "assistant.draft_delta":
            guard let sequence = event.deltaSequence, var draft = drafts[event.draftID],
                  draft.conversationID == event.conversationID,
                  draft.workID == event.workID, draft.invocationID == event.invocationID,
                  sequence > draft.greatestSequence else { return }
            let payload: DraftDeltaPayload
            do { payload = try event.payload.decode(DraftDeltaPayload.self) }
            catch { throw ClientError.malformedPayload }
            draft.greatestSequence = sequence
            if payload.kind == .text { draft.text += payload.text } else { draft.refusal += payload.text }
            drafts[event.draftID] = draft
        case "assistant.draft_abandoned":
            _ = try event.payload.decode(DraftAbandonedPayload.self)
            guard let draft = drafts[event.draftID], draft.workID == event.workID,
                  draft.invocationID == event.invocationID else { return }
            drafts.removeValue(forKey: event.draftID)
            tombstones.insert(event.draftID)
        default:
            throw ProtocolModelError.unknownEventType
        }
    }
}
