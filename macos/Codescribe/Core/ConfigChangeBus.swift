import Foundation

/// Cross-surface config observation for dual writers that share the same env
/// keys (Settings panel + tray menu).
///
/// K4 (W10-E): Settings and tray both read `hold_badge_size` / HOLD_INDICATOR
/// but previously did not observe each other's writes, so the UI could show
/// 8px in Settings and 4px in the tray simultaneously.
///
/// K3: size/visibility changes persist immediately and take effect on the
/// *next* badge show — this bus only syncs UI observers; it does not live-resize
/// a visible caret badge.
enum ConfigChangeBus {
    static let holdBadgeDidChange = Notification.Name("codescribe.config.holdBadgeDidChange")

    static func postHoldBadgeChanged() {
        NotificationCenter.default.post(name: holdBadgeDidChange, object: nil)
    }
}
