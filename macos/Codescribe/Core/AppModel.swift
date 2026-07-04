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
        self.overlay = OverlayController(store: chat)
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
    let state = OverlayState()
    /// Independent text scale for the dictation overlay (⌘+/-/0 while the panel is
    /// key). Separate from the chat scale so a distance-readable transcript and an
    /// up-close chat can be tuned independently.
    let textScale = TextScaleController(key: "DictationOverlayPanel.textScale.v1")
    private var panel: NSPanel?
    // Read fresh at show-time so the tray's "Transcription Overlay" toggle takes
    // effect on the very next dictation (stateless bridge handle — cheap).
    private let config = CodescribeConfig()

    init(store: AgentChatStore) {
        state.engine = ControllerDictationEngine()
        // Drive the tray status off the SAME authoritative recording lifecycle the
        // overlay already receives. The tray view-model otherwise only polls on
        // appear (and the popover is built once), so it stayed "Recording" after
        // Finish. These hooks fire for every start/stop path (hotkey, tray, auto).
        state.onRecordingPreparing = { [weak self] in
            self?.showForRecording()
            AppModel.shared.tray.isStartingDictation = true
            // Block the composer mic while the shared recorder owns the microphone.
            AppModel.shared.chat.dictationBlocked = true
        }
        state.onRecordingStarted = { [weak self] in
            self?.showForRecording()
            AppModel.shared.tray.isRecording = true
            AppModel.shared.tray.isStartingDictation = false
            AppModel.shared.chat.dictationBlocked = true
        }
        state.onRecordingStopped = { [weak self] in
            self?.markStopped()
            AppModel.shared.tray.isRecording = false
            AppModel.shared.tray.isStartingDictation = false
            AppModel.shared.chat.dictationBlocked = false
        }
        state.onClose = { [weak self] in self?.hide() }
        state.onSendToAgent = { [weak self, weak store] text in
            guard let store, !text.isEmpty else { return }
            // Reveal + focus the Agent window (same path as the tray's Open Chat
            // intent) so the reply streams into a visible store, not a hidden one.
            AppModel.shared.tray.onIntent(.openChat)
            store.draft = text
            store.send()
            self?.hide()
        }
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
        guard config.trayToggles().transcriptionOverlayEnabled else { return }
        show()
    }

    func show() {
        let panel = panel ?? DictationOverlayWindow.make(state: state, textScale: textScale)
        self.panel = panel
        let screen = NSScreen.main
        // Clamp the current size to the active screen (it may have shrunk since the
        // size was chosen) and re-centre — in ONE setFrame so there is no transient
        // mismatched frame. Enforcing the min here covers programmatic sizing, which
        // AppKit's minSize does not.
        let size = DictationOverlayWindow.clamp(panel.frame.size, to: screen)
        if let screen {
            let visible = screen.visibleFrame
            let origin = NSPoint(
                x: visible.midX - size.width / 2,
                y: visible.minY + visible.height * 0.22
            )
            panel.setFrame(NSRect(origin: origin, size: size), display: false)
        } else {
            panel.setContentSize(size)
        }
        panel.orderFrontRegardless()
    }

    func markStopped() {
        state.finishControllerRecording()
    }

    func hide() {
        // Persist the user's chosen size for next launch (replaces frame autosave,
        // which used to write back the old feedback loop's runaway sizes).
        if let panel {
            DictationOverlayWindow.persist(size: panel.frame.size)
        }
        panel?.orderOut(nil)
    }
}
