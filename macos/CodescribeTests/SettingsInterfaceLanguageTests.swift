import Observation
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

  /// Both appearance choices repaint immediately; only language restarts.
  func testAppearanceAndSheetShadeApplyAtOnce() async throws {
    try await withPreferences { preferences, _ in
      let settings = model(preferences: preferences)
      XCTAssertEqual(settings.appearanceChoice, .system)
      var applied = 0
      settings.onApplyAppearance = { applied += 1 }
      settings.selectAppearance(.dark)
      XCTAssertEqual(applied, 1)
      XCTAssertEqual(preferences.string(forKey: AppearancePreference.key), "dark")
      settings.selectAppearance(.system)
      XCTAssertNil(
        preferences.object(forKey: AppearancePreference.key), "system is the absence of a choice")

      XCTAssertEqual(settings.sheetShadeDark, .todayDark)
      XCTAssertEqual(settings.sheetShadeLight, .todayLight)
      let changed = expectation(description: "The open shade picker observes its preference")
      withObservationTracking {
        _ = settings.sheetShadeDark
      } onChange: {
        changed.fulfill()
      }
      settings.selectSheetShade(.warmCarbon)
      XCTAssertEqual(settings.sheetShadeDark, .warmCarbon)
      XCTAssertEqual(
        settings.sheetShadeLight, .todayLight, "a dark shade leaves the light one alone")
      XCTAssertEqual(preferences.string(forKey: SheetShadePreference.darkKey), "201F1E")
      await fulfillment(of: [changed], timeout: 1)

      settings.selectSheetShade(.todayDark)
      XCTAssertNil(preferences.object(forKey: SheetShadePreference.darkKey))
      // A light raw value stored under the dark key is ignored, never painted.
      preferences.set(SheetShade.limestone.rawValue, forKey: SheetShadePreference.darkKey)
      XCTAssertEqual(SheetShadePreference(defaults: preferences).current(dark: true), .todayDark)
      XCTAssertEqual(SheetShadePreference(defaults: preferences).current(dark: false), .todayLight)
    }
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
        InterfaceLanguageRestartError.busy.message(locale: InterfaceLanguage.polish.locale))
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
        InterfaceLanguageRestartError.unavailable.message(locale: InterfaceLanguage.polish.locale))
      XCTAssertEqual(preferences.stringArray(forKey: InterfaceLanguagePreference.key), ["pl"])
    }
  }

  /// The restart row speaks the language the user just chose, in both directions.
  func testRestartCopySpeaksTheChosenLanguage() async throws {
    try await withPreferences { preferences, _ in
      let english = model(preferences: preferences, processLanguage: .english)
      english.selectInterfaceLanguage(.polish)
      XCTAssertEqual(english.interfaceLanguageRestartTitle, "Uruchom ponownie")
      XCTAssertTrue(
        english.interfaceLanguageRestartExplanation.hasPrefix("Codescribe uruchomi się"))

      let polish = model(preferences: preferences, processLanguage: .polish)
      polish.selectInterfaceLanguage(.english)
      XCTAssertEqual(polish.interfaceLanguageRestartTitle, "Restart now")
      XCTAssertTrue(polish.interfaceLanguageRestartExplanation.hasPrefix("Codescribe will restart"))
    }
  }

  func testRelaunchIntentOnlyResumesSetupFromTheWizard() {
    XCTAssertEqual(InterfaceLanguageRelaunch.resumeSetup.launchArguments, ["--resume-onboarding"])
    XCTAssertEqual(InterfaceLanguageRelaunch.plain.launchArguments, [])
    // A restart pressed in Settings comes back to the same section, in front.
    XCTAssertEqual(
      InterfaceLanguageRelaunch.reopenSettings(.agent).launchArguments,
      ["--open-settings", "agent"])
    XCTAssertEqual(
      InterfaceLanguageRelaunch.settingsSection(arguments: [
        "Codescribe", "--open-settings", "creator",
      ]),
      .creator)
    XCTAssertNil(InterfaceLanguageRelaunch.settingsSection(arguments: ["--open-settings"]))
    XCTAssertNil(InterfaceLanguageRelaunch.settingsSection(arguments: ["--open-settings", "nope"]))
    XCTAssertNil(InterfaceLanguageRelaunch.settingsSection(arguments: ["--resume-onboarding"]))
    let source = try? String(
      contentsOf: URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent()
        .appendingPathComponent("Codescribe/App.swift"),
      encoding: .utf8)
    // The Settings language restart carries the section; the wizard keeps its intent.
    XCTAssertEqual(source?.components(separatedBy: "relaunch: .reopenSettings(section)").count, 2)
    XCTAssertEqual(source?.components(separatedBy: "relaunch: .plain").count, 1)
    // The relaunch script passes `--args` only when the intent carries any.
    XCTAssertTrue(AppDelegate.languageRelaunchScript.contains("if [ \"$#\" -gt 0 ]"))
    XCTAssertTrue(AppDelegate.languageRelaunchScript.contains("--args \"$@\""))
  }
}
