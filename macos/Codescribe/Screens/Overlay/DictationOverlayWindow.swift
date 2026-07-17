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

/// Content container for the overlay panel. Its sole job is to keep the SwiftUI
/// hosting view's frame identical to its own bounds on every resize — including each
/// step of a live edge-drag — via an ABSOLUTE frame sync rather than an autoresizing
/// mask. The mask resizes by DELTAS measured from the hosting view's initial frame;
/// on a borderless resizable panel those deltas drift the hosting view off the
/// window's content bounds after an edge-drag, so content spilled past the window
/// edge (clipped action row, left-anchored pill/waveform) and — because the SwiftUI
/// rounded glass background was then painted beyond the window rectangle — the
/// visible corners squared off. Re-asserting `hosting.frame = bounds` per resize step
/// keeps the glass panel covering the window 1:1 at any size. Exports no layout
/// constraints, so the content↔window sizing feedback loop that once hung the app
/// stays structurally dead.
private final class OverlayContentContainer: NSView {
    private let hosting: NSView

    init(hosting: NSView) {
        self.hosting = hosting
        super.init(frame: .zero)
        addSubview(hosting)
        hosting.frame = bounds
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        hosting.frame = bounds
    }

    override func layout() {
        super.layout()
        hosting.frame = bounds
    }
}

enum DictationOverlayWindow {
    /// Hard floor for the panel's content size — below this the glass chrome and
    /// compact action row overlap. Enforced for user edge-drag (`minSize`/`contentMinSize`)
    /// AND for every programmatic `setFrame` via `clamp(_:to:)` (AppKit does not
    /// apply `minSize` to programmatic frames).
    /// Height raised 250 → 300 so the live-transcript body keeps its reserved floor
    /// (`DictationOverlayView.bodyMinHeight` = waveform block + ~3 transcript
    /// lines) without the content column overflowing the window and squaring the
    /// glass corners. U22 kept 300 in lockstep: the action row slimmed by ~16pt
    /// and `bodyMinHeight` grew 114 → 130 by the same amount, so the chrome +
    /// body sum is unchanged (and the view now carries a terminal window-frame
    /// clip as the structural backstop). Width floor (320) is unchanged.
    static let minSize = NSSize(width: 320, height: 300)
    /// First-launch content size (no persisted value yet). LANDSCAPE rectangle —
    /// operator spec: the resting state is a horizontal bar (waveform + a few
    /// transcript lines), never a portrait column. Resizing persists, so users
    /// who prefer a tall panel drag it once and keep it.
    static let defaultSize = NSSize(width: 470, height: 330)
    /// Bumped v4 → v5: v4 shipped a portrait default by mistake; the restored
    /// landscape default must take effect once over that persisted shape.
    private static let sizeDefaultsKey = "DictationOverlayPanel.contentSize.v5"

    /// Build the floating overlay panel around an injected `OverlayState`.
    /// The state's `engine`, `onClose`, and `onSendToAgent` are wired by the
    /// orchestrator before the panel is shown.
    @MainActor
    static func make(state: OverlayState, textScale: TextScaleController) -> NSPanel {
        // Wrap in TextScaleRoot so ⌘+/-/0 on this panel scale the overlay text
        // (transcript + status) via `\.csTextScale`, independently of the chat.
        let root = TextScaleRoot(controller: textScale) { DictationOverlayView(state: state) }
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
        // Fill by an ABSOLUTE frame sync (OverlayContentContainer), not an
        // autoresizing mask. AppKit's spring mask resizes by deltas from the view's
        // initial frame; on a borderless resizable panel those deltas drift the
        // hosting view off the window's content bounds after an edge-drag, clipping
        // content at the edges and squaring off the rounded glass corners. Frame-based
        // layout (no exported constraints) keeps the sizing feedback loop dead while
        // the container re-pins the hosting frame to its bounds on every resize step.
        hosting.translatesAutoresizingMaskIntoConstraints = true
        hosting.autoresizingMask = []

        let panel = FloatingOverlayPanel(
            contentRect: NSRect(origin: .zero, size: restoredContentSize()),
            styleMask: [.borderless, .nonactivatingPanel, .resizable],
            backing: .buffered,
            defer: false
        )
        panel.contentView = OverlayContentContainer(hosting: hosting)

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
