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

    init() {
        let chat = AgentChatStore(engine: RealChatEngine(), threadsProvider: RealThreadsEngine())
        self.chat = chat
        self.overlay = OverlayController(store: chat)
        self.tray = TrayViewModel(engine: RealTrayEngine())
    }
}

/// Owns the floating dictation NSPanel + its OverlayState.
/// Recording is owned by `CodescribeHotkeys`/`RecordingController`; this panel is
/// only the SwiftUI surface for that single controller.
@MainActor
final class OverlayController: ObservableObject {
    let state = OverlayState()
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
        }
        state.onRecordingStarted = { [weak self] in
            self?.showForRecording()
            AppModel.shared.tray.isRecording = true
            AppModel.shared.tray.isStartingDictation = false
        }
        state.onRecordingStopped = { [weak self] in
            self?.markStopped()
            AppModel.shared.tray.isRecording = false
            AppModel.shared.tray.isStartingDictation = false
        }
        state.onClose = { [weak self] in self?.hide() }
        state.onSendToAgent = { [weak self, weak store] text in
            guard let store, !text.isEmpty else { return }
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
        let panel = panel ?? DictationOverlayWindow.make(state: state)
        self.panel = panel
        if let screen = NSScreen.main {
            let frame = panel.frame
            let visible = screen.visibleFrame
            panel.setFrameOrigin(NSPoint(
                x: visible.midX - frame.width / 2,
                y: visible.minY + visible.height * 0.22
            ))
        }
        panel.orderFrontRegardless()
    }

    func markStopped() {
        state.finishControllerRecording()
    }

    func hide() {
        panel?.orderOut(nil)
    }
}
