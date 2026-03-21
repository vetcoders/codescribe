import SwiftUI

@main
struct CodeScribeApp: App {
    var body: some Scene {
        MenuBarExtra("CodeScribe", systemImage: "waveform") {
            Button("Quit") {
                NSApplication.shared.terminate(nil)
            }
            .keyboardShortcut("q")
        }
    }
}
