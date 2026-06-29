import AppKit
import SwiftUI

/// Owns the app's long-lived view-models + engines so they can reference each
/// other (overlay → chat, tray → overlay) without @StateObject init-order pain.
/// The menu-bar status item itself lives in the AppDelegate (proven reliable).
@MainActor
final class AppModel: ObservableObject {
    static let shared = AppModel()

    let chat: AgentChatStore
    let tray: TrayViewModel
    let overlay: OverlayController

    init() {
        let chat = AgentChatStore(engine: RealChatEngine(), threadsProvider: RealThreadsEngine())
        self.chat = chat
        self.overlay = OverlayController(store: chat)
        self.tray = TrayViewModel(engine: RealTrayEngine())
    }
}

/// Owns the floating dictation NSPanel + its OverlayState; shows/hides on demand.
/// The panel auto-starts recording on appear (the product's hero flow), so it is
/// only created/shown when the user explicitly opens it (Tray → Open Overlay).
@MainActor
final class OverlayController: ObservableObject {
    let state = OverlayState()
    private var panel: NSPanel?

    init(store: AgentChatStore) {
        state.engine = RealDictationEngine()
        state.onClose = { [weak self] in self?.hide() }
        state.onSendToAgent = { [weak self, weak store] text in
            guard let store, !text.isEmpty else { return }
            store.draft = text
            store.send()
            self?.hide()
        }
    }

    func toggle() { (panel?.isVisible ?? false) ? hide() : show() }

    func show() {
        let panel = panel ?? DictationOverlayWindow.make(state: state)
        self.panel = panel
        if let screen = NSScreen.main {
            let frame = panel.frame
            panel.setFrameOrigin(NSPoint(
                x: screen.frame.midX - frame.width / 2,
                y: screen.frame.minY + screen.frame.height * 0.18
            ))
        }
        panel.orderFrontRegardless()
    }

    func hide() {
        state.stop()
        panel?.orderOut(nil)
    }
}
