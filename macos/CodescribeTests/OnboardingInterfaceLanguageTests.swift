import AppKit
import SwiftUI
import Vision
import XCTest

@testable import Codescribe

@MainActor
final class OnboardingInterfaceLanguageTests: XCTestCase {
  private func withPreferences(_ body: (UserDefaults, String) throws -> Void) throws {
    let suite = "codescribe-interface-language-tests-\(UUID().uuidString)"
    let preferences = try XCTUnwrap(UserDefaults(suiteName: suite))
    // XCTest forces English through launch arguments. Only this isolated suite
    // drops that argument domain so a relaunch can read its persisted choice.
    preferences.setVolatileDomain([:], forName: UserDefaults.argumentDomain)
    defer { preferences.removePersistentDomain(forName: suite) }
    try body(preferences, suite)
  }

  private func model(
    preferences: UserDefaults, engine: LanguageRecordingEngine = LanguageRecordingEngine()
  ) -> OnboardingViewModel {
    OnboardingViewModel(
      engine: engine, hotkeys: MockHotkeysEngine(), agentStatus: MockAgentStatusEngine(),
      agentBridge: LanguageTestBridge(), probe: MockPermissionProbe(.allGranted),
      languagePreferences: preferences)
  }

  func testSystemPreferenceMatchingAndUnsupportedLanguageDefault() {
    XCTAssertEqual(InterfaceLanguage.preferred(from: ["pl-PL", "en"]), .polish)
    XCTAssertEqual(InterfaceLanguage.preferred(from: ["en-GB", "pl"]), .english)
    XCTAssertEqual(InterfaceLanguage.preferred(from: ["de-DE", "pl"]), .polish)
    XCTAssertEqual(InterfaceLanguage.preferred(from: ["de-DE"]), .english)
    XCTAssertEqual(InterfaceLanguage.preferred(from: []), .english)
  }

  func testResumeIndicesStillIdentifyTheOriginalPermissionAndDictationSteps() throws {
    let expected: [OnboardingStep] = [
      .interfaceLanguage, .mode, .permission(.microphone), .permission(.accessibility),
      .permission(.inputMonitoring), .permission(.screenRecording),
      .permission(.speechRecognition), .permission(.fullDiskAccess), .language,
      .apiKey, .hotkeyMode, .agenticReadiness, .done,
    ]
    try withPreferences { preferences, _ in
      for (index, step) in expected.enumerated() {
        let engine = LanguageRecordingEngine()
        engine.fixture.progress = UInt32(index)
        let resumed = model(preferences: preferences, engine: engine)
        XCTAssertEqual(resumed.step, step, "Persisted index \(index)")
        XCTAssertEqual(resumed.totalSteps, 13)
        XCTAssertTrue(engine.configWrites.isEmpty)
      }
    }
  }

  func testChoosingInterfaceLanguagePersistsWithoutChangingDictationLanguage() throws {
    try withPreferences { preferences, suite in
      preferences.set(["en-GB"], forKey: "AppleLanguages")
      let engine = LanguageRecordingEngine()
      engine.fixture.language = .auto
      let wizard = model(preferences: preferences, engine: engine)
      XCTAssertEqual(wizard.interfaceLanguage, .english)
      wizard.selectInterfaceLanguage(.polish)
      XCTAssertEqual(
        preferences.persistentDomain(forName: suite)?["AppleLanguages"] as? [String], ["pl"])
      XCTAssertEqual(wizard.selectedLanguage, .auto)
      XCTAssertEqual(engine.fixture.language, .auto)
      XCTAssertTrue(engine.configWrites.isEmpty)
      XCTAssertEqual(model(preferences: preferences).interfaceLanguage, .polish)
    }
  }

  func testConstructionDoesNotPersistAndContinueSavesUntouchedChoice() throws {
    try withPreferences { preferences, suite in
      let before = preferences.persistentDomain(forName: suite)
      let engine = LanguageRecordingEngine()
      let wizard = model(preferences: preferences, engine: engine)
      XCTAssertEqual(
        preferences.persistentDomain(forName: suite) as NSDictionary?, before as NSDictionary?)
      wizard.advance()
      XCTAssertEqual(
        preferences.persistentDomain(forName: suite)?["AppleLanguages"] as? [String],
        [wizard.interfaceLanguage.rawValue])
      XCTAssertEqual(wizard.step, .mode)
      XCTAssertEqual(engine.fixture.progress, 1)
      XCTAssertTrue(engine.configWrites.isEmpty)
      wizard.back()
      XCTAssertEqual(wizard.step, .interfaceLanguage)
    }
  }

  func testSameWizardChangesFoundationCopyImmediatelyInBothDirections() throws {
    try withPreferences { preferences, _ in
      preferences.set(["en"], forKey: "AppleLanguages")
      let wizard = model(preferences: preferences)
      XCTAssertEqual(wizard.primaryLabel, "Continue")
      XCTAssertEqual(wizard.progressLabel, "Step 1 of 13")
      let englishTitle = wizard.windowTitle
      wizard.selectInterfaceLanguage(.polish)
      XCTAssertEqual(wizard.primaryLabel, "Dalej")
      XCTAssertEqual(wizard.progressLabel, "Krok 1 z 13")
      XCTAssertNotEqual(wizard.windowTitle, englishTitle)
      XCTAssertEqual(
        PermissionKind.microphone.onboardingTitle(locale: wizard.interfaceLocale),
        "Dostęp do mikrofonu")
      XCTAssertEqual(PermissionState.granted.label(locale: wizard.interfaceLocale), "przyznano")
      wizard.selectInterfaceLanguage(.english)
      XCTAssertEqual(wizard.primaryLabel, "Continue")
      XCTAssertEqual(wizard.progressLabel, "Step 1 of 13")
      XCTAssertEqual(wizard.windowTitle, englishTitle)
      XCTAssertEqual(
        PermissionKind.microphone.onboardingTitle(locale: wizard.interfaceLocale),
        "Microphone Access")
    }
  }

  func testOffscreenWizardRendersNativeNamesAndUpdatesLocalizedControls() throws {
    try withPreferences { preferences, _ in
      preferences.set(["en"], forKey: "AppleLanguages")
      let wizard = model(preferences: preferences)
      let host = NSHostingView(rootView: OnboardingView(model: wizard))
      let window = NSWindow(
        contentRect: NSRect(x: 0, y: 0, width: 720, height: 620),
        styleMask: [.borderless], backing: .buffered, defer: false)
      window.isReleasedWhenClosed = false
      host.sizingOptions = []
      host.frame = NSRect(x: 0, y: 0, width: 720, height: 620)
      window.contentView = host
      defer {
        window.contentView = nil
        window.close()
      }
      settle(host)
      let english = try renderedText(host)
      XCTAssertTrue(english.contains("English"), english)
      XCTAssertTrue(english.contains("Polski"), english)
      XCTAssertTrue(english.contains("Continue"), english)
      wizard.selectInterfaceLanguage(.polish)
      settle(host)
      let polish = try renderedText(host)
      XCTAssertTrue(polish.contains("Dalej"), polish)
      XCTAssertTrue(polish.contains("Wybierz"), polish)
      XCTAssertFalse(polish.contains("Continue"), polish)
      XCTAssertFalse(window.isVisible, "No live screen or system input is used")
    }
  }

  private func settle(_ host: NSView) {
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.06))
    host.layoutSubtreeIfNeeded()
  }

  private func renderedText(_ host: NSView) throws -> String {
    let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
    host.cacheDisplay(in: host.bounds, to: bitmap)
    let request = VNRecognizeTextRequest()
    request.recognitionLevel = .fast
    request.recognitionLanguages = ["en-US"]
    request.usesLanguageCorrection = false
    try VNImageRequestHandler(cgImage: XCTUnwrap(bitmap.cgImage)).perform([request])
    return (request.results ?? []).compactMap { $0.topCandidates(1).first?.string }
      .joined(separator: "\n")

  }

}

@MainActor
private final class LanguageRecordingEngine: OnboardingEngine {
  let fixture = MockOnboardingEngine()
  var configWrites: [(String, String)] = []
  func shouldShowOnboarding() -> Bool { fixture.shouldShowOnboarding() }
  func onboardingProgress() -> UInt32 { fixture.onboardingProgress() }
  func saveOnboardingProgress(step: UInt32) { fixture.saveOnboardingProgress(step: step) }
  func markOnboardingDone() { fixture.markOnboardingDone() }
  func onboardingMode() -> String? { fixture.onboardingMode() }
  func setOnboardingMode(_ mode: String) throws { try fixture.setOnboardingMode(mode) }
  func currentLanguage() -> CsLanguage { fixture.currentLanguage() }
  func assistiveProvider() -> String? { fixture.assistiveProvider() }
  func keyStatus() -> CsKeyStatus { fixture.keyStatus() }
  func availableProviders() -> [CsProviderOption] { fixture.availableProviders() }
  func setApiKey(account: String, secret: String) throws {
    try fixture.setApiKey(account: account, secret: secret)
  }
  func updateConfig(key: String, value: String) throws { configWrites.append((key, value)) }
}

private struct LanguageTestBridge: AgentBridgeInstalling {
  func status() -> AgentBridgeInstallationStatus {
    .init(
      payloadAvailable: false, bundleVersion: nil, installedClients: [], installedPaths: [],
      detail: "")
  }
  func install(selectedClients: Set<AgentBridgeClient>) throws -> AgentBridgeInstallationStatus {
    throw AgentBridgeInstallationError.payloadUnavailable
  }
  func adoptManualSkill(client: AgentBridgeClient) throws -> AgentBridgeAdoptionResult {
    throw AgentBridgeInstallationError.payloadUnavailable
  }
}
