import AppKit
import SwiftUI

/// Owns the app's long-lived view-models + engines so they can reference each
/// other without @StateObject init-order pain.
/// The menu-bar status item itself lives in the AppDelegate (proven reliable).
@MainActor
final class AppModel: ObservableObject {
    static let shared = AppModel()

    let chat: AgentChatStore
    let overlay: OverlayController
    let tray: TrayViewModel
    /// Independent text scale for the agent chat surface (⌘+/-/0 while the chat
    /// window is key). The overlay's scale lives on `OverlayController`.
    let chatTextScale = TextScaleController(key: "AgentChat.textScale.v1")

    init() {
        let chat = AgentChatStore(engine: RealChatEngine(), threadsProvider: RealThreadsEngine())
        self.chat = chat
        self.overlay = OverlayController(store: chat, engine: ControllerDictationEngine())
        self.tray = TrayViewModel(engine: RealTrayEngine())
        // Composer voice-note dictation (independent recorder; disabled while a
        // hotkey/overlay session owns the mic — see OverlayController hooks).
        chat.dictation = RealComposerDictation(store: chat)
    }
}

/// Owns the floating dictation NSPanel + its OverlayState.
/// Recording is owned by `CodescribeHotkeys`/`RecordingController`; this panel is
/// only the SwiftUI surface for that single controller.
@MainActor
final class OverlayController: ObservableObject {
    let state: OverlayState
    /// Independent text scale for the dictation overlay (⌘+/-/0 while the panel is
    /// key). Separate from the chat scale so a distance-readable transcript and an
    /// up-close chat can be tuned independently.
    let textScale = TextScaleController(key: "DictationOverlayPanel.textScale.v1")
    private var panel: NSPanel?
    private let overlayEnabledProvider: () -> Bool
    private let assistiveStatusProvider: () -> Bool
    private let panelFactory: @MainActor (OverlayState, TextScaleController) -> NSPanel
    private let orderPanelFront: @MainActor (NSPanel) -> Void
    private let orderPanelOut: @MainActor (NSPanel) -> Void
    /// Latched across the session (preparing → started → stopped) because the
    /// Rust controller clears its assistive flag right after the stop pipeline —
    /// a single read at finalize would race it. Mid-hold upgrades (Fn → Fn+Shift)
    /// flip the tray status while recording, so every lifecycle hook re-polls.
    private var sessionWasAssistive = false

    init(
        store: AgentChatStore? = nil,
        state: OverlayState? = nil,
        engine: DictationEngine? = nil,
        overlayEnabledProvider: @escaping () -> Bool = {
            CodescribeConfig().trayToggles().transcriptionOverlayEnabled
        },
        assistiveStatusProvider: @escaping () -> Bool = {
            CodescribeTrayStatus().currentStatus().assistive
        },
        panelFactory: (@MainActor (OverlayState, TextScaleController) -> NSPanel)? = nil,
        orderPanelFront: (@MainActor (NSPanel) -> Void)? = nil,
        orderPanelOut: (@MainActor (NSPanel) -> Void)? = nil
    ) {
        let state = state ?? OverlayState()
        self.state = state
        self.overlayEnabledProvider = overlayEnabledProvider
        self.assistiveStatusProvider = assistiveStatusProvider
        self.panelFactory = panelFactory ?? {
            DictationOverlayWindow.make(state: $0, textScale: $1)
        }
        self.orderPanelFront = orderPanelFront ?? { $0.orderFrontRegardless() }
        self.orderPanelOut = orderPanelOut ?? { $0.orderOut(nil) }
        state.engine = engine
        // Drive the tray status off the SAME authoritative recording lifecycle the
        // overlay already receives. The tray view-model otherwise only polls on
        // appear (and the popover is built once), so it stayed "Recording" after
        // Finish. These hooks fire for every start/stop path (hotkey, tray, auto).
        state.onRecordingPreparing = { [weak self] in
            guard let self else { return }
            self.sessionWasAssistive = false
            self.refreshAssistiveLatch()
            self.showForRecording()
            AppModel.shared.tray.isStartingDictation = true
            // Block the composer mic while the shared recorder owns the microphone.
            AppModel.shared.chat.dictationBlocked = true
        }
        state.onRecordingStarted = { [weak self] in
            guard let self else { return }
            self.refreshAssistiveLatch()
            self.showForRecording()
            AppModel.shared.tray.isRecording = true
            AppModel.shared.tray.isStartingDictation = false
            AppModel.shared.chat.dictationBlocked = true
        }
        state.onRecordingStopped = { [weak self] in
            guard let self else { return }
            self.refreshAssistiveLatch()
            self.markStopped()
            AppModel.shared.tray.isRecording = false
            AppModel.shared.tray.isStartingDictation = false
            AppModel.shared.chat.dictationBlocked = false
        }
        state.onClose = { [weak self] in self?.hide() }
        state.onSendToAgent = { [weak self] text in
            guard !text.isEmpty else { return }
            // Rust already persisted and streamed the turn. TurnStarted opened
            // the chat passively; do NOT activate here (focus-steal at Done was
            // the wave10 operator bug). Fallback is also passive in case the
            // delivery listener missed TurnStarted.
            AppModel.shared.tray.onIntent(.revealChat)
            self?.hide()
        }
        state.onPlacementChanged = { [weak self] in self?.applyPlacement(animated: true) }
        state.attach()
    }

    func prepareForRecordingStart() {
        state.prepareForExternalStart()
    }

    /// Show the overlay for a dictation session, honouring the "Transcription
    /// Overlay" toggle. When disabled, dictation runs headless — hold the hotkey,
    /// dictate, and the text lands at the cursor (+ clipboard) with no window.
    /// Delivery is engine-side (LocalFinalPass), independent of this window, so
    /// hiding the overlay never suppresses the paste.
    func showForRecording() {
        refreshAssistiveLatch()
        guard overlayEnabledProvider() else {
            if panel != nil { hide() }
            return
        }
        show()
    }

    func show() {
        let panel = panel ?? panelFactory(state, textScale)
        self.panel = panel
        // A pending fade-out must not leave a freshly shown panel invisible.
        panel.alphaValue = 1
        applyPlacement(animated: false)
        orderPanelFront(panel)
    }

    /// Derive and apply the panel's frame from the placement prefs: free motion
    /// restores the last dragged origin, anchored derives from the anchor —
    /// in ONE setFrame so there is no transient mismatched frame. Clamping the
    /// size here covers programmatic sizing, which AppKit's minSize does not.
    private func applyPlacement(animated: Bool) {
        guard let panel else { return }
        let screen = NSScreen.main
        let size = DictationOverlayWindow.clamp(panel.frame.size, to: screen)
        let origin: NSPoint?
        if state.freeMotion {
            origin = OverlayPlacement.restoredOrigin(size: size, on: screen) ?? panel.frame.origin
        } else {
            origin = OverlayPlacement.origin(for: state.placementAnchor, size: size, on: screen)
        }
        guard let origin else {
            panel.setContentSize(size)
            return
        }
        let frame = NSRect(origin: origin, size: size)
        if animated, panel.isVisible {
            panel.animator().setFrame(frame, display: true)
        } else {
            panel.setFrame(frame, display: false)
        }
    }

    func markStopped() {
        state.finishControllerRecording()
    }

    /// Called by the live TrayStatusStore listener. All indicator surfaces
    /// consume the same Rust mode transition; agent arm never hides the overlay.
    func handleIndicatorModeChange(_ mode: CsIndicatorMode) {
        if mode == .assistive {
            sessionWasAssistive = true
        }
        state.setAutoPasteControlAvailable(!sessionWasAssistive)
        state.applyIndicatorMode(mode)
    }

    func handleAssistiveStatusChange(_ assistive: Bool) {
        handleIndicatorModeChange(assistive ? .assistive : .hold)
    }

    private func refreshAssistiveLatch() {
        handleAssistiveStatusChange(assistiveStatusProvider())
    }

    func hide() {
        // Persist the user's chosen size for next launch (replaces frame autosave,
        // which used to write back the old feedback loop's runaway sizes) — and,
        // in free motion, the dragged origin.
        if let panel {
            DictationOverlayWindow.persist(size: panel.frame.size)
            if state.freeMotion {
                OverlayPlacement.persistOrigin(panel.frame.origin)
            }
        }
        if let panel { orderPanelOut(panel) }
    }

    /// The dictated transcript was handed to the agent (voice turn opened in the
    /// chat window). The overlay's job is done — fade it out immediately instead
    /// of lingering over the conversation it just fed.
    func hideForAgentHandoff() {
        guard let panel, panel.isVisible else { return }
        DictationOverlayWindow.persist(size: panel.frame.size)
        if state.freeMotion {
            OverlayPlacement.persistOrigin(panel.frame.origin)
        }
        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.18
            panel.animator().alphaValue = 0
        } completionHandler: { [weak self] in
            Task { @MainActor in
                guard let self, let panel = self.panel else { return }
                self.orderPanelOut(panel)
                panel.alphaValue = 1
            }
        }
    }
}
