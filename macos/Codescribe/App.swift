import AppKit
import OSLog
import SwiftUI

// codescribe redesign — SwiftUI host (Option B), backed by the REAL codescribe engine
// via UniFFI. AppKit owns the menu-bar status item/popover; SwiftUI owns the
// Settings scene and the content hosted inside AppKit windows.
private let appLogger = Logger(
    subsystem: Bundle.main.bundleIdentifier ?? "com.vetcoders.codescribe",
    category: "App"
)

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
    private static let showAgentNotification = Notification.Name("com.vetcoders.codescribe.showAgent")
    private static let notificationObject = Bundle.main.bundleIdentifier ?? "com.vetcoders.codescribe"

    private let model = AppModel.shared
    private let hotkeys = CodescribeHotkeys()
    private var agentWindow: NSWindow?
    private var statusItem: NSStatusItem!
    private let popover = NSPopover()
    private var shouldExitForDuplicate = false

    func applicationWillFinishLaunching(_ notification: Notification) {
        guard Self.isDuplicateInstance else { return }
        shouldExitForDuplicate = true
        DistributedNotificationCenter.default().postNotificationName(
            Self.showAgentNotification,
            object: Self.notificationObject,
            userInfo: nil,
            deliverImmediately: true
        )
        NSApp.terminate(nil)
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        guard !shouldExitForDuplicate else { return }
        NSApp.setActivationPolicy(.accessory)

        DistributedNotificationCenter.default().addObserver(
            self,
            selector: #selector(showAgentFromExternalLaunch),
            name: Self.showAgentNotification,
            object: Self.notificationObject,
            suspensionBehavior: .deliverImmediately
        )

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
            }
        }
        model.tray.onDictationStartRequested = { [model] in
            model.overlay.prepareForRecordingStart()
            model.overlay.show()
        }
        installStatusItem()
        startHotkeys()
        prewarmRecordingController()
    }

    func applicationWillTerminate(_ notification: Notification) {
        hotkeys.stop()
        DistributedNotificationCenter.default().removeObserver(self)
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

    @objc private func showAgentFromExternalLaunch() {
        showAgent()
    }

    private func startHotkeys() {
        Task { [hotkeys] in
            do {
                try await hotkeys.start()
                appLogger.info("Codescribe hotkeys active: \(hotkeys.isActive(), privacy: .public)")
            } catch {
                appLogger.error("Codescribe hotkeys unavailable: \(String(describing: error), privacy: .public)")
            }
        }
    }

    private func prewarmRecordingController() {
        Task { [hotkeys] in
            do {
                try await Task.sleep(nanoseconds: 750_000_000)
                try await hotkeys.prewarmRecording()
                appLogger.info("Codescribe recording controller prewarmed")
            } catch {
                appLogger.error("Codescribe recording prewarm failed: \(String(describing: error), privacy: .public)")
            }
        }
    }

    private static var isDuplicateInstance: Bool {
        guard let bundleIdentifier = Bundle.main.bundleIdentifier else { return false }
        let currentPID = ProcessInfo.processInfo.processIdentifier
        return NSRunningApplication
            .runningApplications(withBundleIdentifier: bundleIdentifier)
            .contains { app in
                app.processIdentifier != currentPID && !app.isTerminated
            }
    }
}
