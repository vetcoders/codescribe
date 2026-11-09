import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// On-demand screen atlas: the production windows of Overlay, Agent, Settings,
/// Onboarding and the Tray menu, each in its states, in Light and Dark, with a
/// manifest and an `index.html` gallery. `scripts/app-screen-atlas.sh` is the
/// one command that runs it; `macos/CodescribeTests/README.md` describes it.
///
/// Runs only on request: without CODESCRIBE_SCREEN_ATLAS_DIR every method
/// skips, so the ordinary suite stays fast.
///
/// Two outputs per cell:
/// - Offscreen bitmap (always): drawn by AppKit from the window's frame view,
///   not by the compositor. It proves layout, proportions, typography and ink,
///   not Liquid Glass, vibrancy or Dark-mode glass.
/// - Live capture (CODESCRIBE_SCREEN_ATLAS_LIVE=1): the window is ordered
///   front and an outside capturer holding Screen Recording takes
///   `screencapture -l` of it. This is the compositor truth. The test host
///   cannot activate, so every live window has the inactive appearance.
///
/// Every store and model is synthetic: no engine, no Keychain, no settings
/// write, no permission prompt.
@MainActor
final class AppScreenAtlasTests: XCTestCase {
  private static let agentSidebarKey = "AgentChat.sidebarExpanded.v1"

  // MARK: - Overlay

  func testOverlayAtlas() throws {
    let atlas = try ScreenAtlas.open(surface: .overlay)
    let states: [(String, () -> OverlayState)] = [
      ("listening", { OverlayState.previewListening() }),
      ("listening-evidence", { OverlayState.previewListeningWithEvidence() }),
      ("transcribing", { OverlayState.previewTranscribing() }),
      ("no-speech", { OverlayState.previewNoSpeech() }),
      ("formatted", { OverlayState.previewFormatted() }),
      ("error", { OverlayState.previewError() }),
      (
        "mini-listening",
        {
          let state = OverlayState.previewListening()
          state.setPresentationMode(.mini)
          return state
        }
      ),
    ]
    for scheme in AtlasScheme.allCases {
      for (name, makeState) in states {
        try renderOverlay(
          atlas, state: makeState(), name: name, scheme: scheme,
          expanded: !name.hasPrefix("mini"))
      }
    }
    atlas.note(
      "Overlay panels come from DictationOverlayWindow.make with isolated size defaults; "
        + "the panel builds its own SwiftUI root, so the scheme is applied through NSAppearance only."
    )
    try atlas.finish()
  }

  private func renderOverlay(
    _ atlas: ScreenAtlas, state: OverlayState, name: String, scheme: AtlasScheme,
    expanded: Bool
  ) throws {
    let suite = "AppScreenAtlasTests.overlay.\(UUID().uuidString)"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { defaults.removePersistentDomain(forName: suite) }
    let panel = try XCTUnwrap(
      DictationOverlayWindow.make(
        state: state,
        textScale: TextScaleController(key: "AppScreenAtlasTests.overlay.scale"),
        reduceTransparencyOverride: false,
        contrastOverride: .standard,
        sizeDefaults: defaults
      ) as? FloatingOverlayPanel)
    defer { BackgroundTestWindow.tearDown(panel, state: state) }
    panel.contentView?.appearance = scheme.appearance
    panel.appearance = scheme.appearance
    if expanded {
      panel.setContentSize(DictationOverlayWindow.defaultSize)
    }
    try atlas.capture(
      panel, id: "overlay-\(name)-\(scheme.rawValue)", state: name, scheme: scheme)
  }

  // MARK: - Agent

  func testAgentAtlas() throws {
    let atlas = try ScreenAtlas.open(surface: .agent)
    let defaults = UserDefaults.standard
    let previousSidebar = defaults.object(forKey: Self.agentSidebarKey)
    defer { restore(previousSidebar, forKey: Self.agentSidebarKey, in: defaults) }
    let suite = "AppScreenAtlasTests.agent.\(UUID().uuidString)"
    let persistence = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { persistence.removePersistentDomain(forName: suite) }

    let widths: [(String, CGFloat, CGFloat)] = [
      ("floor", AgentWindowMetrics.minWidth, AgentWindowMetrics.minHeight),
      ("standard", 1120, 720),
    ]
    for scheme in AtlasScheme.allCases {
      for (widthName, width, height) in widths {
        // Empty: a fresh conversation, the first thing a new person sees.
        defaults.set(true, forKey: Self.agentSidebarKey)
        try renderAgent(
          atlas, store: emptyAgentStore(persistence), state: "empty-\(widthName)-rail-expanded",
          size: NSSize(width: width, height: height), card: false, scheme: scheme)
        for expanded in [true, false] {
          defaults.set(expanded, forKey: Self.agentSidebarKey)
          let rail = expanded ? "rail-expanded" : "rail-collapsed"
          for card in [false, true] {
            try renderAgent(
              atlas, store: conversationAgentStore(persistence),
              state: "conversation-\(widthName)-\(rail)\(card ? "-card" : "")",
              size: NSSize(width: width, height: height), card: card, scheme: scheme)
          }
        }
      }
    }
    atlas.note(
      "Agent windows come from AgentWindowChrome.makeWindow with AgentChatView; "
        + "threads are fixtures without an engine, and the rail key is restored afterwards. "
        + "Floor = AgentWindowMetrics.minWidth x minHeight; standard = 1120 x 720."
    )
    try atlas.finish()
  }

  private func renderAgent(
    _ atlas: ScreenAtlas, store: AgentChatStore, state: String, size: NSSize, card: Bool,
    scheme: AtlasScheme
  ) throws {
    let presenter = AgentContextCardPresenter()
    let hosting = NSHostingController(
      rootView: AgentChatView(store: store, contextCard: presenter)
        .atlasEnvironment(scheme))
    // Resolve the appearance before SwiftUI mounts: dynamic NSColor and the
    // header accessory read it from the view tree, not from the environment.
    hosting.view.appearance = scheme.appearance
    let window = AgentWindowChrome.makeWindow(content: hosting)
    window.appearance = scheme.appearance
    window.setContentSize(size)
    defer {
      presenter.close()
      window.orderOut(nil)
      window.close()
    }
    try atlas.capture(
      window, id: "agent-\(state)-\(scheme.rawValue)", state: state, scheme: scheme,
      prepare: card ? { presenter.open() } : nil)
  }

  private func emptyAgentStore(_ persistence: UserDefaults) -> AgentChatStore {
    AgentChatStore(
      engine: nil,
      threads: [ChatThread(title: AgentChatStore.newThreadTitle, meta: "")],
      persistenceDefaults: persistence)
  }

  /// A conversation with several turns plus two older threads, so the rail
  /// shows grouping and the reading column wraps long replies.
  private func conversationAgentStore(_ persistence: UserDefaults) -> AgentChatStore {
    let stamp = Date(timeIntervalSince1970: 1_794_200_000)
    let messages = [
      ChatMessage(
        role: .you, timestamp: stamp,
        text: "Przygotuj krótki plan wdrożenia nowego panelu kontekstu."),
      ChatMessage(
        role: .assistant, timestamp: stamp,
        text:
          "Plan ma trzy kroki: zastąpić kolumnę kartą otwieraną z nagłówka, oddać rozmowie odzyskaną szerokość i sprawdzić układ w jasnym i ciemnym motywie, przy minimalnej i standardowej szerokości okna."
      ),
      ChatMessage(role: .you, timestamp: stamp, text: "Co z dostępnością?"),
      ChatMessage(
        role: .assistant, timestamp: stamp,
        text:
          "Karta zamyka się Escape i kliknięciem obok, a focus wraca tam, gdzie był. Fonty pozostają w skali systemu."
      ),
      ChatMessage(role: .you, timestamp: stamp, text: "A szerokość minimalna okna?"),
      ChatMessage(
        role: .assistant, timestamp: stamp,
        text:
          "Podłoga okna się nie zmienia: karta pływa nad rozmową, więc jej otwarcie nigdy nie zmienia rozmiaru okna."
      ),
    ]
    var current = ChatThread(
      title: "Redesign panelu kontekstu", meta: "", isRestored: false, messagesLoaded: true,
      messages: messages)
    current.model = "claude-opus-5-5"
    current.updatedAt = stamp
    var older = ChatThread(
      title: "Przegląd skrótów klawiszowych", meta: "", messagesLoaded: true,
      messages: [ChatMessage(role: .you, timestamp: stamp, text: "Sprawdź skróty.")])
    older.updatedAt = stamp.addingTimeInterval(-86_400)
    var oldest = ChatThread(
      title: "Notatki z dyktowania", meta: "", messagesLoaded: true,
      messages: [ChatMessage(role: .you, timestamp: stamp, text: "Zbierz notatki.")])
    oldest.updatedAt = stamp.addingTimeInterval(-86_400 * 9)
    return AgentChatStore(
      engine: nil, threads: [current, older, oldest], persistenceDefaults: persistence)
  }

  // MARK: - Settings

  func testSettingsAtlas() throws {
    let atlas = try ScreenAtlas.open(surface: .settings)
    // The scene's default size, and the floor the columns carry: the 216 pt
    // sidebar plus `SettingsView.detailMinWidth`, at the view's 620 pt height.
    let standard = NSSize(width: 1000, height: 720)
    let floor = NSSize(
      width: CSSidebarWidth.navigation.ideal + SettingsView.detailMinWidth, height: 620)
    for scheme in AtlasScheme.allCases {
      for section in SettingsSection.allCases {
        try renderSettings(
          atlas, section: section, sizeName: "standard", size: standard, scheme: scheme)
      }
      try renderSettings(atlas, section: .creator, sizeName: "floor", size: floor, scheme: scheme)
    }
    atlas.note(
      "Settings is a SwiftUI Window scene in the app; here SettingsView(model: .preview(section)) "
        + "is hosted in a unified-toolbar NSWindow through NSHostingController, as "
        + "CSSidebarMaterialTests does. "
        + "Standard = the scene's 1000 x 720 default; floor = sidebar ideal + detailMinWidth x 620."
    )
    try atlas.finish()
  }

  private func renderSettings(
    _ atlas: ScreenAtlas, section: SettingsSection, sizeName: String, size: NSSize,
    scheme: AtlasScheme
  ) throws {
    let hosting = NSHostingController(
      rootView: SettingsView(model: .preview(section)).atlasEnvironment(scheme))
    hosting.view.appearance = scheme.appearance
    let window = NSWindow(contentViewController: hosting)
    window.isReleasedWhenClosed = false
    // The scene's `.windowToolbarStyle(.unified)`, which a bare window lacks.
    window.toolbarStyle = .unified
    window.appearance = scheme.appearance
    window.setContentSize(size)
    defer {
      window.orderOut(nil)
      window.close()
    }
    let state = "\(section.rawValue)-\(sizeName)"
    try atlas.capture(
      window, id: "settings-\(state)-\(scheme.rawValue)", state: state, scheme: scheme)
  }

  // MARK: - Onboarding

  func testOnboardingAtlas() throws {
    let atlas = try ScreenAtlas.open(surface: .onboarding)
    let suite = "AppScreenAtlasTests.onboarding.\(UUID().uuidString)"
    let preferences = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { preferences.removePersistentDomain(forName: suite) }
    let missing = PermissionSnapshot(
      microphone: .granted, accessibility: .denied, inputMonitoring: .notDetermined,
      screenRecording: .notDetermined, speechRecognition: .notDetermined)
    for scheme in AtlasScheme.allCases {
      for (index, step) in OnboardingStep.flow.enumerated() {
        // The Agentic Readiness step is shown only on the agentic lane.
        let agentic = step == .agenticReadiness
        try renderOnboarding(
          atlas, index: index, state: "\(index + 1)-\(stepName(step))",
          agentic: agentic, permissions: .allGranted, preferences: preferences, scheme: scheme)
      }
      let permissionsIndex = try XCTUnwrap(OnboardingStep.flow.firstIndex(of: .permissions))
      try renderOnboarding(
        atlas, index: permissionsIndex, state: "\(permissionsIndex + 1)-permissions-missing",
        agentic: false, permissions: missing, preferences: preferences, scheme: scheme)
    }
    atlas.note(
      "OnboardingWindowController builds its model from the real engines, so the atlas builds the "
        + "same window (NSHostingController, 720 x 620, titled/closable/fullSizeContentView, "
        + "transparent titlebar, clear background) around an OnboardingViewModel on mock engines "
        + "and an offline local-model store. Basic lane except Agentic Readiness."
    )
    try atlas.finish()
  }

  private func renderOnboarding(
    _ atlas: ScreenAtlas, index: Int, state: String, agentic: Bool,
    permissions: PermissionSnapshot, preferences: UserDefaults, scheme: AtlasScheme
  ) throws {
    let engine = MockOnboardingEngine(progress: UInt32(index))
    engine.mode = agentic ? "agentic" : "basic"
    let model = OnboardingViewModel(
      engine: engine, hotkeys: MockHotkeysEngine(), agentStatus: MockAgentStatusEngine(),
      agentBridge: AtlasOfflineBridge(), probe: MockPermissionProbe(permissions),
      whisperDownloadStore: WhisperDownloadStore(
        statusProvider: { .sampleUnavailable },
        download: { _ in throw CancellationError() }),
      languagePreferences: preferences)
    XCTAssertEqual(model.stepIndex, index, state)
    let hosting = NSHostingController(
      rootView: OnboardingView(model: model).atlasEnvironment(scheme))
    hosting.sizingOptions = []
    hosting.view.appearance = scheme.appearance
    let window = NSWindow(contentViewController: hosting)
    window.title = model.windowTitle
    window.setContentSize(NSSize(width: 720, height: 620))
    window.styleMask = [.titled, .closable, .fullSizeContentView]
    window.titlebarAppearsTransparent = true
    window.isOpaque = false
    window.backgroundColor = .clear
    window.isReleasedWhenClosed = false
    window.appearance = scheme.appearance
    defer {
      window.orderOut(nil)
      window.close()
    }
    try atlas.capture(
      window, id: "onboarding-\(state)-\(scheme.rawValue)", state: state, scheme: scheme)
    XCTAssertEqual(model.stepIndex, index, "rendering must not advance setup: \(state)")
  }

  private func stepName(_ step: OnboardingStep) -> String {
    switch step {
    case .interfaceLanguage: "interface-language"
    case .mode: "mode"
    case .permissions: "permissions"
    case .language: "language"
    case .localModel: "local-model"
    case .apiKey: "api-key"
    case .hotkeyMode: "hotkey-mode"
    case .agenticReadiness: "agentic-readiness"
    case .done: "done"
    }
  }

  // MARK: - Tray

  func testTrayAtlas() throws {
    let atlas = try ScreenAtlas.open(surface: .tray)
    #if DEBUG
      let states: [(String, Bool, CsTrayStatusKind, CsTrayStatusTone)] = [
        ("idle", false, .idle, .neutral),
        ("recording", true, .listening, .active),
        ("error", false, .error, .critical),
      ]
      for scheme in AtlasScheme.allCases {
        for (name, recording, kind, tone) in states {
          try renderTray(
            atlas, name: name, recording: recording,
            status: TrayStatusStore.preview(kind: kind, tone: tone), scheme: scheme)
        }
      }
      atlas.note(
        "Tray menus are TrayPanelSurface + TrayMenuView in the production TrayPanel, sized to the "
          + "reported menu height plus the shadow margin; no status item anchors them.")
    #else
      atlas.note("tray cells omitted: TrayStatusStore.preview is DEBUG-only")
    #endif
    try atlas.finish()
  }

  #if DEBUG
    private func renderTray(
      _ atlas: ScreenAtlas, name: String, recording: Bool, status: TrayStatusStore,
      scheme: AtlasScheme
    ) throws {
      let model = TrayViewModel(
        engine: MockTrayEngine(recording: recording), isRecording: recording)
      model.onQuit = {}
      model.onHelp = {}
      model.onAbout = {}
      model.onIntent = { _ in }
      model.onDictationStartRequested = {}
      model.onSaveLastTranscript = {}
      model.onSaveSelection = {}
      model.onOpenNotesFolder = {}
      model.onOpenTodayNote = {}
      model.onOpenLogFolder = {}
      model.onCopyDebugInfo = {}
      let height = AtlasHeight()
      let hosting = NSHostingController(
        rootView: TrayPanelSurface(content: TrayMenuView(viewModel: model, trayStatus: status)) {
          height.value = $0
        }
        .atlasEnvironment(scheme))
      hosting.sizingOptions = []
      hosting.view.appearance = scheme.appearance
      let panel = TrayPanel()
      panel.appearance = scheme.appearance
      panel.contentViewController = hosting
      let margin = TrayPanel.shadowMargin * 2
      panel.setContentSize(NSSize(width: 300 + margin, height: 460 + margin))
      defer { panel.dismiss() }
      try atlas.capture(
        panel, id: "tray-\(name)-\(scheme.rawValue)", state: name, scheme: scheme,
        prepare: {
          if height.value > 0 {
            panel.setContentSize(
              NSSize(width: 300 + margin, height: height.value.rounded(.up) + margin))
          }
        })
    }
  #endif

  private func restore(_ previous: Any?, forKey key: String, in defaults: UserDefaults) {
    if let previous {
      defaults.set(previous, forKey: key)
    } else {
      defaults.removeObject(forKey: key)
    }
  }
}

// MARK: - Atlas plumbing

private final class AtlasHeight {
  var value: CGFloat = 0
}

private enum AtlasScheme: String, CaseIterable, Codable {
  case light
  case dark

  var appearance: NSAppearance {
    NSAppearance(named: self == .dark ? .darkAqua : .aqua)!
  }

  var colorScheme: ColorScheme { self == .dark ? .dark : .light }
}

private enum AtlasSurface: String, CaseIterable, Codable {
  case overlay = "Overlay"
  case agent = "Agent"
  case settings = "Settings"
  case onboarding = "Onboarding"
  case tray = "Tray"
}

extension View {
  fileprivate func atlasEnvironment(_ scheme: AtlasScheme) -> some View {
    self
      .environment(\.csReduceTransparencyOverride, false)
      .environment(\.csContrastOverride, .standard)
      .environment(\.colorScheme, scheme.colorScheme)
      .preferredColorScheme(scheme.colorScheme)
      .transaction { $0.disablesAnimations = true }
  }
}

/// In-test agent bridge status: the real installer probes its payload on disk.
private struct AtlasOfflineBridge: AgentBridgeInstalling {
  func status() -> AgentBridgeInstallationStatus {
    AgentBridgeInstallationStatus(
      payloadAvailable: true, bundleVersion: nil, installedClients: [], installedPaths: [],
      detail: "atlas fixture")
  }

  func install(selectedClients _: Set<AgentBridgeClient>) throws -> AgentBridgeInstallationStatus {
    throw AgentBridgeInstallationError.payloadUnavailable
  }

  func adoptManualSkill(client _: AgentBridgeClient) throws -> AgentBridgeAdoptionResult {
    throw AgentBridgeInstallationError.payloadUnavailable
  }
}

private struct AtlasSize: Codable, Equatable {
  var width: Double
  var height: Double
}

private struct AtlasCell: Codable, Equatable {
  var id: String
  var surface: String
  var state: String
  var scheme: String
  var size: AtlasSize
  var offscreen: String
  var live: String?
  var notes: [String]
}

private struct AtlasManifest: Codable {
  var generatedAt: String
  var language: String
  var offscreenMeaning: String
  var liveMeaning: String
  var surfaceNotes: [String: [String]]
  var cells: [AtlasCell]
}

/// One test method's run: renders cells into `<dir>/offscreen` (and `<dir>/live`
/// when live capture is on), then merges them into `<dir>/manifest.json` and
/// rewrites `<dir>/index.html`, so surfaces run one at a time accumulate.
@MainActor
private final class ScreenAtlas {
  let directory: URL
  let surface: AtlasSurface
  let live: Bool
  private var cells: [AtlasCell] = []
  private var notes: [String] = []
  /// A locked screen or a missing capturer fails every request alike; after
  /// the first miss the rest of the process stops waiting 20 s per cell.
  private static var liveUnavailable = false

  private init(directory: URL, surface: AtlasSurface, live: Bool) {
    self.directory = directory
    self.surface = surface
    self.live = live
  }

  static func open(surface: AtlasSurface) throws -> ScreenAtlas {
    let environment = ProcessInfo.processInfo.environment
    guard let path = environment["CODESCRIBE_SCREEN_ATLAS_DIR"],
      !path.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    else { throw XCTSkip("The app screen atlas runs only on request") }
    let directory = URL(fileURLWithPath: path, isDirectory: true)
    let live = environment["CODESCRIBE_SCREEN_ATLAS_LIVE"] == "1"
    let manager = FileManager.default
    try manager.createDirectory(
      at: directory.appendingPathComponent("offscreen"), withIntermediateDirectories: true)
    if live {
      try manager.createDirectory(
        at: directory.appendingPathComponent("live"), withIntermediateDirectories: true)
    }
    return ScreenAtlas(directory: directory, surface: surface, live: live)
  }

  func note(_ text: String) { notes.append(text) }

  /// Presents the window under the suite's background policy, settles it,
  /// runs `prepare` (and settles again), then writes the offscreen bitmap and,
  /// when requested, the live capture.
  func capture(
    _ window: NSWindow, id: String, state: String, scheme: AtlasScheme,
    prepare: (() -> Void)? = nil
  ) throws {
    window.setFrameOrigin(NSPoint(x: 80, y: 80))
    BackgroundTestWindow.present(window)
    settle(window)
    if let prepare {
      prepare()
      settle(window)
    }
    var cellNotes: [String] = []
    let resolved = window.effectiveAppearance.bestMatch(from: [.aqua, .darkAqua])
    if resolved != scheme.appearance.name {
      cellNotes.append("window resolved \(resolved?.rawValue ?? "nil"), not \(scheme.rawValue)")
    }
    var livePath: String?
    if live {
      if let captured = captureLive(window, name: "\(id).png") {
        livePath = captured
      } else {
        cellNotes.append("no live capture")
      }
    }
    let offscreen = "offscreen/\(id).png"
    try writeOffscreen(window, to: directory.appendingPathComponent(offscreen))
    let size = window.frame.size
    cells.append(
      AtlasCell(
        id: id, surface: surface.rawValue, state: state, scheme: scheme.rawValue,
        size: AtlasSize(width: Double(size.width), height: Double(size.height)),
        offscreen: offscreen, live: livePath, notes: cellNotes))
  }

  private func writeOffscreen(_ window: NSWindow, to url: URL) throws {
    let frameView = try XCTUnwrap(window.contentView?.superview, "no window frame view")
    frameView.layoutSubtreeIfNeeded()
    let bitmap = try XCTUnwrap(
      frameView.bitmapImageRepForCachingDisplay(in: frameView.bounds), "no bitmap")
    window.effectiveAppearance.performAsCurrentDrawingAppearance {
      frameView.cacheDisplay(in: frameView.bounds, to: bitmap)
    }
    let png = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]), "no png")
    try png.write(to: url)
  }

  /// Compositor evidence: show the window without activating the app, ask the
  /// outside capturer (`scripts/app-screen-atlas.sh`) for `screencapture -l`
  /// by writing "<windowNumber> <name>" to `live/request`, and wait for the file.
  private func captureLive(_ window: NSWindow, name: String) -> String? {
    guard !Self.liveUnavailable else { return nil }
    let liveDirectory = directory.appendingPathComponent("live")
    let result = liveDirectory.appendingPathComponent(name)
    try? FileManager.default.removeItem(at: result)
    window.alphaValue = 1
    window.orderFrontRegardless()
    let deadline = Date().addingTimeInterval(1.5)
    while Date() < deadline {
      RunLoop.current.run(until: Date().addingTimeInterval(0.05))
      window.displayIfNeeded()
    }
    do {
      try "\(window.windowNumber) \(name)\n".write(
        to: liveDirectory.appendingPathComponent("request"), atomically: true, encoding: .utf8)
    } catch {
      Self.liveUnavailable = true
      return nil
    }
    let wait = Date().addingTimeInterval(20)
    while Date() < wait, !FileManager.default.fileExists(atPath: result.path) {
      RunLoop.current.run(until: Date().addingTimeInterval(0.1))
    }
    window.alphaValue = 0
    guard FileManager.default.fileExists(atPath: result.path) else {
      Self.liveUnavailable = true
      return nil
    }
    return "live/\(name)"
  }

  private func settle(_ window: NSWindow) {
    let deadline = Date().addingTimeInterval(1.2)
    while Date() < deadline {
      RunLoop.current.run(until: Date().addingTimeInterval(0.03))
      window.contentView?.layoutSubtreeIfNeeded()
      window.displayIfNeeded()
    }
  }

  // MARK: Manifest and gallery

  func finish() throws {
    let manifestURL = directory.appendingPathComponent("manifest.json")
    var manifest: AtlasManifest
    if let data = try? Data(contentsOf: manifestURL),
      let existing = try? JSONDecoder().decode(AtlasManifest.self, from: data)
    {
      manifest = existing
    } else {
      manifest = AtlasManifest(
        generatedAt: "", language: "",
        offscreenMeaning:
          "AppKit drawing of the window frame view: layout, proportions, typography and ink. Not the compositor: no Liquid Glass, vibrancy or Dark-mode glass.",
        liveMeaning:
          "screencapture -l of the ordered-front window by an outside process holding Screen Recording: compositor truth, inactive-window appearance.",
        surfaceNotes: [:], cells: [])
    }
    manifest.generatedAt = ISO8601DateFormatter().string(from: Date())
    manifest.language = Bundle.main.preferredLocalizations.first ?? "unknown"
    manifest.surfaceNotes[surface.rawValue] = notes
    // A rerun of this surface replaces its cells; other surfaces stay.
    manifest.cells.removeAll { $0.surface == surface.rawValue }
    manifest.cells.append(contentsOf: cells)
    let order = AtlasSurface.allCases.map(\.rawValue)
    manifest.cells =
      AtlasSurface.allCases.flatMap { surface in
        manifest.cells.filter { $0.surface == surface.rawValue }
      } + manifest.cells.filter { !order.contains($0.surface) }
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
    try encoder.encode(manifest).write(to: manifestURL)
    try Self.gallery(manifest).write(
      to: directory.appendingPathComponent("index.html"), atomically: true, encoding: .utf8)
    print("app-screen-atlas: \(surface.rawValue) \(cells.count) cells -> \(directory.path)")
  }

  private static func gallery(_ manifest: AtlasManifest) -> String {
    var body = ""
    for surface in AtlasSurface.allCases {
      let cells = manifest.cells.filter { $0.surface == surface.rawValue }
      guard !cells.isEmpty || manifest.surfaceNotes[surface.rawValue] != nil else { continue }
      body += "<section id=\"\(surface.rawValue.lowercased())\"><h2>\(surface.rawValue)"
      body += " <small>\(cells.count) cells</small></h2>\n"
      for note in manifest.surfaceNotes[surface.rawValue] ?? [] {
        body += "<p class=\"note\">\(escape(note))</p>\n"
      }
      var states: [String] = []
      for cell in cells where !states.contains(cell.state) { states.append(cell.state) }
      for state in states {
        body += "<div class=\"state\"><h3>\(escape(state))</h3><div class=\"pair\">\n"
        for scheme in AtlasScheme.allCases {
          guard
            let cell = cells.first(where: { $0.state == state && $0.scheme == scheme.rawValue })
          else {
            body +=
              "<figure class=\"missing\"><figcaption>\(scheme.rawValue): missing</figcaption></figure>\n"
            continue
          }
          body += figure(cell)
        }
        body += "</div></div>\n"
      }
      body += "</section>\n"
    }
    let nav = AtlasSurface.allCases.map {
      "<a href=\"#\($0.rawValue.lowercased())\">\($0.rawValue)</a>"
    }.joined(separator: " · ")
    return """
      <!doctype html>
      <html lang="en"><head><meta charset="utf-8">
      <meta name="viewport" content="width=device-width, initial-scale=1">
      <title>Codescribe screen atlas</title>
      <style>
      :root { color-scheme: light dark; --bg: #f4f4f6; --fg: #1d1d1f; --muted: #6e6e73;
        --card: #ffffff; --line: #d2d2d7; --badge: #b25000; --badge-fg: #ffffff; }
      @media (prefers-color-scheme: dark) { :root { --bg: #161618; --fg: #f2f2f4;
        --muted: #a1a1a6; --card: #232326; --line: #3a3a3e; --badge: #ff9f0a; --badge-fg: #1d1d1f; } }
      body { margin: 0; padding: 16px 24px 48px; background: var(--bg); color: var(--fg);
        font: 14px/1.45 -apple-system, BlinkMacSystemFont, "Helvetica Neue", sans-serif; }
      header p, .note { color: var(--muted); max-width: 72em; }
      nav { position: sticky; top: 0; background: var(--bg); padding: 8px 0;
        border-bottom: 1px solid var(--line); z-index: 1; }
      nav a { color: inherit; }
      h2 { margin-top: 40px; } h2 small, figcaption small { color: var(--muted); font-weight: normal; }
      .state { margin: 24px 0; }
      h3 { font: 600 13px ui-monospace, Menlo, monospace; margin: 0 0 8px; }
      .pair { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 16px; }
      @media (max-width: 800px) { .pair { grid-template-columns: 1fr; } }
      figure { margin: 0; background: var(--card); border: 1px solid var(--line);
        border-radius: 10px; padding: 10px; position: relative; }
      figure img { max-width: 100%; height: auto; display: block; margin: 0 auto;
        background: repeating-conic-gradient(#8884 0% 25%, transparent 0% 50%) 50% / 16px 16px; }
      figcaption { margin-top: 6px; font-size: 12px; }
      .badge { position: absolute; top: 14px; right: 14px; background: var(--badge);
        color: var(--badge-fg); font: 600 11px -apple-system, sans-serif; padding: 2px 8px;
        border-radius: 999px; }
      .missing { color: var(--muted); }
      </style></head><body>
      <header><h1>Codescribe screen atlas</h1>
      <p>Generated \(escape(manifest.generatedAt)) · interface language \(escape(manifest.language)) · \(manifest.cells.count) cells.</p>
      <p><strong>live</strong>: \(escape(manifest.liveMeaning))<br>
      <strong>offscreen</strong>: \(escape(manifest.offscreenMeaning))</p>
      <p>States are synthetic fixtures, not a live engine.</p></header>
      <nav>\(nav)</nav>
      \(body)
      </body></html>

      """
  }

  private static func figure(_ cell: AtlasCell) -> String {
    let source = cell.live ?? cell.offscreen
    let badge = cell.live == nil ? "<span class=\"badge\">offscreen</span>" : ""
    var links = "<a href=\"\(cell.offscreen)\">offscreen</a>"
    if let live = cell.live { links = "<a href=\"\(live)\">live</a> · " + links }
    let notes = cell.notes.isEmpty ? "" : " · " + escape(cell.notes.joined(separator: "; "))
    return """
      <figure>\(badge)<a href="\(source)"><img src="\(source)" alt="\(escape(cell.id))" loading="lazy"></a>
      <figcaption>\(cell.scheme) · <small>\(cell.id) · \(Int(cell.size.width))×\(Int(cell.size.height)) · \(links)\(notes)</small></figcaption></figure>

      """
  }

  private static func escape(_ text: String) -> String {
    text.replacingOccurrences(of: "&", with: "&amp;")
      .replacingOccurrences(of: "<", with: "&lt;")
      .replacingOccurrences(of: ">", with: "&gt;")
      .replacingOccurrences(of: "\"", with: "&quot;")
  }
}
