import SwiftUI

// codescribe redesign — SwiftUI host (Option B). Hosts the Agent Chat window, the
// standard Settings scene (⌘,), and a menu-bar Tray — all backed by the REAL
// codescribe engine through the UniFFI bridge. Overlay (live dictation NSPanel)
// follows next.
@main
struct CodescribeRedesignApp: App {
    @StateObject private var store = AgentChatStore(engine: RealChatEngine())
    @StateObject private var trayVM = TrayViewModel(engine: RealTrayEngine())

    init() {
        FontLoader.register()
    }

    var body: some Scene {
        WindowGroup("codescribe — Agent", id: "agent") {
            AgentChatView(store: store)
                .frame(minWidth: 900, minHeight: 600)
        }
        .windowStyle(.titleBar)

        // Standard macOS Settings scene (⌘,) backed by the real config bridge.
        Settings {
            SettingsView(model: SettingsViewModel(engine: RealSettingsEngine()))
        }

        // Menu-bar tray: status, dictation toggle, quick toggles, navigation.
        MenuBarExtra("codescribe", systemImage: "waveform") {
            TrayMenuHost(vm: trayVM)
        }
        .menuBarExtraStyle(.window)
    }
}

/// Binds the tray's navigation intents to the SwiftUI scene actions (which are
/// only available inside a View's environment). Open-overlay is wired when the
/// Overlay panel lands.
private struct TrayMenuHost: View {
    @ObservedObject var vm: TrayViewModel
    @Environment(\.openWindow) private var openWindow
    @Environment(\.openSettings) private var openSettings

    var body: some View {
        TrayMenuView(viewModel: vm)
            .onAppear {
                vm.onIntent = { intent in
                    switch intent {
                    case .openChat: openWindow(id: "agent")
                    case .openSettings: openSettings()
                    case .openOverlay: break // wired when the Overlay panel is integrated
                    }
                }
            }
    }
}
