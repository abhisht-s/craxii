import OSLog
import CraxiiClientCore

/// Apple unified-logging adapter for the closed, content-free diagnostic event vocabulary.
public struct OSLogClientDiagnosticRecorder: ClientDiagnosticRecording {
    private let logger: Logger

    public init() {
        logger = Logger(subsystem: "com.craxii.client", category: "session")
    }

    public func record(_ event: ClientDiagnosticEvent) {
        logger.log(
            level: level(event),
            "event=\(event.kind.rawValue, privacy: .public) result=\(field(event.result?.rawValue), privacy: .public) error_class=\(field(event.errorClass?.rawValue), privacy: .public) profile_id=\(field(event.profileID?.rawValue), privacy: .public) generation=\(field(event.generation), privacy: .public) projection_revision=\(field(event.projectionRevision), privacy: .public) command_kind=\(field(event.commandKind?.rawValue), privacy: .public) command_id=\(field(event.commandID?.rawValue), privacy: .public) work_id=\(field(event.workID?.rawValue), privacy: .public) request_id=\(field(event.requestID?.rawValue), privacy: .public) cursor_from=\(field(event.cursorFrom?.rawValue), privacy: .public) cursor_through=\(field(event.cursorThrough?.rawValue), privacy: .public) count=\(field(event.count), privacy: .public) attempt=\(field(event.attempt), privacy: .public) delay_ms=\(field(event.delayMilliseconds), privacy: .public)"
        )
    }

    private func level(_ event: ClientDiagnosticEvent) -> OSLogType {
        switch event.kind {
        case .fatalConfiguration, .fatalProtocol: .error
        case .commandFailed: .default
        default: .info
        }
    }

    private func field<T>(_ value: T?) -> String { value.map(String.init(describing:)) ?? "null" }
}
