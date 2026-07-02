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
    /// Build the floating overlay panel around an injected `OverlayState`.
    /// The state's `engine`, `onClose`, and `onSendToAgent` are wired by the
    /// orchestrator before the panel is shown.
    @MainActor
    static func make(state: OverlayState) -> NSPanel {
        let root = DictationOverlayView(state: state)
        let hosting = NSHostingController(rootView: root)
        // CRITICAL: do NOT let the hosting controller resize the window to fit the
        // (constantly animating) content — that made the panel drift in circles and
        // dodge clicks. The window owns its size (user-resizable, see below); the
        // content lays out inside whatever frame the window has.
        hosting.sizingOptions = []

        let panel = FloatingOverlayPanel(
            contentRect: NSRect(x: 0, y: 0, width: 560, height: 380),
            styleMask: [.borderless, .nonactivatingPanel, .resizable],
            backing: .buffered,
            defer: false
        )
        panel.contentViewController = hosting

        // User-resizable: borderless windows still honour edge-drag resize when
        // `.resizable` is set. Floor keeps the glass chrome + action row readable.
        panel.minSize = NSSize(width: 460, height: 300)
        // Persist the user's chosen size across launches; the orchestrator re-centres
        // the origin on every show(), so only the size is meaningfully restored.
        panel.setFrameAutosaveName("DictationOverlayPanel")

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
}
