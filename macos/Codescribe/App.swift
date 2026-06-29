import AppKit
import SwiftUI

// codescribe redesign — SwiftUI host (Option B). The menu-bar Tray lives in the
// AppDelegate as a real NSStatusItem (the pattern proven to work in vista-kernel;
// SwiftUI MenuBarExtra did not render here). The AgentChat window + Settings scene
// (⌘,) are SwiftUI scenes. All backed by the REAL codescribe engine via UniFFI.
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

        Settings {
            SettingsView(model: SettingsViewModel(engine: RealSettingsEngine()))
        }
    }
}

// Menu-bar status item hosting the tray popover — replicates the working
// vista-kernel setup (NSStatusItem + transient NSPopover, auto-popped once on
// launch so it's immediately visible).
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var statusItem: NSStatusItem!
    private let popover = NSPopover()

    func applicationDidFinishLaunching(_ notification: Notification) {
        // Menu-bar agent identity (vista-kernel-proven): frees menu-bar space so the
        // status item actually gets placed (a regular app's full menu + the notch
        // crowd it into the hidden zone). This is also GATE 2's target identity.
        NSApp.setActivationPolicy(.accessory)
        let model = AppModel.shared

        model.tray.onIntent = { [weak self] intent in
            _ = self
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

        popover.behavior = .transient
        popover.contentSize = NSSize(width: 320, height: 460)
        popover.contentViewController = NSHostingController(rootView: TrayMenuView(viewModel: model.tray))

        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        if let button = statusItem.button {
            button.image = NSImage(systemSymbolName: "waveform", accessibilityDescription: "codescribe")
            button.image?.isTemplate = true
            if button.image == nil { button.title = "cs" }  // text fallback, never zero-width
            button.action = #selector(toggleTray)
            button.target = self
        }

        // Pop the tray once on launch so it's immediately visible without hunting
        // the (crowded) menu bar.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) { [weak self] in
            self?.showTray()
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    private func showTray() {
        guard let button = statusItem?.button else { return }
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
