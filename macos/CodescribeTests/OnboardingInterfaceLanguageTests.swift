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
    preferences: UserDefaults, engine: LanguageRecordingEngine = LanguageRecordingEngine(),
    processLanguage: InterfaceLanguage = .english
  ) -> OnboardingViewModel {
    OnboardingViewModel(
      engine: engine, hotkeys: MockHotkeysEngine(), agentStatus: MockAgentStatusEngine(),
      agentBridge: LanguageTestBridge(), probe: MockPermissionProbe(.allGranted),
      languagePreferences: preferences, processInterfaceLanguage: processLanguage)
  }

  func testSystemPreferenceMatchingAndUnsupportedLanguageDefault() {
    XCTAssertEqual(InterfaceLanguage.preferred(from: ["pl-PL", "en"]), .polish)
    XCTAssertEqual(InterfaceLanguage.preferred(from: ["en-GB", "pl"]), .english)
    XCTAssertEqual(InterfaceLanguage.preferred(from: ["de-DE", "pl"]), .polish)
    XCTAssertEqual(InterfaceLanguage.preferred(from: ["de-DE"]), .english)
    XCTAssertEqual(InterfaceLanguage.preferred(from: []), .english)
  }

  func testResumeIndicesIdentifyGroupedSetupChapters() throws {
    let expected: [OnboardingStep] = [
      .interfaceLanguage, .mode, .permissions, .language, .localModel,
      .apiKey, .hotkeyMode, .agenticReadiness, .done,
    ]
    try withPreferences { preferences, _ in
      for (index, step) in expected.enumerated() {
        let engine = LanguageRecordingEngine()
        engine.fixture.progress = UInt32(index)
        let resumed = model(preferences: preferences, engine: engine)
        XCTAssertEqual(resumed.step, step, "Persisted index \(index)")
        XCTAssertEqual(resumed.totalSteps, 9)
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

  func testChangedLanguageDoesNotLeavePickerWithoutWholeAppApplication() async throws {
    try withPreferences { preferences, _ in
      let engine = LanguageRecordingEngine()
      let wizard = model(preferences: preferences, engine: engine)
      let running = InterfaceLanguage.preferred(from: Bundle.main.preferredLocalizations)
      wizard.selectInterfaceLanguage(running == .english ? .polish : .english)
      wizard.advance()
      XCTAssertEqual(wizard.step, .interfaceLanguage)
      XCTAssertEqual(engine.fixture.progress, 0)
    }
  }

  func testWholeAppApplicationPersistsResumeOnlyAfterIdleAdmission() async throws {
    let suite = "codescribe-language-apply-\(UUID().uuidString)"
    let preferences = try XCTUnwrap(UserDefaults(suiteName: suite))
    preferences.setVolatileDomain([:], forName: UserDefaults.argumentDomain)
    defer { preferences.removePersistentDomain(forName: suite) }
    let engine = LanguageRecordingEngine()
    let wizard = model(preferences: preferences, engine: engine)
    let admitted = expectation(description: "Idle guard called")
    let applied = expectation(description: "Restart armed")
    let gate = AsyncStream<Void>.makeStream()
    var applications = 0
    wizard.onApplyInterfaceLanguage = { beforeTermination in
      applications += 1
      admitted.fulfill()
      for await _ in gate.stream { break }
      XCTAssertEqual(engine.fixture.progress, 0)
      beforeTermination()
      XCTAssertEqual(engine.fixture.progress, 1, "Persist before normal app termination")
      applied.fulfill()
    }
    wizard.selectInterfaceLanguage(.polish)
    wizard.advance()
    await fulfillment(of: [admitted], timeout: 1)
    XCTAssertTrue(wizard.applyingInterfaceLanguage)
    XCTAssertEqual(wizard.step, .interfaceLanguage)
    wizard.advance()
    wizard.selectInterfaceLanguage(.english)
    wizard.back()
    XCTAssertEqual(wizard.interfaceLanguage, .polish)
    XCTAssertEqual(applications, 1)
    gate.continuation.yield(())
    gate.continuation.finish()
    await fulfillment(of: [applied], timeout: 1)
    XCTAssertEqual(wizard.step, .mode)
    XCTAssertTrue(engine.configWrites.isEmpty)
    let resumed = model(preferences: preferences, engine: engine, processLanguage: .polish)
    XCTAssertEqual(resumed.step, .mode)
    XCTAssertEqual(resumed.interfaceLanguage, .polish)
    XCTAssertFalse(resumed.interfaceLanguageNeedsRestart)
    resumed.back()
    resumed.advance()
    XCTAssertEqual(resumed.step, .mode, "The relaunched process does not restart again")
  }

  func testBusyApplicationRetainsPickerAndSavedLanguageForRetry() async throws {
    let suite = "codescribe-language-busy-\(UUID().uuidString)"
    let preferences = try XCTUnwrap(UserDefaults(suiteName: suite))
    preferences.setVolatileDomain([:], forName: UserDefaults.argumentDomain)
    defer { preferences.removePersistentDomain(forName: suite) }
    let engine = LanguageRecordingEngine()
    let wizard = model(preferences: preferences, engine: engine)
    let refused = expectation(description: "Idle guard refused")
    wizard.onApplyInterfaceLanguage = { _ in
      defer { refused.fulfill() }
      throw InterfaceLanguageRestartError.busy
    }
    wizard.selectInterfaceLanguage(.polish)
    wizard.advance()
    await fulfillment(of: [refused], timeout: 1)
    await Task.yield()
    XCTAssertEqual(wizard.step, .interfaceLanguage)
    XCTAssertEqual(engine.fixture.progress, 0)
    XCTAssertFalse(wizard.applyingInterfaceLanguage)
    XCTAssertTrue(try XCTUnwrap(wizard.lastError).contains("Zakończ nagrywanie"))
    XCTAssertEqual(preferences.stringArray(forKey: "AppleLanguages"), ["pl"])
    XCTAssertTrue(engine.configWrites.isEmpty)
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
      XCTAssertEqual(wizard.progressLabel, "Step 1 of 9")
      let englishTitle = wizard.windowTitle
      wizard.selectInterfaceLanguage(.polish)
      XCTAssertEqual(wizard.primaryLabel, "Uruchom ponownie i kontynuuj")
      XCTAssertEqual(wizard.progressLabel, "Krok 1 z 9")
      XCTAssertNotEqual(wizard.windowTitle, englishTitle)
      XCTAssertEqual(
        PermissionKind.microphone.displayName(locale: wizard.interfaceLocale),
        "Mikrofon")
      XCTAssertEqual(PermissionState.granted.label(locale: wizard.interfaceLocale), "Przyznano")
      wizard.selectInterfaceLanguage(.english)
      XCTAssertEqual(wizard.primaryLabel, "Continue")
      XCTAssertEqual(wizard.progressLabel, "Step 1 of 9")
      XCTAssertEqual(wizard.windowTitle, englishTitle)
      XCTAssertEqual(
        PermissionKind.microphone.displayName(locale: wizard.interfaceLocale),
        "Microphone")
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
      XCTAssertTrue(polish.contains("kontynuuj"), polish)
      // OCR may change letter case; the exact localized copy is asserted above.
      XCTAssertTrue(polish.localizedCaseInsensitiveContains("Wybierz"), polish)
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
    let captured = try XCTUnwrap(bitmap.cgImage)
    // A hidden window has no desktop backdrop. Cache-display leaves glass
    // regions transparent; OCR must see them over the same semantic window
    // background, rather than interpreting transparent white text as white.
    let imageBounds = CGRect(
      x: 0, y: 0, width: captured.width * 2, height: captured.height * 2)
    let canvas = try XCTUnwrap(
      CGContext(
        data: nil, width: captured.width * 2, height: captured.height * 2,
        bitsPerComponent: 8, bytesPerRow: 0,
        space: CGColorSpaceCreateDeviceRGB(),
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue))
    host.effectiveAppearance.performAsCurrentDrawingAppearance {
      canvas.setFillColor(NSColor.windowBackgroundColor.cgColor)
      canvas.fill(imageBounds)
    }
    canvas.interpolationQuality = .high
    canvas.draw(captured, in: imageBounds)
    let image = try XCTUnwrap(canvas.makeImage())
    let request = VNRecognizeTextRequest()
    // Enlarge the cached pixels so `.fast` can read 13 pt text even when
    // the hidden window has a 1.0 backing scale. This avoids provisioning
    // the accurate recognition model; all visible-copy assertions remain.
    request.recognitionLevel = .fast
    request.recognitionLanguages = ["en-US"]
    request.usesLanguageCorrection = false
    try VNImageRequestHandler(cgImage: image).perform([request])
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
