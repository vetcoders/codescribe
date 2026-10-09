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

  func testReviewKeysBecomeAUserSentenceAndStayInTheDetails() {
    let raw = "Config: 0 repairs at launch; env key(s) need review: CODESCRIBE_STT_ENGINE"
    let notice = ConfigRepairNotice(raw: raw)

    XCTAssertEqual(notice.kind, .keysNeedReview)
    XCTAssertEqual(notice.reviewKeys, ["CODESCRIBE_STT_ENGINE"])
    XCTAssertEqual(
      notice.headline, "An outdated configuration setting was detected. It needs a review.")
    XCTAssertEqual(notice.reviewKeysLine, "Setting to review: CODESCRIBE_STT_ENGINE")
    XCTAssertEqual(notice.raw, raw, "the original line survives for diagnostics")
  }

  func testSeveralReviewKeysAndABackupSuffixAreSeparated() {
    let notice = ConfigRepairNotice(
      raw:
        "Config: 2 repairs at launch; env key(s) need review: A_KEY, B_KEY (backup /tmp/settings.json.bak)"
    )

    XCTAssertEqual(notice.kind, .keysNeedReview)
    XCTAssertEqual(notice.reviewKeys, ["A_KEY", "B_KEY"])
    XCTAssertEqual(notice.reviewKeysLine, "Setting to review: A_KEY, B_KEY")
  }

  func testRepairWithoutReviewKeysAndRefusalsHaveTheirOwnSentences() {
    let repaired = ConfigRepairNotice(raw: "Config repaired at launch: 3 changes")
    XCTAssertEqual(repaired.kind, .repaired)
    XCTAssertNil(repaired.reviewKeysLine)
    XCTAssertEqual(repaired.headline, "The configuration was repaired when Codescribe started.")

    let refused = ConfigRepairNotice(raw: "ConfigUnrepairable: unknown schema version 9")
    XCTAssertEqual(refused.kind, .unresolved)
    XCTAssertEqual(
      refused.headline, "The configuration could not be fully checked. See the details.")
  }

  // MARK: - First dictation confirmation

  func testProductionPingIsNotLiveInThisBuild() {
    // `ActivationPingConfiguration.production` ships without an analytics
    // domain, so the switch must say that nothing is sent.
    XCTAssertFalse(ActivationPingConfiguration.production.isEnabled)
    let availability = ActivationPingAvailability(optIn: true)
    XCTAssertFalse(availability.serviceEnabled)
    XCTAssertEqual(
      availability.unavailableLine,
      "Not available in this version: nothing is sent, whatever the switch says.")
  }

  func testStateLineSeparatesTheShippedDefaultFromTheCurrentChoice() {
    XCTAssertEqual(
      ActivationPingAvailability(serviceEnabled: true, optIn: true).stateLine,
      "Default: off · Now: on")
    XCTAssertEqual(
      ActivationPingAvailability(serviceEnabled: true, optIn: false).stateLine,
      "Default: off · Now: off")
    XCTAssertNil(ActivationPingAvailability(serviceEnabled: true, optIn: true).unavailableLine)
  }
}
