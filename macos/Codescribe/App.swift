import AppKit
import SwiftUI

// codescribe redesign — SwiftUI host (Option B), backed by the REAL codescribe engine
// via UniFFI. AppKit owns the menu-bar status item/popover; SwiftUI owns the
// Settings scene and the content hosted inside AppKit windows.
@main
struct CodescribeRedesignApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate

    init() {
        FontLoader.register()
    }

    var body: some Scene {
        Settings {
            SettingsView(model: SettingsViewModel(engine: RealSettingsEngine()))
        }
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private let model = AppModel.shared
    private var agentWindow: NSWindow?
    private var statusItem: NSStatusItem!
    private let popover = NSPopover()

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)

        popover.behavior = .transient
        popover.contentSize = NSSize(width: 300, height: 460)
        popover.contentViewController = NSHostingController(
            rootView: TrayMenuView(viewModel: model.tray)
        )

        model.tray.onIntent = { intent in
            switch intent {
            case .openChat:
                self.showAgent()
            case .openSettings:
                NSApp.activate(ignoringOtherApps: true)
                if !NSApp.sendAction(Selector(("showSettingsWindow:")), to: nil, from: nil) {
                    NSApp.sendAction(Selector(("showPreferencesWindow:")), to: nil, from: nil)
                }
            case .openOverlay:
                self.model.overlay.show()
            }
        }
        installStatusItem()
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    private func installStatusItem() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        if let button = item.button {
            let image = NSImage(systemSymbolName: "waveform", accessibilityDescription: "codescribe")
            image?.isTemplate = true
            button.image = image
            button.imagePosition = .imageOnly
            button.title = ""
            button.toolTip = "codescribe"
            button.action = #selector(toggleTray)
            button.target = self
        }
        statusItem = item
    }

    private func showAgent() {
        if agentWindow == nil {
            let hosting = NSHostingController(rootView: AgentChatView(store: model.chat))
            let window = NSWindow(contentViewController: hosting)
            window.title = "codescribe — Agent"
            window.setContentSize(NSSize(width: 1120, height: 720))
            window.styleMask = [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView]
            window.titlebarAppearsTransparent = true
            window.isReleasedWhenClosed = false
            window.center()
            agentWindow = window
        }
        NSApp.activate(ignoringOtherApps: true)
        agentWindow?.makeKeyAndOrderFront(nil)
    }

    private func showTray() {
        guard let button = statusItem.button else { return }
        NSApp.activate(ignoringOtherApps: true)
        popover.show(relativeTo: button.bounds, of: button, preferredEdge: .minY)
        popover.contentViewController?.view.window?.makeKey()
    }

    @objc private func toggleTray() {
        if popover.isShown {
            popover.performClose(nil)
        } else {
            showTray()
        }
    }
}
