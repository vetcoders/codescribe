import Foundation

/// Pure policy for when the Agent chat window may steal focus.
///
/// Voice delivery (`TurnStarted` / end-of-turn fallback) must never activate the
/// app. Explicit user opens (tray, summon shortcut, external launch) may.
///
/// W10-A: end-of-turn used to call activating `.openChat`, which is why the
/// window appeared only with the finished answer and stole focus.
enum AgentRevealIntent: Equatable {
    /// Tray menu, status-item double-click path, show-agent hotkey, external launch.
    case explicitOpen
    /// Voice TurnStarted passive reveal, or end-of-turn non-activating fallback.
    case voiceDelivery
}

enum AgentRevealPolicy {
    /// Whether `NSApp.activate(ignoringOtherApps:)` is allowed for this intent.
    static func shouldActivate(for intent: AgentRevealIntent) -> Bool {
        switch intent {
        case .explicitOpen:
            return true
        case .voiceDelivery:
            return false
        }
    }

    /// Whether a passive reveal should re-order an already-visible window.
    /// Always true: "visible" alone is not enough under LSUIElement + multi-Space.
    static func shouldReorderEvenIfVisible(for intent: AgentRevealIntent) -> Bool {
        switch intent {
        case .explicitOpen, .voiceDelivery:
            return true
        }
    }
}
