import XCTest

@testable import Codescribe

final class MessageTimestampTests: XCTestCase {
  private func instant(_ value: String) -> Date {
    MessageTimestamp.date(fromISO8601: value)!
  }

  private func calendar(_ zone: String = "Europe/Warsaw") -> Calendar {
    var calendar = Calendar(identifier: .gregorian)
    calendar.timeZone = TimeZone(identifier: zone)!
    return calendar
  }

  func testTodayUsesTheSystemLocaleClockWithoutADate() {
    let now = instant("2026-10-11T16:00:00Z")
    let date = instant("2026-10-11T11:05:00Z")
    XCTAssertEqual(
      MessageTimestamp.label(
        for: date, now: now, calendar: calendar(), locale: Locale(identifier: "en_GB")),
      "13:05")
    let american = MessageTimestamp.label(
      for: date, now: now, calendar: calendar(), locale: Locale(identifier: "en_US"))
    // Foundation's AM/PM separator may be a narrow or ordinary no-break space.
    XCTAssertEqual(american.components(separatedBy: .whitespaces).joined(separator: " "), "1:05 PM")
  }

  func testYesterdayUsesLocalCalendarDayAndLocalizedWord() {
    let now = instant("2026-10-11T00:05:00Z")
    let date = instant("2026-10-10T20:05:00Z")
    XCTAssertEqual(
      MessageTimestamp.label(
        for: date, now: now, calendar: calendar(), locale: Locale(identifier: "pl_PL"),
        yesterday: "Wczoraj"),
      "Wczoraj 22:05")
  }

  func testOlderAndFutureDaysCarryFullCalendarDate() {
    for (input, expected) in [
      ("2026-10-09T11:05:00Z", "2026-10-09 13:05"),
      ("2026-10-12T11:05:00Z", "2026-10-12 13:05"),
    ] {
      XCTAssertEqual(
        MessageTimestamp.label(
          for: instant(input), now: instant("2026-10-11T16:00:00Z"), calendar: calendar(),
          locale: Locale(identifier: "en_GB")), expected)
    }
  }

  func testYesterdayAcrossDaylightSavingIsNotTwentyFourHoursAgo() {
    XCTAssertEqual(
      MessageTimestamp.label(
        for: instant("2026-10-25T00:30:00Z"), now: instant("2026-10-25T23:30:00Z"),
        calendar: calendar(), locale: Locale(identifier: "en_GB"), yesterday: "Yesterday"),
      "Yesterday 02:30")
  }

  func testUTCInstantCanBeTodayInAnotherSystemTimeZone() {
    let date = instant("2026-10-10T23:30:00Z")
    let now = instant("2026-10-11T00:30:00Z")
    XCTAssertEqual(
      MessageTimestamp.label(
        for: date, now: now, calendar: calendar(), locale: Locale(identifier: "en_GB")),
      "01:30")
    XCTAssertEqual(
      MessageTimestamp.label(
        for: date, now: now, calendar: calendar("UTC"), locale: Locale(identifier: "en_GB"),
        yesterday: "Yesterday"),
      "Yesterday 23:30")
  }

  func testMissingOrMalformedTimeIsNeverPresentedAsNow() {
    XCTAssertNil(MessageTimestamp.date(fromISO8601: "not a timestamp"))
    XCTAssertEqual(MessageTimestamp.label(for: nil, now: Date()), "—")
    XCTAssertEqual(
      instant("2026-10-11T16:00:00.000Z"), instant("2026-10-11T16:00:00Z"))
  }

  func testRestoringMessagesRetainsFullTimestampAndDoesNotInventMissingDates() {
    let records = [
      CsThreadMessage(role: "user", text: "History", rawJson: "{}", timestampMs: 1_791_729_600_000),
      CsThreadMessage(role: "assistant", text: "Unknown time", rawJson: "{}", timestampMs: 0),
    ]
    let restored = RealThreadsEngine.restoredMessages(from: records)
    XCTAssertEqual(restored.count, 2)
    XCTAssertEqual(restored[0].timestamp, Date(timeIntervalSince1970: 1_791_729_600))
    XCTAssertNil(restored[1].timestamp)
  }
}
