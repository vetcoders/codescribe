import AppKit

// Placement model for the dictation overlay panel.
//
// Two modes, deliberately binary (no hidden third state):
// - Anchored (default): the origin is ALWAYS derived from one of six screen
//   anchors on every show(). A drag in this mode is ephemeral — the next show
//   snaps back to the anchor. Predictability over cleverness.
// - Free motion: the user's last dragged origin is persisted and restored
//   (clamped to the visible frame); the anchor is ignored.
//
// Size is persisted independently of either mode (DictationOverlayWindow).

enum OverlayAnchor: String, CaseIterable, Identifiable {
    case topLeft = "top-left"
    case topCenter = "top-center"
    case topRight = "top-right"
    case bottomLeft = "bottom-left"
    case bottomCenter = "bottom-center"
    case bottomRight = "bottom-right"

    var id: String { rawValue }

    var label: String {
        switch self {
        case .topLeft: return "Top Left"
        case .topCenter: return "Top Center"
        case .topRight: return "Top Right"
        case .bottomLeft: return "Bottom Left"
        case .bottomCenter: return "Bottom Center"
        case .bottomRight: return "Bottom Right"
        }
    }
}

enum OverlayPlacement {
    /// Gap between the panel and the visible-frame edge. The visible frame
    /// already excludes the menu bar, so a top anchor sits just under it —
    /// `.topRight` lands the panel under the tray icon.
    static let margin: CGFloat = 12

    static let defaultAnchor: OverlayAnchor = .topRight

    private static let anchorKey = "DictationOverlayPanel.anchor.v1"
    private static let freeMotionKey = "DictationOverlayPanel.freeMotion.v1"
    private static let originKey = "DictationOverlayPanel.origin.v1"

    static var anchor: OverlayAnchor {
        get {
            guard let raw = UserDefaults.standard.string(forKey: anchorKey),
                  let stored = OverlayAnchor(rawValue: raw)
            else { return defaultAnchor }
            return stored
        }
        set { UserDefaults.standard.set(newValue.rawValue, forKey: anchorKey) }
    }

    static var freeMotion: Bool {
        get { UserDefaults.standard.bool(forKey: freeMotionKey) }
        set { UserDefaults.standard.set(newValue, forKey: freeMotionKey) }
    }

    /// Pure anchor→origin math over a visible frame, split from the NSScreen
    /// wrapper so it is unit-testable without a display.
    static func origin(for anchor: OverlayAnchor, size: NSSize, in visible: NSRect) -> NSPoint {
        let x: CGFloat
        switch anchor {
        case .topLeft, .bottomLeft:
            x = visible.minX + margin
        case .topCenter, .bottomCenter:
            x = visible.midX - size.width / 2
        case .topRight, .bottomRight:
            x = visible.maxX - size.width - margin
        }
        let y: CGFloat
        switch anchor {
        case .topLeft, .topCenter, .topRight:
            y = visible.maxY - size.height - margin
        case .bottomLeft, .bottomCenter, .bottomRight:
            y = visible.minY + margin
        }
        return NSPoint(x: x, y: y)
    }

    static func origin(for anchor: OverlayAnchor, size: NSSize, on screen: NSScreen?) -> NSPoint? {
        guard let visible = screen?.visibleFrame else { return nil }
        return origin(for: anchor, size: size, in: visible)
    }

    /// Free-motion memory: the last dragged origin, restored on show.
    static func persistOrigin(_ point: NSPoint) {
        let defaults = UserDefaults.standard
        defaults.set(Double(point.x), forKey: originKey + ".x")
        defaults.set(Double(point.y), forKey: originKey + ".y")
    }

    /// Restore the persisted free-motion origin, clamped so the panel stays
    /// fully inside the screen's visible frame (displays may have changed).
    static func restoredOrigin(size: NSSize, on screen: NSScreen?) -> NSPoint? {
        let defaults = UserDefaults.standard
        guard defaults.object(forKey: originKey + ".x") != nil,
              defaults.object(forKey: originKey + ".y") != nil
        else { return nil }
        let raw = NSPoint(
            x: defaults.double(forKey: originKey + ".x"),
            y: defaults.double(forKey: originKey + ".y")
        )
        guard let visible = screen?.visibleFrame else { return raw }
        return clampOrigin(raw, size: size, in: visible)
    }

    /// Pure clamp, testable without a display.
    static func clampOrigin(_ origin: NSPoint, size: NSSize, in visible: NSRect) -> NSPoint {
        let maxX = visible.maxX - size.width
        let maxY = visible.maxY - size.height
        return NSPoint(
            x: min(max(origin.x, visible.minX), max(visible.minX, maxX)),
            y: min(max(origin.y, visible.minY), max(visible.minY, maxY))
        )
    }
}
