import SwiftUI
import AppKit

// Borderless floating window host for the dictation overlay.
//
// This is a FACTORY ONLY. Summon/dismiss wiring (hotkey, placement, focus handoff,
// activation policy) belongs to the orchestrator in App.swift — this file just
// builds a correctly-configured panel whose content is `DictationOverlayView`,
// with a clear background so the `.ultraThinMaterial` inside `GlassPanel` blurs
// whatever is underneath.

/// Borderless, non-activating panel that can still become key so the overlay's
/// buttons (Copy / Send to Agent / Close) receive clicks without stealing app focus.
final class FloatingOverlayPanel: NSPanel {
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
}

enum DictationOverlayWindow {
    /// Hard floor for the panel's content size — below this the glass chrome and
    /// action row overlap. Enforced for user edge-drag (`minSize`/`contentMinSize`)
    /// AND for every programmatic `setFrame` via `clamp(_:to:)` (AppKit does not
    /// apply `minSize` to programmatic frames).
    static let minSize = NSSize(width: 460, height: 300)
    /// First-launch content size (no persisted value yet).
    static let defaultSize = NSSize(width: 560, height: 380)
    private static let sizeDefaultsKey = "DictationOverlayPanel.contentSize.v2"

    /// Build the floating overlay panel around an injected `OverlayState`.
    /// The state's `engine`, `onClose`, and `onSendToAgent` are wired by the
    /// orchestrator before the panel is shown.
    @MainActor
    static func make(state: OverlayState) -> NSPanel {
        let root = DictationOverlayView(state: state)
        let hosting = NSHostingView(rootView: root)
        // CRITICAL: the WINDOW owns its size; the SwiftUI content only fills whatever
        // frame the window has. An NSHostingView otherwise installs Auto Layout
        // min/max/intrinsic constraints derived from its (flexible, constantly
        // animating) fitting size and pushes them onto the window every display
        // cycle. On a `.resizable` panel that closed a content↔window feedback loop:
        // the window resized to the fitting size → the flexible content re-fit to the
        // new frame → a different fitting size → … The two chased each other,
        // oscillating between two sizes and grinding the main thread in
        // `updateConstraintsIfNeeded → NSHostingView.updateConstraints` until the app
        // hung. Empty `sizingOptions` removes those constraints entirely; the panel is
        // sized only by us (`setFrame`) and by the user's edge-drag. Setting the
        // hosting VIEW (not just an NSHostingController) is what actually stops the
        // constraint export.
        hosting.sizingOptions = []
        // Track the window's content bounds by springs, not by a one-shot frame: on
        // every window resize AppKit grows the content view to fill, which drives
        // NSHostingView.setFrameSize and reflows the SwiftUI layout (header pinned
        // top, footer bottom, transcript region absorbing the middle). Without this
        // the view kept its creation-time frame and the content rendered anchored,
        // spilling past the window's left edge with dead space filling the rest.
        hosting.translatesAutoresizingMaskIntoConstraints = true
        hosting.autoresizingMask = [.width, .height]

        let panel = FloatingOverlayPanel(
            contentRect: NSRect(origin: .zero, size: restoredContentSize()),
            styleMask: [.borderless, .nonactivatingPanel, .resizable],
            backing: .buffered,
            defer: false
        )
        panel.contentView = hosting

        // User-resizable: borderless windows still honour edge-drag resize when
        // `.resizable` is set. Floor keeps the glass chrome + action row readable.
        panel.minSize = minSize
        panel.contentMinSize = minSize
        // Size is persisted manually (see `persist`/`restoredContentSize`), NOT via
        // `setFrameAutosaveName`: autosave on a borderless resizable panel wrote back
        // the runaway sizes produced by the old feedback loop and restored a stale,
        // oversized frame on relaunch (ghost-outline / clipped-content states). The
        // orchestrator re-centres the origin on every show() and clamps the restored
        // size to the current screen.

        // Transparent chrome so the SwiftUI glass material is the only surface.
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = false              // GlassPanel paints its own deep shadow.

        // Float above normal windows, ride along every Space, never take app focus.
        panel.level = .floating
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary]
        panel.isFloatingPanel = true
        panel.hidesOnDeactivate = false
        panel.isMovableByWindowBackground = true  // draggable via background; edges resize

        panel.titleVisibility = .hidden
        panel.titlebarAppearsTransparent = true
        panel.standardWindowButton(.closeButton)?.isHidden = true
        panel.standardWindowButton(.miniaturizeButton)?.isHidden = true
        panel.standardWindowButton(.zoomButton)?.isHidden = true

        // Size is window-owned (user-resizable) — do NOT resize to fittingSize each frame.
        return panel
    }

    /// Clamp a content size to the hard floor and to the screen's visible frame, so a
    /// programmatic `setFrame` (which AppKit does NOT clamp to `minSize`) or a stale
    /// persisted size can never render smaller than the layout minimum or larger than
    /// the current display.
    static func clamp(_ size: NSSize, to screen: NSScreen? = NSScreen.main) -> NSSize {
        var width = max(size.width, minSize.width)
        var height = max(size.height, minSize.height)
        if let visible = screen?.visibleFrame {
            width = min(width, visible.width)
            height = min(height, visible.height)
        }
        return NSSize(width: width, height: height)
    }

    /// Restore the user's last content size (clamped), or the default on first launch.
    static func restoredContentSize(for screen: NSScreen? = NSScreen.main) -> NSSize {
        let defaults = UserDefaults.standard
        let width = defaults.double(forKey: sizeDefaultsKey + ".w")
        let height = defaults.double(forKey: sizeDefaultsKey + ".h")
        let raw = (width > 0 && height > 0) ? NSSize(width: width, height: height) : defaultSize
        return clamp(raw, to: screen)
    }

    /// Persist the current content size so it survives relaunch. Called on hide().
    static func persist(size: NSSize) {
        let defaults = UserDefaults.standard
        defaults.set(Double(size.width), forKey: sizeDefaultsKey + ".w")
        defaults.set(Double(size.height), forKey: sizeDefaultsKey + ".h")
    }
}
