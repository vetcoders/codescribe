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

/// Testable seam for the no-payload Agent command. AppDelegate remains the
/// sole window owner; this action only sequences fronting that window and
/// requesting composer focus on the existing store.
@MainActor
final class AgentSummonAction {
  private let store: AgentChatStore
  private let showAgent: @MainActor () -> Void

  init(
    store: AgentChatStore,
    showAgent: @escaping @MainActor () -> Void
  ) {
    self.store = store
    self.showAgent = showAgent
  }

  func perform() {
    showAgent()
    store.requestComposerFocus()
  }
}

/// Testable seam for an explicit Settings open: the tray row, the Agent chrome
/// button, the license gate and ⌘, all route here through `presentSettings()`.
///
/// Settings is a plain SwiftUI `Window` in an LSUIElement app, and the tray
/// hosts its rows in a non-activating panel. `openWindow` alone therefore put
/// Settings on screen while Codescribe stayed inactive, and the next click
/// beside it left the window under the frontmost app (Founder, 2026-09-29).
/// Settings now arrives the way an explicit Agent open does
/// (`AppDelegate.showAgent`): Codescribe becomes the active app first, then the
/// scene fronts its window as key. Opening the tray menu itself never comes
/// here — only an entry that names Settings does.
@MainActor
struct SettingsOpenAction {
  let activateApp: @MainActor () -> Void
  let openScene: @MainActor () -> Void

  func perform() {
    activateApp()
    openScene()
  }
}

extension OpenWindowAction {
  @MainActor
  func presentSettings() {
    SettingsOpenAction(
      activateApp: { NSApp.activate(ignoringOtherApps: true) },
      openScene: { self(id: SettingsView.windowID) }
    ).perform()
  }
}

/// UniFFI callbacks arrive off-main. This listener performs exactly one hop to
/// the AppDelegate-owned action and carries no recording/model payload.
final class AgentAppActionListener: CsAppActionListener, Sendable {
  private enum Action: Sendable { case showAgent, maxApprovalsChanged }
  private let continuation: AsyncStream<Action>.Continuation
  private let consumer: Task<Void, Never>

  @MainActor
  init(
    maxApprovalsChanged: @escaping @MainActor @Sendable () async -> Void = {},
    summonAgent: @escaping @MainActor @Sendable () -> Void
  ) {
    let channel = AsyncStream<Action>.makeStream()
    continuation = channel.continuation
    consumer = Task { @MainActor in
      for await action in channel.stream {
        switch action {
        case .showAgent: summonAgent()
        case .maxApprovalsChanged: await maxApprovalsChanged()
        }
      }
    }
  }

  func onShowAgent() {
    continuation.yield(.showAgent)
  }

  func onMaxApprovalsChanged() {
    continuation.yield(.maxApprovalsChanged)
  }

  func invalidate() {
    continuation.finish()
    consumer.cancel()
  }
}

/// AppKit grants one deferred Quit reply. The injected deadline lets XCTest
/// exercise both completion orders without launching or terminating NSApp.
@MainActor
final class AppTerminationCoordinator {
  private let deadline: @Sendable () async -> Void
  private var started = false
  private var replied = false
  private var deadlineTask: Task<Void, Never>?

  init(
    deadline: @escaping @Sendable () async -> Void = {
      try? await Task.sleep(for: .seconds(10))
    }
  ) {
    self.deadline = deadline
  }

  func begin(
    cleanup: @escaping @MainActor () async -> Void,
    reply: @escaping @MainActor () -> Void
  ) {
    guard !started else { return }
    started = true
    deadlineTask = Task { @MainActor in
      await deadline()
      finish(reply: reply)
    }
    Task { @MainActor in
      await cleanup()
      finish(reply: reply)
    }
  }

  private func finish(reply: @MainActor () -> Void) {
    guard !replied else { return }
    replied = true
    deadlineTask?.cancel()
    deadlineTask = nil
    reply()
  }
}

@main
struct CodescribeApp: App {
  @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
  @Environment(\.openWindow) private var openWindow

  init() {
    if CommandLine.arguments.contains("--install-agent-bridge") {
      do {
        print(try RealAgentBridgeInstaller().installRuntime())
        Darwin.exit(0)
      } catch {
        print("Agent bridge installation failed: " + error.localizedDescription)
        Darwin.exit(1)
      }
    }
    FontLoader.register()
  }

  var body: some Scene {
    settingsWindow
  }

  private var settingsWindow: some Scene {
    Window("Settings", id: SettingsView.windowID) {
      if QualityCaptureHost.isRunningTests {
        // AppDelegate already keeps the XCTest host passive. Its scene must
        // also avoid opening real model caches before the test runner starts.
        EmptyView()
      } else {
        SettingsView(
          model: SettingsViewModel(
            engine: RealSettingsEngine(),
            agentStatus: RealAgentStatusEngine(),
            mcpAdmin: RealMCPAdminEngine(),
            hotkeys: RealHotkeysEngine(),
            licenseService: LicenseService.shared
          ))
      }
    }
    .defaultSize(width: 1000, height: 720)
    .windowResizability(.contentMinSize)
    .commands {
      CommandGroup(replacing: .appSettings) {
        Button("Settings…") { openWindow.presentSettings() }
          .keyboardShortcut(",", modifiers: .command)
      }
    }
  }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
  private static let showAgentNotification = Notification.Name("com.vetcoders.codescribe.showAgent")
  private static let notificationObject = Bundle.main.bundleIdentifier ?? "com.vetcoders.codescribe"

  private static let helpURL = URL(string: "https://github.com/vetcoders/codescribe#readme")!
  private static let privacyURL = URL(string: "https://codescribe.vetcoders.io/privacy")!
  private static let termsURL = URL(string: "https://codescribe.vetcoders.io/terms")!

  // Every core-touching handle below is `lazy` for correctness, not for launch
  // cost. The XCTest bundle uses this app as its host, so `AppDelegate` is
  // instantiated inside the test process. Stored properties are initialised at
  // *instantiation*, which happens before `applicationWillFinishLaunching` —
  // so the `Self.isRunningTests` guards on the lifecycle methods below are
  // structurally unable to stop them, no matter what they check.
  //
  // Constructed eagerly, they boot a second Rust core beside the test:
  // `AppModel.shared` builds the chat/overlay/tray engines, and
  // `TrayStatusStore.init` goes further and registers a live listener on the
  // core (`TrayStatusStore.swift:56`). That listener answers
  // `ConfigChangeBus.holdBadgeDidChange`, so a pure-logic settings unit test
  // running entirely on a mock engine drove real core work.
  //
  // Measured 2026-08-08 on this host, whole suite (317 tests), same binary
  // except for these keywords:
  //   eager   86.7 s  — of which 82.3 s is one test,
  //                     SettingsTruthTests.testHoldBadgeControlRoundTrips…,
  //                     which costs 0.009 s when run alone
  //   lazy     4.1-4.5 s  (n=4, app running or not)
  // The gap is fan-out: XCTest holds every test-case instance for the whole
  // run, so each view model an earlier test built is still a live observer
  // when this one posts. Removing the app's own core-backed observers from the
  // host is what collapses it.
  //
  // Deferring to first access costs production nothing: the earliest use is
  // `applicationDidFinishLaunching` (line ~179), already behind the guard.
  private lazy var model = AppModel.shared
  private lazy var trayStatus = TrayStatusStore()
  private lazy var hotkeys = CodescribeHotkeys()
  // Stateless bridge handles backing the tray's app-level actions (notes,
  // config paths, transcript history). Each call reads/writes live on-disk truth.
  private lazy var notes = CodescribeNotes()
  private lazy var config = CodescribeConfig()
  private lazy var threads = CodescribeThreads()
  private var agentWindow: NSWindow?
  // Strong ref to the voice-assistive delivery listener: UniFFI releases the
  // foreign callback the moment Swift drops its reference, which would silently
  // kill live voice-reply rendering. Held for the app's lifetime.
  private var voiceDeliveryListener: VoiceDeliveryListener?
  private var appActionListener: AgentAppActionListener?
  private var onDeviceFormatter: OnDeviceFormatterHost?
  private lazy var maxPermissionModel = SettingsViewModel(engine: RealSettingsEngine())
  private lazy var agentSummonAction = AgentSummonAction(
    store: model.chat,
    showAgent: { [weak self] in self?.showAgent() }
  )
  private var statusItem: NSStatusItem!
  private var hasUnreadAgentUpdate = false
  // Local key monitor for ⌘+ / ⌘- / ⌘0 text scaling, routed to the key window's
  // surface (overlay panel vs agent window). Held so it can be removed on quit.
  private var textScaleMonitor: Any?
  // NSWorkspace sleep/wake bridge. The observer itself only coalesces one
  // next-tick callback; the async Rust hop happens outside AppKit's callout.
  private var sleepWakeObserver: SystemSleepWakeObserver?
  private lazy var trayPanel = TrayPanel()
  private var shouldExitForDuplicate = false
  private let terminationCoordinator = AppTerminationCoordinator()
  // First-run onboarding wizard host. Presented at launch when the core gate
  // (`shouldShowOnboarding`) reports setup is due.
  private lazy var onboarding = OnboardingWindowController(engine: RealOnboardingEngine())
  // Sparkle update channel. Created in didFinishLaunching (after the
  // duplicate-instance/test-host guard) so the XCTest host never starts a
  // scheduled updater alongside the live app.
  private var updater: UpdaterService?
  private let vocabularyAB = VocabularyABAction()

  /// True when the process is the XCTest host, not a user launch. The unit-test
  /// runner reuses this app as its host: without this gate the duplicate-instance
  /// check would `terminate` the runner whenever the real app is running, and
  /// the host would start hotkeys + engine prewarm alongside the live instance
  /// (fighting it for the CGEventTap and the microphone).
  private static let isRunningTests = QualityCaptureHost.isRunningTests

  func applicationWillFinishLaunching(_ notification: Notification) {
    guard !Self.isRunningTests, Self.isDuplicateInstance else { return }
    shouldExitForDuplicate = true
    DistributedNotificationCenter.default().postNotificationName(
      Self.showAgentNotification,
      object: Self.notificationObject,
      userInfo: nil,
      deliverImmediately: true
    )
    NSApp.terminate(nil)
  }

  func application(_ application: NSApplication, open urls: [URL]) {
    guard !Self.isRunningTests, !shouldExitForDuplicate else { return }
    for url in urls {
      vocabularyAB.receive(url, labEnabled: DictationOverlayGate.isLabModeOn())
    }
  }

  func applicationShouldOpenUntitledFile(_ sender: NSApplication) -> Bool { false }

  /// A reopen — Dock icon click, `open -a`, a Finder or Spotlight launch of the
  /// already running app — presents nothing. Left unhandled, SwiftUI answers a
  /// reopen by presenting a window scene, and the only one here is the Settings
  /// `Window`: a scratch probe on 2026-09-29 (accessory app, one `Window`
  /// scene, `kAEReopenApplication` sent to itself) got the closed window back
  /// on screen, and returning false kept it closed. Settings opens only from an
  /// explicit entry (`OpenWindowAction.presentSettings`).
  func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool)
    -> Bool
  {
    false
  }

  func applicationDidFinishLaunching(_ notification: Notification) {
    guard !shouldExitForDuplicate, !Self.isRunningTests else { return }
    // Honour the persisted "Show Dock Icon" toggle at launch. LSUIElement
    // makes us an accessory by default; promote to .regular when enabled so
    // the launch state matches the tray toggle.
    let trayToggles = config.trayToggles()
    NSApp.setActivationPolicy(trayToggles.showDockIcon ? .regular : .accessory)

    do {
      let runtime = try startApplicationRuntime()
      appLogger.info(
        "Application runtime started with \(runtime.workerCount, privacy: .public) workers: \(runtime.workerNames.joined(separator: ","), privacy: .public)"
      )
    } catch {
      appLogger.fault(
        "Application runtime failed to start: \(error.localizedDescription, privacy: .public)")
      NSApp.terminate(nil)
      return
    }

    Task { @MainActor in
      let detail = await Task.detached(priority: .utility) {
        RealAgentBridgeInstaller().synchronizeManagedPayload()
      }.value
      appLogger.info("\(detail, privacy: .public)")
      SettingsViewModel.recordAgentBridgeLaunchSynchronization(detail)
    }

    DistributedNotificationCenter.default().addObserver(
      self,
      selector: #selector(showAgentFromExternalLaunch),
      name: Self.showAgentNotification,
      object: Self.notificationObject,
      suspensionBehavior: .deliverImmediately
    )

    trayPanel.onDismiss = { [weak self] in self?.model.tray.collapseDisclosures() }

    model.tray.onIntent = { intent in
      switch intent {
      case .openChat:
        self.showAgent()
      case .revealChat:
        self.revealAgentForDelivery()
      }
    }
    model.tray.onDictationStartRequested = { [model] in
      model.overlay.prepareForRecordingStart()
      model.overlay.showForRecording()
    }
    updater = UpdaterService()
    wireTrayActions()
    installStatusItem()
    installTextScaleMonitor()
    registerAppActions()
    registerOnDeviceFormatter()
    startHotkeys()
    installSystemSleepWakeObserver()
    registerVoiceDelivery()
    prewarmRecordingController()
    // Speech Recognition TCC must be requested from THIS process
    // (com.vetcoders.codescribe). The bridge child is co-located under
    // Contents/MacOS and inherits the app's responsible identity; granting
    // only via Terminal/CLI leaves the app as speech_auth_not_determined.
    ensureSpeechRecognitionAtLaunch()
    // Show the first-run wizard on top of the freshly-installed tray when the
    // core reports onboarding is still due (no setup_done marker, or a stale
    // one invalidated because a required permission is missing).
    onboarding.presentIfNeeded()
  }

  /// Prompt Speech Recognition while undetermined so Apple live dictation can
  /// start on first hotkey without a Settings detour. Denied/restricted is a
  /// no-op here — Settings › Dictation surfaces the deep link.
  ///
  /// Accessory (LSUIElement) apps often fail to surface TCC dialogs unless the
  /// process is briefly activation-eligible — promote to regular for the
  /// request window, then restore the dock policy.
  private func ensureSpeechRecognitionAtLaunch() {
    let probe = NativePermissionProbe().snapshot()
    guard probe.speechRecognition == .notDetermined else {
      appLogger.info(
        "Speech Recognition at launch already \(String(describing: probe.speechRecognition), privacy: .public)"
      )
      return
    }
    let priorPolicy = NSApp.activationPolicy()
    if priorPolicy == .accessory {
      NSApp.setActivationPolicy(.regular)
    }
    NSApp.activate(ignoringOtherApps: true)
    Task { @MainActor [weak self] in
      let state = await SpeechRecognitionPermission.request()
      guard let self else { return }
      if priorPolicy == .accessory {
        // Restore accessory only when the user has not enabled Dock icon.
        if !self.config.trayToggles().showDockIcon {
          NSApp.setActivationPolicy(.accessory)
        }
      }
      appLogger.info(
        "Speech Recognition at launch → \(String(describing: state), privacy: .public)")
    }
  }

  /// Bind the tray's app-level action closures (Help / About / Notes /
  /// Diagnostics) to real behaviour. Navigation intents are wired separately via
  /// `onIntent`; these are the non-navigation actions the tray view invokes.
  private func wireTrayActions() {
    model.tray.onAbout = {
      NSApp.activate(ignoringOtherApps: true)
      // Build provenance in the standard About panel (Pensieve-style):
      // version/build come from Info.plist keys stamped by scripts/build-app.sh;
      // commit + built-at land in the credits block below them. Privacy /
      // Terms links open the public trust pages (MoR buyer requirement).
      let info = Bundle.main.infoDictionary ?? [:]
      let commit = info["CSBuildCommit"] as? String ?? "dev"
      let builtAt = info["CSBuiltAt"] as? String ?? "unknown"
      let mono: [NSAttributedString.Key: Any] = [
        .font: NSFont.monospacedSystemFont(ofSize: 11, weight: .regular),
        .foregroundColor: NSColor.secondaryLabelColor,
      ]
      let provenance =
        String(
          localized: "Commit: \(commit)",
          comment: "About panel provenance line; %@ is the source commit of this build"
        ) + "\n"
        + String(
          localized: "Built: \(builtAt)",
          comment: "About panel provenance line; %@ is the build timestamp"
        ) + "\n\n"
      let credits = NSMutableAttributedString(string: provenance, attributes: mono)
      let privacy = NSAttributedString(
        string: String(
          localized: "Privacy Policy",
          comment: "About panel link to the public privacy page"),
        attributes: mono.merging([.link: Self.privacyURL]) { _, new in new }
      )
      let terms = NSAttributedString(
        string: String(
          localized: "Terms of Use & EULA",
          comment: "About panel link to the public terms page; EULA = end-user licence agreement"),
        attributes: mono.merging([.link: Self.termsURL]) { _, new in new }
      )
      credits.append(privacy)
      credits.append(NSAttributedString(string: "\n", attributes: mono))
      credits.append(terms)
      NSApp.orderFrontStandardAboutPanel(options: [.credits: credits])
    }
    model.tray.onHelp = {
      NSWorkspace.shared.open(Self.helpURL)
    }
    model.tray.onCheckForUpdates = { [weak self] in
      self?.updater?.checkForUpdates()
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
      let emptyMessage = String(
        localized: "No transcript to save",
        comment: "Tray banner: there is no recent transcript to append to the daily note")
      self?.saveToNote(tray: model.tray, emptyMessage: emptyMessage) {
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
      let emptyMessage = String(
        localized: "No text selected",
        comment: "Tray banner: no selection was found to append to the daily note")
      self.saveToNote(tray: model.tray, emptyMessage: emptyMessage) {
        if let own = self.harvestAgentWindowSelection() {
          notesLog.info(
            "save selection: harvested \(own.count, privacy: .public) chars from agent window")
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
        let settings: CsSettings?
        do {
          settings = try config.loadDiagnosticSettings()
        } catch {
          // Report unavailability, not raw errors or invented default values.
          settings = nil
        }
        let text = codescribeDebugInfo(
          build: .current(), osVersion: ProcessInfo.processInfo.operatingSystemVersionString,
          recording: recording, settings: settings, lastServing: currentServingVerdict(),
          settingsFile: config.settingsFilePath(), dataDirectory: config.configDir(),
          notesDirectory: notes.notesDir()
        )
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
    guard
      let path = threads.recentHistory(limit: 32)
        .first(where: { $0.kind.isCopyableTranscript })?.path
    else { return nil }
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
        tray.showNoteStatus(
          .init(
            kind: .success,
            message: String(
              localized: "Saved to daily note",
              comment: "Tray banner: the text was appended to today's note")))
      } else {
        notesLog.info("note save: nothing to save")
        tray.showNoteStatus(.init(kind: .failure, message: emptyMessage))
      }
    } catch {
      notesLog.error("note save failed: \(error.localizedDescription, privacy: .public)")
      tray.showNoteStatus(
        .init(
          kind: .failure,
          message: String(
            localized: "Could not save note",
            comment: "Tray banner: writing the daily note failed")))
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

    let handled =
      window.firstResponder?
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

  func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
    guard !shouldExitForDuplicate, !Self.isRunningTests else { return .terminateNow }
    terminationCoordinator.begin(
      cleanup: { [weak self] in await self?.shutdownForTermination() },
      reply: { sender.reply(toApplicationShouldTerminate: true) }
    )
    return .terminateLater
  }

  private func shutdownForTermination() async {
    // The launch guards in applicationShouldTerminate keep the XCTest host
    // from constructing a bridge here solely to stop it at teardown.
    let voiceStop = Task { await VoiceLabRuntime.shared.stopOwnedProcess() }
    trayPanel.dismiss()
    model.chat.invalidate()
    appActionListener?.invalidate()
    voiceDeliveryListener?.invalidate()
    trayStatus.invalidate()
    sleepWakeObserver?.invalidate()
    sleepWakeObserver = nil
    if let textScaleMonitor { NSEvent.removeMonitor(textScaleMonitor) }
    DistributedNotificationCenter.default().removeObserver(self)
    // UniFFI stop calls are synchronous. Keep them off the main actor so the
    // ten-second AppKit reply deadline can fire even if a native stop stalls.
    let hotkeyRuntime = hotkeys
    let nativeStop = Task.detached { [hotkeyRuntime] in
      hotkeyRuntime.stop()
      do {
        let runtime = try shutdownApplicationRuntime()
        appLogger.info(
          "Application runtime stopped with \(runtime.activeTasks, privacy: .public) owned tasks and \(runtime.stoppedWorkerNames.count, privacy: .public) stopped workers"
        )
      } catch {
        appLogger.error(
          "Application runtime shutdown failed: \(error.localizedDescription, privacy: .public)"
        )
      }
    }
    await nativeStop.value
    await voiceStop.value
  }

  /// Bind the active recorder to the real host power lifecycle.
  ///
  /// The observer is object-scoped and coalesced. This closure runs on the
  /// following main-queue tick and only launches the non-creating bridge
  /// query; the provider transition happens in the Rust session loop.
  private func installSystemSleepWakeObserver() {
    let observer = SystemSleepWakeObserver { [weak self] in
      Task { [weak self] in
        guard let self else { return }
        let reachedActiveRecorder = await self.hotkeys.noteSleepWake()
        appLogger.info(
          "Host sleep/wake boundary forwarded to active recorder: \(reachedActiveRecorder, privacy: .public)"
        )
      }
    }
    sleepWakeObserver = observer
    observer.start()
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
        let controller = self.textScaleController(for: NSApp.keyWindow)
      else {
        return event
      }
      switch event.charactersIgnoringModifiers {
      case "+", "=":
        controller.increase()
        return nil
      case "-", "_":
        controller.decrease()
        return nil
      case "0":
        controller.reset()
        return nil
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
    trayStatus.onChange = { [weak self] status in
      guard let self else { return }
      self.model.overlay.handleIndicatorModeChange(status.indicatorMode)
      self.applyStatusItemStatus()
    }
    applyStatusItemStatus()
  }

  private func applyStatusItemStatus() {
    guard let button = statusItem?.button else { return }
    // The glyph never changes with status — it is always the brand mark.
    // Mode is conveyed by the status dot composited into the glyph's
    // bottom-right corner, 1:1 with the tray status feed: green ready /
    // red recording / orange processing / purple assistive.
    button.imagePosition = .imageOnly
    button.title = ""
    if let dot = trayStatus.menuBarDotColor, let base = statusItemImage() {
      // The composite is a flattened, non-template image, so the
      // unread-agent tint rides the glyph tint, not contentTintColor.
      button.image = TrayStatusDotIcon.composite(
        base: base,
        dot: NSColor(dot),
        glyphTint: hasUnreadAgentUpdate ? .systemYellow : nil
      )
      button.contentTintColor = nil
    } else {
      button.image = statusItemImage()
      button.contentTintColor = hasUnreadAgentUpdate ? NSColor.systemYellow : nil
    }
    let tooltip = trayStatus.tooltip
    button.toolTip =
      hasUnreadAgentUpdate
      ? String(
        localized: "\(tooltip) - agent reply ready",
        comment: "Menu bar tooltip while an unread agent reply waits; %@ is the status tooltip")
      : tooltip
  }

  private func statusItemImage() -> NSImage? {
    // Brand mark from Assets.xcassets (template image → auto-tints for
    // light/dark menu bars). Status is signaled by a colored dot composited
    // into this icon's corner, never by swapping the glyph. A missing asset
    // is a build bug to surface (empty item), not something to paper over.
    let image = NSImage(named: "MenuBarIcon")
    image?.isTemplate = true
    return image
  }

  private func ensureAgentWindow() -> NSWindow {
    if let agentWindow { return agentWindow }
    // Wrap in TextScaleRoot so ⌘+/-/0 on the chat window scale the message
    // bodies + composer via `\.csTextScale`, independently of the overlay.
    let root = TextScaleRoot(controller: model.chatTextScale) {
      AgentChatView(store: model.chat, maxPermissions: maxPermissionModel)
    }
    let hosting = NSHostingController(rootView: root)
    let window = NSWindow(contentViewController: hosting)
    window.title = String(localized: "Agent", comment: "Title of the agent chat window")
    window.setContentSize(NSSize(width: 1120, height: 720))
    window.styleMask = [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView]
    window.titlebarAppearsTransparent = true
    window.isReleasedWhenClosed = false
    // LSUIElement accessory: join the active Space so a passive
    // `orderFrontRegardless` is actually visible during voice delivery.
    window.collectionBehavior = [.moveToActiveSpace, .fullScreenAuxiliary]
    window.center()
    agentWindow = window
    return window
  }

  /// Show the agent chat window.
  /// - Parameter activating: when true (tray/menu/summon), steal focus. When
  ///   false (voice TurnStarted / end-of-turn fallback), order front without
  ///   `NSApp.activate` so the user's frontmost app stays frontmost.
  private func showAgent(activating: Bool = true) {
    let window = ensureAgentWindow()
    let revealIntent = AgentRevealPolicy.intent(activating: activating)
    let shouldActivate = AgentRevealPolicy.shouldActivate(for: revealIntent)
    // Keep Space-join behavior even if the window was created earlier on an
    // older build of this method before collectionBehavior was set.
    window.collectionBehavior.formUnion([.moveToActiveSpace, .fullScreenAuxiliary])
    if shouldActivate {
      hasUnreadAgentUpdate = false
      applyStatusItemStatus()
      NSApp.activate(ignoringOtherApps: true)
      window.makeKeyAndOrderFront(nil)
      appLogger.info(
        "w10a_agent_show activating=true isVisible=\(window.isVisible, privacy: .public)"
      )
    } else if AgentRevealPolicy.shouldReorderEvenIfVisible(for: revealIntent) {
      // Passive path: always re-order. Early-return on isVisible alone hid
      // windows that were "visible" on another Space or occluded, so live
      // voice turns never painted until the activating end-of-turn open.
      hasUnreadAgentUpdate = true
      applyStatusItemStatus()
      window.orderFrontRegardless()
      appLogger.info(
        "w10a_agent_show activating=false isVisible=\(window.isVisible, privacy: .public) isKey=\(window.isKeyWindow, privacy: .public)"
      )
    }
  }

  /// Passive reveal for voice→agent delivery. Must never call NSApp.activate.
  /// Structured log line is the runtime receipt anchor: reveal_ts < done_ts.
  private func revealAgentForDelivery() {
    appLogger.info("w10a_reveal_begin")
    showAgent(activating: false)
    appLogger.info("w10a_reveal_done")
  }

  private func showTray() {
    guard let button = statusItem.button else { return }
    trayPanel.present(from: button) {
      TrayMenuView(viewModel: model.tray, trayStatus: trayStatus)
    }
  }

  @objc private func toggleTray() {
    if trayPanel.isVisible {
      trayPanel.dismiss()
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

  private func registerAppActions() {
    let action = agentSummonAction
    let listener = AgentAppActionListener(
      maxApprovalsChanged: { [weak self] in
        guard let self else { return }
        await self.maxPermissionModel.refreshMaxToolApprovals()
        if !self.maxPermissionModel.maxToolApprovals.isEmpty
          || self.maxPermissionModel.maxApprovalError != nil
        {
          self.showAgent(activating: false)
        }
      },
      summonAgent: { [weak action] in
        action?.perform()
        appLogger.info("Agent summon command handled: window fronted and composer focus requested")
      }
    )
    appActionListener = listener
    hotkeys.setAppActionListener(listener: listener)
  }

  private func registerOnDeviceFormatter() {
    let formatter = OnDeviceFormatterHost()
    onDeviceFormatter = formatter
    hotkeys.setOnDeviceFormatter(formatter: formatter)
  }

  private func startHotkeys() {
    Task { [hotkeys] in
      do {
        try await hotkeys.start()
        appLogger.info("Codescribe hotkeys active: \(hotkeys.isActive(), privacy: .public)")
      } catch {
        appLogger.error(
          "Codescribe hotkeys unavailable: \(String(describing: error), privacy: .public)")
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
        appLogger.error(
          "Codescribe recording prewarm failed: \(String(describing: error), privacy: .public)")
      }
    }
  }

  private static var isDuplicateInstance: Bool {
    guard let bundleIdentifier = Bundle.main.bundleIdentifier else { return false }
    let currentPID = ProcessInfo.processInfo.processIdentifier
    return
      NSRunningApplication
      .runningApplications(withBundleIdentifier: bundleIdentifier)
      .contains { app in
        isOtherInstance(
          bundleIdentifier: bundleIdentifier, currentPID: currentPID,
          candidateBundleIdentifier: app.bundleIdentifier,
          candidatePID: app.processIdentifier, isTerminated: app.isTerminated
        )
      }
  }

  static func isOtherInstance(
    bundleIdentifier: String, currentPID: pid_t,
    candidateBundleIdentifier: String?, candidatePID: pid_t, isTerminated: Bool
  ) -> Bool {
    candidateBundleIdentifier == bundleIdentifier && candidatePID != currentPID && !isTerminated
  }
}

/// A bounded diagnostic projection, not a dump of configuration or credentials.
@MainActor
func codescribeDebugInfo(
  build: AppBuildInfo, osVersion: String, recording: Bool, settings: CsSettings?,
  lastServing: CsLastServingVerdict?, settingsFile: String,
  dataDirectory: String, notesDirectory: String
) -> String {
  var lines = [
    "codescribe debug info",
    "app version: \(build.version) (\(build.build))",
    "source commit: \(build.commit)",
    "built at: \(build.builtAt)",
    "macOS: \(osVersion)",
    "recording: \(recording)",
    "settings file: \(settingsFile)",
    "app data dir: \(dataDirectory)",
    "notes dir: \(notesDirectory)",
  ]
  if let settings {
    lines += [
      "configuration: resolved now; may include loader repairs; not proof of the active capture snapshot",
      "configured ASR mode: \(settings.asrMode ?? "not specified")",
      "configured input device: \(settings.audioInputDevice ?? "system default")",
      "formatting enabled: \(settings.aiFormattingEnabled)",
      "configured formatting policy: \(settings.formattingLevel ?? "not specified")",
      "configured formatting provider: \(settings.llmFormattingProvider ?? "not specified")",
      "configured formatting model: \(settings.llmFormattingModel ?? "not specified")",
      "configured agent provider: \(settings.llmAssistiveProvider ?? "not specified")",
      "configured agent model: \(settings.llmAssistiveModel ?? "not specified")",
    ]
  } else {
    lines.append(
      "configuration: unavailable; loader refused or a configuration refusal was recorded in this process"
    )
  }
  if let lastServing {
    lines.append("last completed serving engine: \(lastServing.engine)")
    lines.append("last serving disposition: \(lastServing.disposition ?? "not reported")")
    lines.append("last serving used alternate engine: \(lastServing.fallbackUsed)")
  } else {
    lines.append("last completed serving engine: not yet observed in this process")
  }
  lines.append("Contains local paths and configuration labels; review before sharing.")
  return lines.joined(separator: "\n")
}
