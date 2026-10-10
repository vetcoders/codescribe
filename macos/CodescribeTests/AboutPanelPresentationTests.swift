import XCTest

@testable import Codescribe

/// The About panel shows readable sentences for runtime facts while keeping the
/// raw values for diagnostics. These presenters are pure, so the tests pin the
/// exact mapping without a view.
final class AboutPanelPresentationTests: XCTestCase {
  // MARK: - Build date

  func testBuildDateFollowsTheInterfaceLocaleAndKeepsTheRawValueElsewhere() throws {
    let raw = "2026-10-08T13:50:45Z"
    let utc = try XCTUnwrap(TimeZone(identifier: "UTC"))

    let polish = try XCTUnwrap(
      BuildDatePresentation.readable(raw, locale: Locale(identifier: "pl"), timeZone: utc))
    let english = try XCTUnwrap(
      BuildDatePresentation.readable(raw, locale: Locale(identifier: "en"), timeZone: utc))

    XCTAssertTrue(polish.contains("8 października 2026"), polish)
    XCTAssertTrue(polish.contains("13:50"), "Polish uses a 24-hour clock: \(polish)")
    XCTAssertFalse(polish.contains("PM"), polish)
    XCTAssertTrue(english.contains("October 8, 2026"), english)
    XCTAssertTrue(english.contains("1:50"), english)
    XCTAssertTrue(english.contains("PM"), english)
  }

  func testUnparseableBuildDateIsNotInvented() {
    XCTAssertNil(BuildDatePresentation.readable("unknown", locale: Locale(identifier: "pl")))
    XCTAssertNil(BuildDatePresentation.readable("", locale: Locale(identifier: "en")))
  }

  // MARK: - Launch repair receipt

  func testAnUnreadEnvKeyIsNamedWithItsEffectAndWhatToDo() throws {
    let raw = "Config: 0 repairs at launch; env key(s) need review: CODESCRIBE_STT_ENGINE"
    let notice = ConfigRepairNotice(raw: raw)

    XCTAssertEqual(notice.kind, .keysNeedReview)
    XCTAssertTrue(notice.isWarning)
    XCTAssertEqual(notice.reviewKeys, ["CODESCRIBE_STT_ENGINE"])
    XCTAssertEqual(notice.title, "The configuration needs a review")
    XCTAssertEqual(notice.subtitle, "See which setting is out of date")
    XCTAssertNil(notice.outcomeLine, "the key items carry the explanation")

    let items = notice.reviewItems(envFile: "~/.codescribe/.env")
    let item = try XCTUnwrap(items.first)
    XCTAssertEqual(items.count, 1)
    XCTAssertEqual(item.key, "CODESCRIBE_STT_ENGINE")
    XCTAssertEqual(
      item.impact, "Codescribe does not read this entry in ~/.codescribe/.env, so it has no effect.")
    XCTAssertEqual(
      item.action,
      "No action is required. To clear this notice, delete or correct the line, then restart Codescribe."
    )
    XCTAssertEqual(notice.raw, raw, "the original line survives for diagnostics")
  }

  func testSeveralReviewKeysAndABackupSuffixAreSeparated() {
    let notice = ConfigRepairNotice(
      raw:
        "Config: 2 repairs at launch; env key(s) need review: A_KEY, B_KEY (backup /tmp/settings.json.bak)"
    )

    XCTAssertEqual(notice.kind, .keysNeedReview)
    XCTAssertEqual(notice.reviewKeys, ["A_KEY", "B_KEY"])
    XCTAssertEqual(notice.subtitle, "See which settings are out of date")
    XCTAssertEqual(notice.reviewItems(envFile: ".env").map(\.key), ["A_KEY", "B_KEY"])
  }

  func testAFormattingLevelOverrideSaysWhichValueWinsAndHowToDropIt() throws {
    let notice = ConfigRepairNotice(
      raw: "Config: 0 repairs at launch; env key(s) need review: FORMATTING_LEVEL")
    let item = try XCTUnwrap(notice.reviewItems(envFile: "~/.codescribe/.env").first)

    XCTAssertEqual(item.key, ConfigRepairNotice.formattingLevelKey)
    XCTAssertEqual(
      item.impact,
      "A formatting level set outside the app differs from the one chosen in Settings. The value from outside the app is in effect."
    )
    XCTAssertEqual(
      item.action,
      "To use the level from Settings, remove FORMATTING_LEVEL from the launch environment or from ~/.codescribe/.env, then restart Codescribe."
    )
  }

  func testRepairWithoutReviewKeysAndRefusalsHaveTheirOwnSentences() {
    let repaired = ConfigRepairNotice(raw: "Config repaired at launch: 3 changes")
    XCTAssertEqual(repaired.kind, .repaired)
    XCTAssertFalse(repaired.isWarning, "a completed repair informs, it does not warn")
    XCTAssertTrue(repaired.reviewItems(envFile: ".env").isEmpty)
    XCTAssertEqual(repaired.title, "The configuration was repaired at startup")
    XCTAssertEqual(repaired.subtitle, "No action needed. See what changed")
    XCTAssertNotNil(repaired.outcomeLine)

    let refused = ConfigRepairNotice(
      raw: "Config needs attention: unknown schema version 9 (/tmp/settings.json)")
    XCTAssertEqual(refused.kind, .unresolved)
    XCTAssertTrue(refused.isWarning)
    XCTAssertEqual(refused.title, "The configuration could not be fully checked")
    XCTAssertEqual(refused.subtitle, "See what to correct")
    XCTAssertEqual(
      refused.outcomeLine,
      "Codescribe left the file unchanged. Correct the file named in the record below, then restart Codescribe."
    )
  }

  // MARK: - First dictation confirmation

  func testProductionPingIsNotLiveSoThePanelHidesItsSwitch() {
    // `ActivationPingConfiguration.production` ships without an analytics
    // domain; About shows the opt-in only when the build can send it.
    XCTAssertFalse(ActivationPingConfiguration.production.isEnabled)
  }
}
