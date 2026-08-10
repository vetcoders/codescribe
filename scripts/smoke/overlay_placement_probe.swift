// ============================================================================
// overlay_placement_probe.swift — headless NSPanel placement canary
// ============================================================================
// Compiled TOGETHER with the production source:
//
//   swiftc macos/Codescribe/Screens/Overlay/OverlayPlacement.swift \
//          scripts/smoke/overlay_placement_probe.swift -o overlay_probe
//
// Why a standalone binary instead of the XCTest suite: the CodescribeTests
// host boots the full AppModel, which on this host hangs "before establishing
// connection" while the shipped app is running. `OverlayPlacement` imports
// nothing but AppKit and its geometry entry points are pure, so the placement
// contract can be proven with zero app lifecycle — which is exactly what a
// per-beta smoke needs.
//
// Only the PURE entry points are exercised (`origin(for:size:in:)` and
// `clampOrigin(_:size:in:)`). The UserDefaults-backed properties are NOT
// touched: a bare executable would write into its own preference domain, and
// a smoke run must never mutate operator state.
//
// Output protocol: TAB-separated `row<TAB>status<TAB>evidence` on stdout.
// Exit 0 when every assertion holds, 1 otherwise.

import AppKit

@main
enum OverlayPlacementProbe {

    static var failures: [String] = []
    static var checks = 0

    static func expect(_ condition: Bool, _ message: @autoclosure () -> String) {
        checks += 1
        if !condition { failures.append(message()) }
    }

    static func rect(_ origin: NSPoint, _ size: NSSize) -> NSRect {
        NSRect(origin: origin, size: size)
    }

    /// A panel is placed correctly when it sits fully inside the visible frame.
    static func expectContained(_ frame: NSRect, in visible: NSRect, _ label: String) {
        expect(
            frame.minX >= visible.minX - 0.5 && frame.maxX <= visible.maxX + 0.5
                && frame.minY >= visible.minY - 0.5 && frame.maxY <= visible.maxY + 0.5,
            "\(label): panel \(frame) escapes visible frame \(visible)"
        )
    }

    static func expectFinite(_ point: NSPoint, _ label: String) {
        expect(
            point.x.isFinite && point.y.isFinite,
            "\(label): non-finite origin \(point)"
        )
    }

    // The overlay's own default size (DictationOverlayWindow default content size).
    static let panel = NSSize(width: 420, height: 260)

    // -----------------------------------------------------------------------
    // Geometries. Each mirrors a real macOS 27 screen situation.
    // -----------------------------------------------------------------------

    /// Normal Space on a built-in display: visibleFrame excludes menu bar + Dock.
    static let normalSpace = NSRect(x: 0, y: 76, width: 1728, height: 1004)

    /// Fullscreen Space: AppKit reports visibleFrame == frame (no menu-bar
    /// inset) because the menu bar auto-hides. The placement math must still
    /// land the panel inside, and must not assume a non-zero top inset.
    static let fullscreenSpace = NSRect(x: 0, y: 0, width: 1728, height: 1117)

    /// Secondary display left of and below the primary — negative origins are
    /// the classic source of off-screen panels after a topology change.
    static let secondaryDisplay = NSRect(x: -1920, y: -420, width: 1920, height: 1080)

    /// Pathological: a visible frame smaller than the panel itself (tiny
    /// external display / heavily scaled mode). The clamp must degrade to the
    /// frame origin, never to NaN or an inverted rect.
    static let tinyDisplay = NSRect(x: 100, y: 100, width: 320, height: 200)

    static func main() {
        let geometries = [
            ("normal-space", normalSpace),
            ("fullscreen-space", fullscreenSpace),
            ("secondary-display", secondaryDisplay),
        ]

        // -------------------------------------------------------------------
        // Row 1 — anchored placement lands inside the visible frame, all six
        // anchors, on every geometry including a fullscreen-shaped frame.
        // -------------------------------------------------------------------

        for (name, visible) in geometries {
            for anchor in OverlayAnchor.allCases {
                let origin = OverlayPlacement.origin(for: anchor, size: panel, in: visible)
                expectFinite(origin, "\(name)/\(anchor.rawValue)")
                expectContained(rect(origin, panel), in: visible, "\(name)/\(anchor.rawValue)")
            }
        }

        // The default anchor must sit under the tray icon: top-right + margin.
        let defaultOrigin = OverlayPlacement.origin(
            for: OverlayPlacement.defaultAnchor, size: panel, in: normalSpace)
        expect(
            abs(defaultOrigin.x - (normalSpace.maxX - panel.width - OverlayPlacement.margin))
                < 0.001,
            "default anchor x drifted: \(defaultOrigin.x)"
        )
        expect(
            abs(defaultOrigin.y - (normalSpace.maxY - panel.height - OverlayPlacement.margin))
                < 0.001,
            "default anchor y drifted: \(defaultOrigin.y)"
        )

        // -------------------------------------------------------------------
        // Row 2 — the clamp pulls a stale persisted origin back onto screen.
        // -------------------------------------------------------------------

        for (name, visible) in geometries {
            for stale in [
                NSPoint(x: 99_999, y: 99_999),
                NSPoint(x: -99_999, y: -99_999),
                NSPoint(x: visible.maxX, y: visible.maxY),
                NSPoint(x: visible.minX - 1, y: visible.minY - 1),
            ] {
                let clamped = OverlayPlacement.clampOrigin(stale, size: panel, in: visible)
                expectFinite(clamped, "clamp/\(name)")
                expectContained(rect(clamped, panel), in: visible, "clamp/\(name) from \(stale)")
            }
        }

        // An origin already inside survives untouched (clamp is not re-anchor).
        let inside = NSPoint(x: normalSpace.minX + 40, y: normalSpace.minY + 40)
        let untouched = OverlayPlacement.clampOrigin(inside, size: panel, in: normalSpace)
        expect(
            abs(untouched.x - inside.x) < 0.001 && abs(untouched.y - inside.y) < 0.001,
            "clamp moved an already-valid origin: \(inside) -> \(untouched)"
        )

        // -------------------------------------------------------------------
        // Row 3 — degenerate geometry degrades, never explodes.
        // -------------------------------------------------------------------

        let degenerate = OverlayPlacement.clampOrigin(
            NSPoint(x: 10_000, y: -10_000), size: panel, in: tinyDisplay)
        expectFinite(degenerate, "tiny-display")
        expect(
            abs(degenerate.x - tinyDisplay.minX) < 0.001
                && abs(degenerate.y - tinyDisplay.minY) < 0.001,
            "tiny-display clamp should pin to the frame origin, got \(degenerate)"
        )

        // -------------------------------------------------------------------

        let status = failures.isEmpty ? "PASS" : "FAIL"
        let evidence =
            failures.isEmpty
            ? "\(checks) assertions green · 6 anchors × {normal, fullscreen, secondary} + stale-origin clamp + degenerate frame"
            : failures.prefix(4).joined(separator: " | ")
        print("overlay-placement\t\(status)\t\(evidence)")
        exit(failures.isEmpty ? 0 : 1)
    }
}
