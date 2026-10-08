import XCTest

@testable import Codescribe

/// Proves the localization pipeline end to end on the built app bundle: the
/// catalogs under `Resources/Localization` are compiled into it, English is the
/// development language, and catalog-resolved copy reaches a lookup.
/// Contract: docs/LOCALIZATION.md.
final class LocalizationFoundationTests: XCTestCase {
  /// The test bundle is hosted by the app, so `Bundle.main` is Codescribe.app.
  private let app = Bundle.main

  func testEnglishIsTheDevelopmentLanguage() {
    XCTAssertEqual(app.developmentLocalization, "en")
    XCTAssertTrue(app.localizations.contains("en"), "\(app.localizations)")
  }

  func testTheSuiteRunsInEnglish() {
    // The scheme pins the test language (macos/project.yml), so assertions on
    // interface copy read the same on any host.
    XCTAssertEqual(app.preferredLocalizations.first, "en")
  }

  func testPermissionPromptsComeFromTheCatalog() throws {
    let localized = try XCTUnwrap(
      app.localizedInfoDictionary, "InfoPlist.xcstrings was not compiled into the bundle")
    let prompts = [
      "NSAppleEventsUsageDescription", "NSMicrophoneUsageDescription",
      "NSScreenCaptureUsageDescription", "NSSpeechRecognitionUsageDescription",
    ]
    for key in prompts {
      let plist = try XCTUnwrap(app.infoDictionary?[key] as? String, key)
      XCTAssertEqual(localized[key] as? String, plist, key)
      XCTAssertFalse(plist.isEmpty, key)
    }
  }

  /// Counted phrases pick their noun in the catalog, never in code (R4).
  func testPluralVariationsSelectTheNoun() {
    func sources(_ count: Int) -> String {
      String(localized: "\(count) tool sources", bundle: app)
    }
    XCTAssertEqual(sources(1), "1 tool source")
    XCTAssertEqual(sources(2), "2 tool sources")
    XCTAssertEqual(sources(0), "0 tool sources")
  }

  /// A sentence with several counts inflects each one through its own
  /// substitution.
  func testEachCountInASentenceInflectsOnItsOwn() {
    func moved(_ threads: Int, _ files: Int) -> String {
      String(
        localized: "Moves \(threads) Agent threads and \(files) Agent files to Trash.", bundle: app)
    }
    XCTAssertEqual(moved(1, 2), "Moves 1 Agent thread and 2 Agent files to Trash.")
    XCTAssertEqual(moved(3, 1), "Moves 3 Agent threads and 1 Agent file to Trash.")
  }

  /// A key written as an identifier resolves to the English given beside it
  /// in code, never to the identifier (R7).
  func testAnIdentifierKeyResolvesToItsEnglishText() {
    let terms = SettingsSection.audio.searchKeywords
    XCTAssertTrue(terms.contains("microphone"), "\(terms)")
    XCTAssertFalse(terms.contains { $0.hasPrefix("settings.search") }, "\(terms)")
  }
}
