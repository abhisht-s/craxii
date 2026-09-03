import Foundation

public enum TranscriptScrollAction: Equatable, Sendable {
    case none
    case scrollToBottom
    case recordUnseenActivity
}

public enum TranscriptScrollPolicy {
    public static let defaultNearBottomThreshold: Double = 80

    public static func isNearBottom(
        distance: Double, threshold: Double = defaultNearBottomThreshold
    ) -> Bool {
        distance <= threshold
    }

    public static func action(
        initialLoad: Bool,
        userSubmitted: Bool,
        activityChanged: Bool,
        isNearBottom: Bool
    ) -> TranscriptScrollAction {
        if initialLoad || userSubmitted { return .scrollToBottom }
        guard activityChanged else { return .none }
        return isNearBottom ? .scrollToBottom : .recordUnseenActivity
    }
}
