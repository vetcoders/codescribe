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

// Breadcrumbs for the tray Notes actions. Inspect with:
//   log show --predicate 'subsystem == "com.vetcoders.codescribe" && category == "notes"' --info
private let notesLog = Logger(
    subsystem: Bundle.main.bundleIdentifier ?? "com.vetcoders.codescribe",
    category: "notes"
)

@main
struct CodescribeApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate

    init() {
        FontLoader.register()
    }

    var body: some Scene {
        Settings {
            SettingsView(model: SettingsViewModel(
                engine: RealSettingsEngine(),
                agentStatus: RealAgentStatusEngine(),
                mcpAdmin: RealMCPAdminEngine(),
                hotkeys: RealHotkeysEngine()
            ))
        }
        // Make the Settings window user-resizable: the content's `.frame` floor
        // becomes the window minimum, and it can grow from there (default is a
        // fixed content-sized window). SwiftUI restores the frame across launches.
        .windowResizability(.contentMinSize)
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private static let showAgentNotification = Notification.Name("com.vetcoders.codescribe.showAgent")
    private static let notificationObject = Bundle.main.bundleIdentifier ?? "com.vetcoders.codescribe"

    private static let helpURL = URL(string: "https://vetcoders.github.io/codescribe/")!

    private let model = AppModel.shared
    private let trayStatus = TrayStatusStore()
    private let hotkeys = CodescribeHotkeys()
    // Stateless bridge handles backing the tray's app-level actions (notes,
    // config paths, transcript history). Each call reads/writes live on-disk truth.
    private let notes = CodescribeNotes()
    private let config = CodescribeConfig()
    private let threads = CodescribeThreads()
    private var agentWindow: NSWindow?
    // Strong ref to the voice-assistive delivery listener: UniFFI releases the
    // foreign callback the moment Swift drops its reference, which would silently
    // kill live voice-reply rendering. Held for the app's lifetime.
    private var voiceDeliveryListener: VoiceDeliveryListener?
    private var statusItem: NSStatusItem!
    private var hasUnreadAgentUpdate = false
    // Local key monitor for ⌘+ / ⌘- / ⌘0 text scaling, routed to the key window's
    // surface (overlay panel vs agent window). Held so it can be removed on quit.
    private var textScaleMonitor: Any?
    private let popover = NSPopover()
    private var shouldExitForDuplicate = false
    // First-run onboarding wizard host. Presented at launch when the core gate
    // (`shouldShowOnboarding`) reports setup is due.
    private let onboarding = OnboardingWindowController(engine: RealOnboardingEngine())

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
        // Honour the persisted "Show Dock Icon" toggle at launch. LSUIElement
        // makes us an accessory by default; promote to .regular when enabled so
        // the launch state matches the tray toggle.
        NSApp.setActivationPolicy(config.trayToggles().showDockIcon ? .regular : .accessory)

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
            rootView: TrayMenuView(viewModel: model.tray, trayStatus: trayStatus)
        )

        model.tray.onIntent = { intent in
            switch intent {
            case .openChat:
                self.showAgent()
            }
        }
        model.tray.onDictationStartRequested = { [model] in
            model.overlay.prepareForRecordingStart()
            model.overlay.showForRecording()
        }
        wireTrayActions()
        installStatusItem()
        installTextScaleMonitor()
        startHotkeys()
        registerVoiceDelivery()
        prewarmRecordingController()
        // Show the first-run wizard on top of the freshly-installed tray when the
        // core reports onboarding is still due (no setup_done marker, or a stale
        // one invalidated because a required permission is missing).
        onboarding.presentIfNeeded()
    }

    /// Bind the tray's app-level action closures (Help / About / Notes /
    /// Diagnostics) to real behaviour. Navigation intents are wired separately via
    /// `onIntent`; these are the non-navigation actions the tray view invokes.
    private func wireTrayActions() {
        model.tray.onAbout = {
            NSApp.activate(ignoringOtherApps: true)
            NSApp.orderFrontStandardAboutPanel(nil)
        }
        model.tray.onHelp = {
            NSWorkspace.shared.open(Self.helpURL)
        }
        // Re-open the setup wizard on demand. Unlike `presentIfNeeded()` (launch
        // gate), `present()` always fronts the window — resume when onboarding is
        // still due, or re-run from Welcome after completion.
        model.tray.onOpenSetupWizard = { [onboarding] in
            onboarding.present()
        }

        // ── Notes ──
        model.tray.onOpenNotesFolder = { [notes] in
            NSWorkspace.shared.open(URL(fileURLWithPath: notes.notesDir()))
        }
        model.tray.onOpenTodayNote = { [notes] in
            let path = notes.todayNotePath()
            if FileManager.default.fileExists(atPath: path) {
                NSWorkspace.shared.open(URL(fileURLWithPath: path))
            } else {
                // No note captured today yet — reveal the notes folder instead.
                NSWorkspace.shared.open(URL(fileURLWithPath: notes.notesDir()))
            }
        }
        // One-shot: append the most recent transcript to the daily note. No paste
        // — Notes is a brain-dump destination. Result is surfaced in the popover
        // (and the bridge's OS toast) so the action is never a silent no-op.
        model.tray.onSaveLastTranscript = { [weak self, notes, threads, model] in
            let text = Self.latestTranscriptText(threads) ?? ""
            self?.saveToNote(tray: model.tray, emptyMessage: "No transcript to save") {
                try notes.saveText(text: text)
            }
        }
        // One-shot: capture the current selection into the daily note. The tray
        // popover steals key focus and SwiftUI `Text.textSelection` doesn't expose
        // `AXSelectedText`, so the system-wide AX read can't see a selection made
        // in our own agent window — harvest it from that window's responder chain
        // first, then fall back to the AX/clipboard path for other apps.
        model.tray.onSaveSelection = { [weak self, notes, model] in
            guard let self else { return }
            self.saveToNote(tray: model.tray, emptyMessage: "No text selected") {
                if let own = self.harvestAgentWindowSelection() {
                    notesLog.info("save selection: harvested \(own.count, privacy: .public) chars from agent window")
                    return try notes.saveText(text: own)
                }
                notesLog.info("save selection: no own-window selection; trying AX/clipboard path")
                return try notes.saveSelection()
            }
        }

        // ── Diagnostics ──
        model.tray.onOpenLogFolder = { [config] in
            // stream.log + .env + notes/transcriptions all live under the data dir.
            NSWorkspace.shared.open(URL(fileURLWithPath: config.configDir()))
        }
        model.tray.onCopyDebugInfo = { [config, notes, hotkeys] in
            Task { @MainActor in
                let recording = await hotkeys.isRecording()
                let settings = config.loadSettings()
                let info = Bundle.main.infoDictionary
                let version = info?["CFBundleShortVersionString"] as? String ?? "?"
                let build = info?["CFBundleVersion"] as? String ?? "?"
                let stt = settings.useLocalStt
                    ? "local (\(settings.localModel))"
                    : "cloud (\(settings.sttEndpoint ?? "default"))"
                let text = [
                    "codescribe debug info",
                    "app version: \(version) (\(build))",
                    "macOS: \(ProcessInfo.processInfo.operatingSystemVersionString)",
                    "recording: \(recording)",
                    "STT engine: \(stt)",
                    "config dir: \(config.configDir())",
                    "notes dir: \(notes.notesDir())",
                ].joined(separator: "\n")
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(text, forType: .string)
            }
        }
    }

    /// Text of the most recent transcript artifact, mirroring the tray engine's
    /// `latestTranscriptText` (newest history entry → its file contents).
    private static func latestTranscriptText(_ threads: CodescribeThreads) -> String? {
        // Skip failure / no-speech markers so "Save last transcript" never writes a
        // "failed" placeholder into the daily note (see RealTrayEngine).
        guard let path = threads.recentHistory(limit: 32)
            .first(where: { $0.kind.isCopyableTranscript })?.path else { return nil }
        return try? threads.readHistoryText(path: path)
    }

    /// Run a Notes save and reflect the outcome in the still-open popover. The
    /// bridge returns the saved payload (non-nil) on success, nil when there was
    /// nothing to save, and throws on a write error — every branch gets a banner
    /// so the action is fail-loud, never a silent no-op.
    private func saveToNote(
        tray: TrayViewModel,
        emptyMessage: String,
        _ perform: () throws -> String?
    ) {
        do {
            let saved = try perform()
            if let saved, !saved.isEmpty {
                notesLog.info("note saved (\(saved.count, privacy: .public) chars)")
                tray.showNoteStatus(.init(kind: .success, message: "Saved to daily note"))
            } else {
                notesLog.info("note save: nothing to save")
                tray.showNoteStatus(.init(kind: .failure, message: emptyMessage))
            }
        } catch {
            notesLog.error("note save failed: \(error.localizedDescription, privacy: .public)")
            tray.showNoteStatus(.init(kind: .failure, message: "Could not save note"))
        }
    }

    /// Best-effort harvest of the live text selection from our own agent window.
    ///
    /// The system-wide AX read used by the bridge can't see it: the tray popover
    /// has stolen key focus and SwiftUI `Text.textSelection` doesn't expose
    /// `AXSelectedText`. Instead we ask the agent window's responder chain to
    /// `copy:`, snapshotting and restoring the real pasteboard so the user's
    /// clipboard is left untouched. Returns nil when the window is absent/hidden
    /// or holds no selection (a `copy:` on an empty selection leaves the
    /// pasteboard `changeCount` unmoved).
    private func harvestAgentWindowSelection() -> String? {
        guard let window = agentWindow, window.isVisible else {
            notesLog.info("harvest: no visible agent window")
            return nil
        }
        let pasteboard = NSPasteboard.general
        let changeCountBefore = pasteboard.changeCount
        // Snapshot the ENTIRE pasteboard (every item, every type) so restoring it
        // can't clobber images/files the user had copied — a string-only snapshot
        // would drop them. Items read from the pasteboard are owned by it, so each
        // is deep-copied into a fresh NSPasteboardItem before we overwrite them.
        let savedItems: [NSPasteboardItem] = (pasteboard.pasteboardItems ?? []).map { item in
            let copy = NSPasteboardItem()
            for type in item.types {
                if let data = item.data(forType: type) {
                    copy.setData(data, forType: type)
                }
            }
            return copy
        }

        let handled = window.firstResponder?
            .tryToPerform(#selector(NSText.copy(_:)), with: nil) ?? false
        guard handled, pasteboard.changeCount != changeCountBefore else {
            notesLog.info("harvest: responder copy produced no selection")
            return nil
        }

        let harvested = pasteboard.string(forType: .string)?
            .trimmingCharacters(in: .whitespacesAndNewlines)

        // Restore the user's full clipboard — Save selection must not clobber it.
        pasteboard.clearContents()
        if !savedItems.isEmpty { pasteboard.writeObjects(savedItems) }

        guard let harvested, !harvested.isEmpty else { return nil }
        return harvested
    }

    func applicationWillTerminate(_ notification: Notification) {
        hotkeys.stop()
        if let textScaleMonitor { NSEvent.removeMonitor(textScaleMonitor) }
        DistributedNotificationCenter.default().removeObserver(self)
    }

    // MARK: - Text scaling (⌘+ / ⌘- / ⌘0)

    /// Install one local key monitor that routes text-scale shortcuts to the SURFACE
    /// under focus: the key window decides which scale you adjust. Handled events are
    /// swallowed (return nil); anything else passes through untouched.
    private func installTextScaleMonitor() {
        textScaleMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self else { return event }
            let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
            // Require ⌘ with no other command modifiers; Shift is allowed because
            // "+" is Shift-"=" on most layouts.
            guard flags.contains(.command),
                  !flags.contains(.option), !flags.contains(.control),
                  let controller = self.textScaleController(for: NSApp.keyWindow) else {
                return event
            }
            switch event.charactersIgnoringModifiers {
            case "+", "=": controller.increase(); return nil
            case "-", "_": controller.decrease(); return nil
            case "0": controller.reset(); return nil
            default: return event
            }
        }
    }

    /// The text-scale controller for a window, or nil when the key window is not a
    /// scalable surface (Settings, tray popover, panels). The overlay is discriminated
    /// by its `FloatingOverlayPanel` type; the chat by identity.
    private func textScaleController(for window: NSWindow?) -> TextScaleController? {
        guard let window else { return nil }
        if window is FloatingOverlayPanel { return model.overlay.textScale }
        if window == agentWindow { return model.chatTextScale }
        return nil
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    private func installStatusItem() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        if let button = item.button {
            button.imagePosition = .imageOnly
            button.title = ""
            button.action = #selector(toggleTray)
            button.target = self
        }
        statusItem = item
        trayStatus.onChange = { [weak self] _ in
            self?.applyStatusItemStatus()
        }
        applyStatusItemStatus()
    }

    private func applyStatusItemStatus() {
        guard let button = statusItem?.button else { return }
        button.image = statusItemImage()
        button.imagePosition = .imageOnly
        button.title = ""
        button.contentTintColor = hasUnreadAgentUpdate ? NSColor.systemYellow : nil
        button.toolTip = hasUnreadAgentUpdate
            ? "\(trayStatus.status.tooltip) - agent reply ready"
            : trayStatus.status.tooltip
    }

    private func statusItemImage() -> NSImage? {
        for symbolName in trayStatus.menuBarSymbolNames {
            if let image = NSImage(
                systemSymbolName: symbolName,
                accessibilityDescription: trayStatus.status.tooltip
            ) {
                image.isTemplate = true
                return image
            }
        }

        // Brand mark from Assets.xcassets (template image → auto-tints for
        // light/dark menu bars). If it's ever missing that's a build bug to
        // surface (empty item), not something to paper over with an old glyph.
        let image = NSImage(named: "MenuBarIcon")
        image?.isTemplate = true
        return image
    }

    private func ensureAgentWindow() -> NSWindow {
        if let agentWindow { return agentWindow }
        // Wrap in TextScaleRoot so ⌘+/-/0 on the chat window scale the message
        // bodies + composer via `\.csTextScale`, independently of the overlay.
        let root = TextScaleRoot(controller: model.chatTextScale) {
            AgentChatView(store: model.chat)
                .preferredColorScheme(.dark)
        }
        let hosting = NSHostingController(rootView: root)
        let window = NSWindow(contentViewController: hosting)
        window.title = "codescribe — Agent"
        window.setContentSize(NSSize(width: 1120, height: 720))
        window.styleMask = [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView]
        window.titlebarAppearsTransparent = true
        window.isReleasedWhenClosed = false
        window.center()
        agentWindow = window
        return window
    }

    private func showAgent(activating: Bool = true) {
        let window = ensureAgentWindow()
        if activating {
            hasUnreadAgentUpdate = false
            applyStatusItemStatus()
            NSApp.activate(ignoringOtherApps: true)
            window.makeKeyAndOrderFront(nil)
        } else if !window.isVisible {
            hasUnreadAgentUpdate = true
            applyStatusItemStatus()
            window.orderFront(nil)
        }
    }

    private func revealAgentForDelivery() {
        if agentWindow?.isVisible == true { return }
        showAgent(activating: false)
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

    /// Wire the voice-assistive agent reply stream into the chat window. The
    /// hotkey / hands-off send path streams the reply from the core runtime; this
    /// listener renders those events live (opening the chat window on turn start).
    /// Registration is process-global on the bridge side, so it stands independent
    /// of the `hotkeys.start()` Task above.
    private func registerVoiceDelivery() {
        let listener = VoiceDeliveryListener(store: model.chat) { [weak self] in
            self?.revealAgentForDelivery()
        }
        voiceDeliveryListener = listener
        hotkeys.setAgentDeliveryListener(listener: listener)
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
                // Start warmup as early as possible after launch so the engine
                // (model load + first-inference kernel compile) is ready before the
                // user's first dictation. A brief settle keeps it off the very first
                // UI frame; the heavy work runs on a background blocking thread.
                try await Task.sleep(nanoseconds: 100_000_000)
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
