import SwiftUI
import XCTest

@testable import Codescribe

/// Settings › Creator offers the same interface-language switch as the setup
/// wizard, on the same per-app preference and through the same host restart.
@MainActor
final class SettingsInterfaceLanguageTests: XCTestCase {
  private func withPreferences(
    _ body: @MainActor (UserDefaults, String) async throws -> Void
  ) async throws {
    let suite = "codescribe-settings-interface-language-\(UUID().uuidString)"
    let preferences = try XCTUnwrap(UserDefaults(suiteName: suite))
    // XCTest forces English through launch arguments. Only this isolated suite
    // drops that argument domain so the model reads its persisted choice.
    preferences.setVolatileDomain([:], forName: UserDefaults.argumentDomain)
    defer { preferences.removePersistentDomain(forName: suite) }
    try await body(preferences, suite)
  }

  private func model(
    preferences: UserDefaults, engine: MockSettingsEngine = MockSettingsEngine(),
    processLanguage: InterfaceLanguage = .english
  ) -> SettingsViewModel {
    SettingsViewModel(
      engine: engine, permissionProbe: MockPermissionProbe(),
      languagePreferences: preferences, processInterfaceLanguage: processLanguage)
  }

  /// The restart task resets `applyingInterfaceLanguage` in a `defer` after the
  /// host closure returns; give the main actor a few turns to run it.
  private func settleRestart(_ settings: SettingsViewModel) async {
    for _ in 0..<50 where settings.applyingInterfaceLanguage { await Task.yield() }
    XCTAssertFalse(settings.applyingInterfaceLanguage)
  }

  func testPickerReadsTheSavedChoiceAndSavesWithoutTouchingDictation() async throws {
    try await withPreferences { preferences, suite in
      preferences.set(["en-GB"], forKey: InterfaceLanguagePreference.key)
      var engine = MockSettingsEngine()
      var configWrites: [String] = []
      engine.updateConfigObserver = { key, _ in configWrites.append(key) }
      engine.updateConfigManyObserver = { configWrites.append(contentsOf: $0.map(\.key)) }
      let settings = model(preferences: preferences, engine: engine)
      XCTAssertEqual(settings.interfaceLanguage, .english)
      XCTAssertFalse(settings.interfaceLanguageNeedsRestart)

      settings.selectInterfaceLanguage(.polish)

      XCTAssertEqual(settings.interfaceLanguage, .polish)
      XCTAssertTrue(settings.interfaceLanguageNeedsRestart)
      XCTAssertNil(settings.interfaceLanguageNotice)
      XCTAssertEqual(
        preferences.persistentDomain(forName: suite)?[InterfaceLanguagePreference.key]
          as? [String], ["pl"])
      XCTAssertEqual(settings.settings.whisperLanguage, CsSettings.sample.whisperLanguage)
      XCTAssertTrue(configWrites.isEmpty, "The interface language never enters settings.json")
      XCTAssertEqual(model(preferences: preferences).interfaceLanguage, .polish)
      XCTAssertEqual(
        InterfaceLanguagePreference(defaults: preferences, processLanguage: .english).current,
        .polish, "The wizard reads the same preference Settings saved")
    }
  }

  func testChoosingTheRunningLanguageAgainNeedsNoRestart() async throws {
    try await withPreferences { preferences, _ in
      let settings = model(preferences: preferences)
      settings.selectInterfaceLanguage(.polish)
      XCTAssertTrue(settings.interfaceLanguageNeedsRestart)
      settings.selectInterfaceLanguage(.english)
      XCTAssertFalse(settings.interfaceLanguageNeedsRestart)
      var applications = 0
      settings.onApplyInterfaceLanguage = { _ in applications += 1 }
      settings.applyInterfaceLanguage()
      await Task.yield()
      XCTAssertEqual(applications, 0, "Nothing to apply, nothing restarts")
      XCTAssertEqual(preferences.stringArray(forKey: InterfaceLanguagePreference.key), ["en"])
    }
  }

  func testRestartGoesThroughTheHostGuardOnceAndIgnoresRepeatedClicks() async throws {
    try await withPreferences { preferences, _ in
      let settings = model(preferences: preferences)
      let admitted = expectation(description: "Host guard called")
      let gate = AsyncStream<Void>.makeStream()
      var applications = 0
      settings.onApplyInterfaceLanguage = { beforeTermination in
        applications += 1
        admitted.fulfill()
        for await _ in gate.stream { break }
        beforeTermination()
      }
      settings.selectInterfaceLanguage(.polish)
      settings.applyInterfaceLanguage()
      settings.applyInterfaceLanguage()
      await fulfillment(of: [admitted], timeout: 1)
      XCTAssertTrue(settings.applyingInterfaceLanguage)
      settings.selectInterfaceLanguage(.english)
      XCTAssertEqual(settings.interfaceLanguage, .polish, "The picker is locked while restarting")
      gate.continuation.yield(())
      gate.continuation.finish()
      await settleRestart(settings)
      XCTAssertEqual(applications, 1)
      XCTAssertNil(settings.interfaceLanguageNotice)
    }
  }

  func testBusyHostKeepsTheChoiceAndExplainsHowToRetry() async throws {
    try await withPreferences { preferences, _ in
      let settings = model(preferences: preferences)
      let refused = expectation(description: "Host guard refused")
      settings.onApplyInterfaceLanguage = { _ in
        defer { refused.fulfill() }
        throw InterfaceLanguageRestartError.busy
      }
      settings.selectInterfaceLanguage(.polish)
      settings.applyInterfaceLanguage()
      await fulfillment(of: [refused], timeout: 1)
      await settleRestart(settings)
      XCTAssertEqual(settings.interfaceLanguage, .polish)
      XCTAssertTrue(settings.interfaceLanguageNeedsRestart)
      XCTAssertEqual(preferences.stringArray(forKey: InterfaceLanguagePreference.key), ["pl"])
      XCTAssertEqual(
        settings.interfaceLanguageNotice,
        InterfaceLanguageRestartError.busy.message(locale: Locale.current))
      settings.selectInterfaceLanguage(.polish)
      XCTAssertNil(settings.interfaceLanguageNotice, "A new choice clears the stale notice")
    }
  }

  func testWithoutAHostRestartTheRowAsksForAManualRelaunch() async throws {
    try await withPreferences { preferences, _ in
      let settings = model(preferences: preferences)
      settings.selectInterfaceLanguage(.polish)
      settings.applyInterfaceLanguage()
      XCTAssertFalse(settings.applyingInterfaceLanguage)
      XCTAssertEqual(
        settings.interfaceLanguageNotice,
        InterfaceLanguageRestartError.unavailable.message(locale: Locale.current))
      XCTAssertEqual(preferences.stringArray(forKey: InterfaceLanguagePreference.key), ["pl"])
    }
  }

  func testRelaunchIntentOnlyResumesSetupFromTheWizard() {
    XCTAssertEqual(InterfaceLanguageRelaunch.resumeSetup.launchArguments, ["--resume-onboarding"])
    XCTAssertEqual(InterfaceLanguageRelaunch.plain.launchArguments, [])
    // The relaunch script passes `--args` only when the intent carries any.
    XCTAssertTrue(AppDelegate.languageRelaunchScript.contains("if [ \"$#\" -gt 0 ]"))
    XCTAssertTrue(AppDelegate.languageRelaunchScript.contains("--args \"$@\""))
  }
}
