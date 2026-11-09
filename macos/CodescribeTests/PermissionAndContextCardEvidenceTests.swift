import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

// Offscreen review bitmaps of the permission rows (Setup and Settings ›
// General) and the compact Context card, for the before/after comparison of
// the x9eval integration (2026-11-09). Unordered windows with an explicit
// aqua or darkAqua appearance; nothing is ordered front, no TCC is touched
// (the probe and the request client are synthetic).
//
// The bitmaps prove layout, wrapping, ink and approximate colour. They do not
// prove hover, focus, VoiceOver, the compositor or Liquid Glass; those remain
// the installed-candidate checks.
//
// Output: CODESCRIBE_VISUAL_EVIDENCE_DIR (TEST_RUNNER_CODESCRIBE_VISUAL_EVIDENCE_DIR
// through xcodebuild), otherwise a temporary directory. Run once per
// `-testLanguage`: Settings and the card resolve copy from the process language.
@MainActor
final class PermissionAndContextCardEvidenceTests: XCTestCase {
  /// The state of the smoke review: a final microphone refusal, Accessibility
  /// requested and still missing, Input Monitoring and Screen Recording
  /// missing, Speech granted, the optional ones undecided.
  private static let mixed = PermissionSnapshot(
    microphone: .denied, accessibility: .denied, inputMonitoring: .denied,
    screenRecording: .denied, speechRecognition: .granted, fullDiskAccess: .notDetermined,
    spotifyAutomation: .notDetermined)

  func testRenderPermissionRowsAndContextCard() throws {
    let directory = try evidenceDirectory()
    let language = Bundle.main.preferredLocalizations.first ?? "en"
    var written: [String] = []
    for dark in [false, true] {
      let scheme = dark ? "dark" : "light"

      let wizard = onboardingModel(language: language)
      wizard.grantPermission(for: .accessibility, client: PermissionRequestFixture())
      spin { !wizard.pendingPermissionRequests.contains(.accessibility) }
      written.append(
        try render(
          "onboarding-permissions-\(language)-\(scheme)", dark: dark,
          size: NSSize(width: 600, height: 1180),
          PermissionsStepView(model: wizard).frame(width: 560).padding(20)
            .environment(\.locale, wizard.interfaceLocale)))

      let settings = settingsModel()
      settings.notePermissionRequested(.accessibility)
      for width in [664, 900] {
        written.append(
          try render(
            "settings-general-\(width)-\(language)-\(scheme)", dark: dark,
            size: NSSize(width: CGFloat(width), height: 2400),
            CreatorPanel(model: settings).frame(width: CGFloat(width))))
      }

      for (name, size) in [
        ("narrow", NSSize(width: 300, height: 420)),
        ("standard", NSSize(width: 760, height: 420)),
        ("short", NSSize(width: 760, height: 170)),
      ] {
        let card = AgentContextCardPresenter()
        card.open()
        written.append(
          try render(
            "context-card-\(name)-\(language)-\(scheme)", dark: dark, size: size,
            CSColor.sheet.overlay {
              AgentContextCardLayer(store: cardStore(), presenter: card, openToolSettings: {})
            }))
      }
    }
    try (written.joined(separator: "\n") + "\n").write(
      to: directory.appendingPathComponent("index-\(language).txt"), atomically: true,
      encoding: .utf8)
    XCTAssertEqual(written.count, 12)
  }

  // MARK: - Fixtures

  private func onboardingModel(language: String) -> OnboardingViewModel {
    let suite = "codescribe.evidence.\(UUID().uuidString)"
    let preferences = UserDefaults(suiteName: suite)!
    preferences.set([language], forKey: "AppleLanguages")
    addTeardownBlock { UserDefaults().removePersistentDomain(forName: suite) }
    return OnboardingViewModel(
      engine: MockOnboardingEngine(), hotkeys: MockHotkeysEngine(),
      agentStatus: MockAgentStatusEngine(), agentBridge: EvidenceOfflineBridge(),
      probe: MockPermissionProbe(Self.mixed), languagePreferences: preferences,
      preferredLanguages: [language],
      processInterfaceLanguage: language.hasPrefix("pl") ? .polish : .english)
  }

  private func settingsModel() -> SettingsViewModel {
    SettingsViewModel(
      creatorAgentBridge: EvidenceOfflineBridge(),
      permissionProbe: MockPermissionProbe(Self.mixed),
      agentStatus: MockAgentStatusEngine(),
      mcpAdmin: MockMCPAdminEngine(),
      hotkeys: MockHotkeysEngine(),
      licenseService: .preview,
      runtimeLlmLaneProvider: { lane in
        CsRuntimeLlmLane(
          lane: lane, providerId: "fixture", providerDisplayName: "Fixture",
          wire: "responses", endpoint: "https://fixture.invalid/v1/responses",
          model: "fixture", keyAccount: "FIXTURE_KEY", keyPresent: false,
          accountAuth: false, available: false,
          unavailable: .modelMissing(provider: "Fixture"))
      },
      servingStatusProvider: { nil })
  }

  private func cardStore() -> AgentChatStore {
    let messages = (0..<12).map {
      ChatMessage(role: $0.isMultiple(of: 2) ? .you : .assistant, timestamp: nil, text: "m\($0)")
    }
    var thread = ChatThread(title: "Evidence", meta: "", messages: messages, model: "fixture")
    thread.updatedAt = Date().addingTimeInterval(-26 * 3600)
    return AgentChatStore(engine: nil, threads: [thread])
  }

  // MARK: - Rendering

  private func evidenceDirectory() throws -> URL {
    let environment = ProcessInfo.processInfo.environment["CODESCRIBE_VISUAL_EVIDENCE_DIR"]
    let url =
      environment.flatMap { $0.isEmpty ? nil : URL(fileURLWithPath: $0, isDirectory: true) }
      ?? FileManager.default.temporaryDirectory.appendingPathComponent(
        "codescribe-permission-card-evidence", isDirectory: true)
    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    return url
  }

  private func spin(until predicate: () -> Bool) {
    let deadline = Date().addingTimeInterval(2)
    while Date() < deadline, !predicate() {
      RunLoop.main.run(until: Date().addingTimeInterval(0.02))
    }
  }

  private func render<V: View>(_ id: String, dark: Bool, size: NSSize, _ root: V) throws
    -> String
  {
    let appearance = NSAppearance(named: dark ? .darkAqua : .aqua)!
    let window = NSWindow(
      contentRect: NSRect(origin: .zero, size: size), styleMask: [.titled],
      backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.appearance = appearance
    defer {
      window.contentView = nil
      window.close()
    }
    let host = NSHostingView(
      rootView:
        root
        .environment(\.colorScheme, dark ? .dark : .light)
        .transaction { $0.disablesAnimations = true }
        .frame(width: size.width, height: size.height, alignment: .topLeading)
        .background(Color(nsColor: .windowBackgroundColor)))
    host.appearance = appearance
    host.sizingOptions = []
    host.frame = NSRect(origin: .zero, size: size)
    window.contentView = host
    XCTAssertFalse(window.isVisible, "\(id) stays unordered")
    let deadline = Date().addingTimeInterval(0.6)
    while Date() < deadline {
      host.layoutSubtreeIfNeeded()
      RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    }
    host.layoutSubtreeIfNeeded()
    let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
    host.cacheDisplay(in: host.bounds, to: bitmap)
    let png = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
    let file = try evidenceDirectory().appendingPathComponent("\(id).png")
    try png.write(to: file)
    return file.lastPathComponent
  }
}

private struct EvidenceOfflineBridge: AgentBridgeInstalling {
  func status() -> AgentBridgeInstallationStatus {
    AgentBridgeInstallationStatus(
      payloadAvailable: true, bundleVersion: "evidence-fixture", installedClients: [],
      installedPaths: [], detail: "Offline fixture")
  }

  func install(selectedClients _: Set<AgentBridgeClient>) throws -> AgentBridgeInstallationStatus {
    throw AgentBridgeInstallationError.payloadUnavailable
  }

  func adoptManualSkill(client _: AgentBridgeClient) throws -> AgentBridgeAdoptionResult {
    throw AgentBridgeInstallationError.payloadUnavailable
  }
}
