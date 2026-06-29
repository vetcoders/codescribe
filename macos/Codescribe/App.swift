import AppKit
import SwiftUI

// codescribe redesign — SwiftUI host (Option B). Hosts the Agent Chat window and
// the standard Settings scene (⌘,); the menu-bar Tray + floating Overlay are
// driven from an AppKit AppDelegate (NSStatusItem proved more reliable than
// SwiftUI MenuBarExtra on this macOS). All backed by the REAL codescribe engine
// through the UniFFI bridge.
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

        // Standard macOS Settings scene (⌘,) backed by the real config bridge.
        Settings {
            SettingsView(model: SettingsViewModel(engine: RealSettingsEngine()))
        }
    }
}

/// Owns the menu-bar status item + its SwiftUI tray popover, and binds the tray's
/// navigation intents to AppKit actions (the SwiftUI scene-action environment is
/// not available here). NSStatusItem renders reliably where MenuBarExtra did not.
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var statusItem: NSStatusItem?
    private let popover = NSPopover()

    func applicationDidFinishLaunching(_ notification: Notification) {
        let model = AppModel.shared

        // Tray navigation intents (AppKit side — no SwiftUI environment here).
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

        // SwiftUI tray menu hosted in a transient popover.
        let host = NSHostingController(rootView: TrayMenuView(viewModel: model.tray))
        host.sizingOptions = [.preferredContentSize]
        popover.behavior = .transient
        popover.contentViewController = host

        // Menu-bar status item (waveform glyph).
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        item.button?.image = NSImage(systemSymbolName: "waveform", accessibilityDescription: "codescribe")
        item.button?.action = #selector(togglePopover(_:))
        item.button?.target = self
        statusItem = item
    }

    @objc private func togglePopover(_ sender: NSStatusBarButton) {
        if popover.isShown {
            popover.performClose(sender)
        } else {
            popover.show(relativeTo: sender.bounds, of: sender, preferredEdge: .minY)
            popover.contentViewController?.view.window?.makeKey()
        }
    }
}
