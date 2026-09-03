import Foundation
import CraxiiProtocol
import CraxiiClientCore

public enum ConversationGate: Equatable, Sendable {
    case setup
    case connecting
    case usable
    case credentialRepair
    case configurationMismatch
    case fatalProtocol
}

public enum ConnectionBannerKind: Equatable, Sendable {
    case refreshing
    case reconnecting
    case offline
    case blocking
}

public struct ConnectionBanner: Equatable, Sendable {
    public let kind: ConnectionBannerKind
    public let title: String
    public let detail: String?
    public let offersRetry: Bool

    public init(kind: ConnectionBannerKind, title: String, detail: String?, offersRetry: Bool) {
        self.kind = kind
        self.title = title
        self.detail = detail
        self.offersRetry = offersRetry
    }
}

public enum TranscriptRowKind: Equatable, Sendable {
    case canonical
    case optimistic(PendingCommandDeliveryState)
    case draft
}

public struct TranscriptRow: Equatable, Sendable, Identifiable {
    public let id: String
    public let kind: TranscriptRowKind
    public let role: MessageRole
    public let text: String
    public let refusal: String
    public let status: String?
    public let canonicalMessageID: MessageID?
    public let clientMessageID: ClientMessageID?
    public let workID: WorkID?

    public var isCanonical: Bool {
        if case .canonical = kind { return true }
        return false
    }
}

public struct ToolActivityPresentation: Equatable, Sendable, Identifiable {
    public let id: String
    public let title: String
    public let detail: String?
    public let isUnknownOutcome: Bool
}

public enum WorkTone: Equatable, Sendable {
    case neutral
    case active
    case success
    case warning
    case failure
}

public struct WorkPresentation: Equatable, Sendable, Identifiable {
    public let id: String
    public let workID: WorkID
    public let ordinal: Int64
    public let title: String
    public let detail: String?
    public let tone: WorkTone
    public let canCancel: Bool
    public let cancellationStatus: String?
    public let tools: [ToolActivityPresentation]
}

public struct ErrorPresentation: Equatable, Sendable {
    public let title: String
    public let message: String
    public let code: String?
    public let requestID: String?
    public let retryable: Bool

    public init(
        title: String, message: String, code: String? = nil,
        requestID: String? = nil, retryable: Bool = false
    ) {
        self.title = title
        self.message = message
        self.code = code
        self.requestID = requestID
        self.retryable = retryable
    }
}

public struct ConversationPresentation: Equatable, Sendable {
    public let gate: ConversationGate
    public let craxiiName: String
    public let hasUsableProjection: Bool
    public let mutationsAllowed: Bool
    public let banner: ConnectionBanner?
    public let transcript: [TranscriptRow]
    public let works: [WorkPresentation]
    public let error: ErrorPresentation?
    public let presentationRevision: UInt64
    public let cursor: Int64

    public var activityFingerprint: String {
        let rows = transcript.map { "\($0.id):\($0.text.utf8.count):\($0.refusal.utf8.count):\($0.status ?? "")" }
        let work = works.map { "\($0.id):\($0.title):\($0.detail ?? ""): \($0.cancellationStatus ?? "")" }
        return (rows + work).joined(separator: "|")
    }
}

public enum ConversationPresenter {
    public static func present(
        snapshot: ClientSnapshot,
        endpointConfigured: Bool = true,
        localError: ClientError? = nil
    ) -> ConversationPresentation {
        let usable = snapshot.projection.primaryConversation != nil
            && snapshot.projection.craxii != nil
        let configurationMismatch = localError == .configurationMismatch
            || snapshot.lastError == .configurationMismatch
        let gate: ConversationGate
        if snapshot.connectionState == .authenticationFailed {
            gate = .credentialRepair
        } else if configurationMismatch {
            gate = .configurationMismatch
        } else if snapshot.connectionState == .fatalProtocolError {
            gate = .fatalProtocol
        } else if !endpointConfigured || snapshot.credentialStatus == .required
            || snapshot.credentialStatus == .malformed {
            gate = .setup
        } else if !usable {
            gate = .connecting
        } else {
            gate = .usable
        }

        let blocked = gate == .setup || gate == .connecting || gate == .credentialRepair
            || gate == .configurationMismatch || gate == .fatalProtocol
        let banner = connectionBanner(snapshot: snapshot, usable: usable, gate: gate)
        let transcript = transcriptRows(snapshot: snapshot)
        let works = workRows(snapshot: snapshot)
        return ConversationPresentation(
            gate: gate,
            craxiiName: snapshot.projection.craxii?.displayName ?? "Craxii",
            hasUsableProjection: usable,
            mutationsAllowed: usable && !blocked,
            banner: banner,
            transcript: transcript,
            works: works,
            error: errorPresentation(
                localError ?? snapshot.lastError,
                backend: snapshot.lastBackendError),
            presentationRevision: snapshot.presentationRevision,
            cursor: snapshot.projection.lastAppliedCursor.rawValue)
    }

    private static func connectionBanner(
        snapshot: ClientSnapshot, usable: Bool, gate: ConversationGate
    ) -> ConnectionBanner? {
        switch gate {
        case .setup where usable:
            return ConnectionBanner(
                kind: .blocking, title: "Device setup required",
                detail: "Open settings to repair the endpoint or credential.", offersRetry: false)
        case .credentialRepair:
            return ConnectionBanner(
                kind: .blocking, title: "Credential needs attention",
                detail: "Update the device credential to reconnect.", offersRetry: false)
        case .configurationMismatch:
            return ConnectionBanner(
                kind: .blocking, title: "Endpoint does not match this Craxii",
                detail: "Review the endpoint or reset disposable client state.", offersRetry: false)
        case .fatalProtocol:
            return ConnectionBanner(
                kind: .blocking, title: "Craxii cannot safely sync",
                detail: "The transcript remains selectable, but sending is disabled.", offersRetry: false)
        default: break
        }
        switch snapshot.connectionState {
        case .bootstrapping where usable, .replaying where usable:
            return ConnectionBanner(
                kind: .refreshing, title: "Refreshing Craxii", detail: nil, offersRetry: false)
        case .bootstrapping, .replaying:
            return ConnectionBanner(
                kind: .refreshing, title: "Connecting to Craxii", detail: nil, offersRetry: false)
        case .reconnecting:
            return ConnectionBanner(
                kind: .reconnecting, title: "Reconnecting…",
                detail: "Committed messages and work are still safe.", offersRetry: false)
        case .disconnected where usable:
            return ConnectionBanner(
                kind: .offline, title: "Offline",
                detail: "New commands can be saved locally; delivery may wait for connection.",
                offersRetry: true)
        case .disconnected where snapshot.credentialStatus == .installed:
            return ConnectionBanner(
                kind: .offline, title: "Unable to connect to Craxii",
                detail: "Check the connection and try again.", offersRetry: true)
        default:
            return nil
        }
    }

    private static func transcriptRows(snapshot: ClientSnapshot) -> [TranscriptRow] {
        let canonicalClientIDs = Set(snapshot.projection.messages.compactMap(\.clientMessageID))
        var rows = snapshot.projection.messages.sorted { $0.canonicalOrder < $1.canonicalOrder }.map {
            TranscriptRow(
                id: $0.messageID.rawValue, kind: .canonical, role: $0.role,
                text: $0.content.map(\.text).joined(separator: "\n"), refusal: "", status: nil,
                canonicalMessageID: $0.messageID, clientMessageID: $0.clientMessageID,
                workID: $0.workID)
        }
        rows += snapshot.pendingCommands.compactMap { command -> TranscriptRow? in
            guard command.kind == .message,
                  let clientID = command.clientMessageID,
                  !canonicalClientIDs.contains(clientID),
                  let text = command.visibleMessageText else { return nil }
            return TranscriptRow(
                id: "local-\(clientID.rawValue)", kind: .optimistic(command.deliveryState),
                role: .user, text: text, refusal: "",
                status: messageDeliveryLabel(command.deliveryState), canonicalMessageID: nil,
                clientMessageID: clientID, workID: command.workID)
        }
        rows += snapshot.drafts.map {
            TranscriptRow(
                id: $0.draftID.rawValue, kind: .draft, role: .assistant,
                text: $0.text, refusal: $0.refusal,
                status: $0.text.isEmpty && $0.refusal.isEmpty
                    ? "Craxii is responding" : "Draft — not yet committed",
                canonicalMessageID: nil, clientMessageID: nil, workID: $0.workID)
        }
        return rows
    }

    private static func messageDeliveryLabel(_ state: PendingCommandDeliveryState) -> String {
        switch state {
        case .sending: "Sending"
        case .waitingForConnection: "Waiting for connection"
        case .deliveryNotConfirmed: "Delivery not confirmed"
        case .notSent: "Not sent"
        case .acceptedAwaitingProjection: "Accepted — syncing"
        }
    }

    private static func workRows(snapshot: ClientSnapshot) -> [WorkPresentation] {
        var cancellations: [WorkID: PendingCommandProjection] = [:]
        for command in snapshot.pendingCommands where command.kind == .cancellation {
            if let workID = command.workID { cancellations[workID] = command }
        }
        let unresolved = Set(snapshot.projection.unresolvedOutcomes.map(\.workID))
        return snapshot.projection.works.sorted {
            $0.conversationWorkOrdinal < $1.conversationWorkOrdinal
        }.map { work in
            let cancellation = work.state.isTerminal || work.state == .cancelRequested
                ? nil : cancellations[work.workID]
            let state = workState(work, unresolved: unresolved.contains(work.workID))
            let canCancelState: Bool
            switch work.state {
            case .queued, .running, .waitingOnModel, .waitingOnTool: canCancelState = true
            default: canCancelState = false
            }
            return WorkPresentation(
                id: work.workID.rawValue, workID: work.workID,
                ordinal: work.conversationWorkOrdinal, title: state.title,
                detail: state.detail, tone: state.tone,
                canCancel: canCancelState && cancellation == nil,
                cancellationStatus: cancellation.map { cancellationLabel($0.deliveryState) },
                tools: work.tools.map(toolPresentation))
        }
    }

    private static func workState(
        _ work: WorkProjection, unresolved: Bool
    ) -> (title: String, detail: String?, tone: WorkTone) {
        if unresolved {
            return ("Outcome unknown", "Craxii cannot honestly confirm the final outcome.", .warning)
        }
        switch work.state {
        case .queued:
            return ("Queued", "This message is separate work and will run in order.", .neutral)
        case .running:
            return ("Working", nil, .active)
        case .waitingOnModel:
            return ("Waiting on model", nil, .active)
        case .waitingOnTool:
            let name = work.tools.last(where: { $0.finishedAt == nil })?.toolName
            return (name.map { "Using \($0)" } ?? "Using a tool", nil, .active)
        case .cancelRequested:
            return ("Cancelling", "Cancellation is requested; cleanup may still be in progress.", .warning)
        case .completed:
            return ("Completed", nil, .success)
        case .cancelled:
            return ("Cancelled", nil, .neutral)
        case .failed:
            if work.terminalReason == .lifecycleLimit {
                return ("Craxii reached a work limit", nil, .failure)
            }
            if work.terminalReason == .refused {
                return ("Craxii declined this request", nil, .warning)
            }
            return ("Work failed", terminalReasonDetail(work.terminalReason), .failure)
        case .interrupted:
            return ("Work interrupted", terminalReasonDetail(work.terminalReason), .warning)
        }
    }

    private static func terminalReasonDetail(_ reason: WorkTerminalReason?) -> String? {
        switch reason {
        case .providerOutcomeUnknown, .toolOutcomeUnknown, .cleanupUnconfirmed:
            "The outcome could not be confirmed."
        case .gracefulShutdown:
            "Craxii stopped before the work finished."
        case .runtimeOwnershipLost:
            "Craxii lost ownership of the running work."
        case .providerExhausted:
            "The model service could not complete the work."
        case .invalidModelOutput:
            "The model returned an unusable response."
        case .lifecycleLimit:
            "Craxii reached a work limit."
        default:
            nil
        }
    }

    private static func cancellationLabel(_ state: PendingCommandDeliveryState) -> String {
        switch state {
        case .sending: "Sending cancellation request"
        case .waitingForConnection: "Cancellation request is waiting for connection"
        case .deliveryNotConfirmed: "Cancellation delivery is not confirmed"
        case .notSent: "Cancellation request is not sent"
        case .acceptedAwaitingProjection: "Cancellation request accepted — syncing"
        }
    }

    private static func toolPresentation(_ tool: SafeToolProjection) -> ToolActivityPresentation {
        if tool.outcomeUnknown {
            return ToolActivityPresentation(
                id: tool.executionID.rawValue, title: "Tool outcome unknown",
                detail: "Craxii cannot confirm whether the operation finished.",
                isUnknownOutcome: true)
        }
        if tool.finishedAt == nil {
            return ToolActivityPresentation(
                id: tool.executionID.rawValue,
                title: tool.toolName.map { "Using \($0)" } ?? "Using a tool",
                detail: nil, isUnknownOutcome: false)
        }
        let detail: String?
        switch tool.resultClass {
        case "success": detail = "Finished successfully"
        case "nonzero_exit": detail = "Finished with a nonzero result"
        case "timed_out": detail = "Timed out"
        case "cancelled": detail = "Cancelled"
        default: detail = nil
        }
        return ToolActivityPresentation(
            id: tool.executionID.rawValue,
            title: tool.toolName.map { "Used \($0)" } ?? "Tool finished",
            detail: detail, isUnknownOutcome: false)
    }

    public static func errorPresentation(
        _ error: ClientError?, backend: SafeBackendError? = nil
    ) -> ErrorPresentation? {
        let backendFromError: SafeBackendError?
        if case let .backend(value) = error { backendFromError = value }
        else { backendFromError = nil }
        if let detail = backendFromError ?? (error == nil ? backend : nil) {
            return ErrorPresentation(
                title: "Craxii returned an error", message: detail.message,
                code: detail.code, requestID: detail.requestID.rawValue,
                retryable: detail.retryable)
        }
        guard let error else { return nil }
        switch error {
        case .credentialRequired:
            return ErrorPresentation(title: "Credential required", message: "Enter the provisioned device credential.")
        case .credentialMalformed:
            return ErrorPresentation(title: "Credential is not valid", message: "Review and replace the saved credential.")
        case .authentication:
            return ErrorPresentation(title: "Authentication failed", message: "Repair the device credential to reconnect.")
        case .networkOffline:
            return ErrorPresentation(title: "No network connection", message: "Saved commands will not be delivered until a connection is available.", retryable: true)
        case .timeout, .serverUnavailable, .serverNotReady:
            return ErrorPresentation(title: "Craxii is unavailable", message: "Try connecting again. No work outcome was inferred.", retryable: true)
        case .incompatibleProtocol, .malformedPayload, .projectionInvariant, .outboxCorrupt:
            return ErrorPresentation(title: "Craxii cannot safely sync", message: "Sending is disabled to protect the authoritative transcript.")
        case .configurationMismatch:
            return ErrorPresentation(title: "Configuration mismatch", message: "Review the endpoint or reset disposable client state.")
        case .cancellationTransportFailure:
            return ErrorPresentation(title: "Cancellation not confirmed", message: "Craxii may still be working. Durable work state remains authoritative.", retryable: true)
        case .cacheCorrupt:
            return ErrorPresentation(title: "Local cache was recovered", message: "Craxii will rebuild from the server.")
        case .keychainFailure:
            return ErrorPresentation(title: "Credential could not be saved", message: "The macOS Keychain operation failed.")
        case let .commandRejected(code):
            return ErrorPresentation(title: "Command was not sent", message: "Review the request and try again.", code: code)
        case .backend:
            return nil
        }
    }
}
