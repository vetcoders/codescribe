import SwiftUI

// codescribe redesign — SwiftUI host (Option B, W2). Hosts the Agent Chat screen
// backed by the REAL codescribe engine through the UniFFI bridge. First relocated
// screen; the rest follow as the bridge widens (STT/overlay/settings).
@main
struct CodescribeRedesignApp: App {
    @StateObject private var store = AgentChatStore(engine: RealChatEngine())

    init() {
        FontLoader.register()
    }

    var body: some Scene {
        WindowGroup("codescribe — Agent") {
            AgentChatView(store: store)
                .frame(minWidth: 900, minHeight: 600)
        }
        .windowStyle(.titleBar)

        // Standard macOS Settings scene (⌘,) backed by the real config bridge.
        Settings {
            SettingsView(model: SettingsViewModel(engine: RealSettingsEngine()))
        }
    }
}
