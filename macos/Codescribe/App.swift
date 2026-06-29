import AppKit
import SwiftUI

// codescribe redesign — SwiftUI host (Option B), backed by the REAL codescribe engine
// via UniFFI. WindowGroup AgentChat (renders reliably) + Settings (⌘,) + a MenuBarExtra
// tray. Regular (dock) app so it is always reachable. NOTE: the menu-bar tray icon does
// not reliably appear in this app yet (manual NSStatusItem stranded its button window
// off-screen at (0,-6); SwiftUI MenuBarExtra is the current attempt) — parked for fresh
// eyes; the app is fully usable via the window + dock meanwhile.
@main
struct CodescribeRedesignApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    private let model = AppModel.shared

    init() {
        FontLoader.register()
    }

    var body: some Scene {
        WindowGroup("codescribe — Agent", id: "agent") {
            AgentChatView(store: model.chat)
                .frame(minWidth: 900, minHeight: 600)
        }
        .windowStyle(.titleBar)

        MenuBarExtra("codescribe", systemImage: "waveform") {
            TrayMenuView(viewModel: model.tray)
        }
        .menuBarExtraStyle(.window)

        Settings {
            SettingsView(model: SettingsViewModel(engine: RealSettingsEngine()))
        }
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        let model = AppModel.shared
        model.tray.onIntent = { intent in
            switch intent {
            case .openChat:
                NSApp.activate(ignoringOtherApps: true)
                NSApp.windows.first { $0.title.contains("Agent") }?.makeKeyAndOrderFront(nil)
            case .openSettings:
                if !NSApp.sendAction(Selector(("showSettingsWindow:")), to: nil, from: nil) {
                    NSApp.sendAction(Selector(("showPreferencesWindow:")), to: nil, from: nil)
                }
            case .openOverlay:
                model.overlay.show()
            }
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }
}
