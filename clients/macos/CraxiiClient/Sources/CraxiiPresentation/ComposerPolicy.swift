import Foundation

public enum ComposerValidation: Equatable, Sendable {
    case empty
    case whitespaceOnly
    case valid(byteCount: Int, feedback: String?)
    case overLimit(byteCount: Int, limit: Int)

    public var isValid: Bool {
        if case .valid = self { return true }
        return false
    }
}

public enum ComposerPolicy {
    public static let maximumUTF8Bytes = 65_536
    public static let feedbackThreshold = 60 * 1_024

    public static func validate(_ text: String) -> ComposerValidation {
        let bytes = text.utf8.count
        if text.isEmpty { return .empty }
        if text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            return .whitespaceOnly
        }
        if bytes > maximumUTF8Bytes {
            return .overLimit(byteCount: bytes, limit: maximumUTF8Bytes)
        }
        let feedback = bytes >= feedbackThreshold
            ? "\(bytes.formatted()) of \(maximumUTF8Bytes.formatted()) UTF-8 bytes"
            : nil
        return .valid(byteCount: bytes, feedback: feedback)
    }

    public static func canSend(
        _ text: String, mutationsAllowed: Bool, isPreparing: Bool
    ) -> Bool {
        mutationsAllowed && !isPreparing && validate(text).isValid
    }

    public static func returnAction(commandModifier: Bool) -> ComposerReturnAction {
        commandModifier ? .send : .insertNewline
    }
}

public enum ComposerReturnAction: Equatable, Sendable {
    case insertNewline
    case send
}
