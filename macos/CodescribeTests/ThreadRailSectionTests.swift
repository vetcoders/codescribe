import XCTest

@testable import Codescribe

/// Pure-function coverage for the thread rail's recency sections and the
/// row metadata formatter (ThreadRail.swift). Fixed UTC calendar + injected
/// `now` keep every boundary deterministic regardless of host timezone.
final class ThreadRailSectionTests: XCTestCase {
    private var calendar: Calendar = {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "UTC")!
        return calendar
    }()

    /// 2026-07-16 12:00:00 UTC — the reference "now" for every case.
    private var now: Date { date(2026, 7, 16, 12, 0) }

    private func date(_ year: Int, _ month: Int, _ day: Int, _ hour: Int, _ minute: Int) -> Date {
        DateComponents(
            calendar: calendar, year: year, month: month, day: day, hour: hour, minute: minute
        ).date!
    }

    // MARK: Section boundaries — the C3 matrix

    func testTodayJustAfterMidnightIsToday() {
        let updated = date(2026, 7, 16, 0, 1)  // dziś 00:01
        XCTAssertEqual(ThreadSection.section(for: updated, now: now, calendar: calendar), .today)
    }

    func testYesterdayLateEveningIsYesterday() {
        let updated = date(2026, 7, 15, 23, 59)  // wczoraj 23:59
        XCTAssertEqual(
            ThreadSection.section(for: updated, now: now, calendar: calendar), .yesterday)
    }

    func testSixDaysAgoIsThisWeek() {
        let updated = date(2026, 7, 10, 9, 30)  // 6 dni temu
        XCTAssertEqual(ThreadSection.section(for: updated, now: now, calendar: calendar), .thisWeek)
    }

    func testEightDaysAgoIsOlder() {
        let updated = date(2026, 7, 8, 9, 30)  // 8 dni temu
        XCTAssertEqual(ThreadSection.section(for: updated, now: now, calendar: calendar), .older)
    }

    func testFutureDateClampsToToday() {
        let updated = date(2026, 7, 17, 8, 0)  // clock skew ahead of "now"
        XCTAssertEqual(ThreadSection.section(for: updated, now: now, calendar: calendar), .today)
    }

    // MARK: Row metadata formatter — nil-resilience

    func testDrawerSubtitleAllNilsIsEmptyWithNoDanglingSeparators() {
        let subtitle = ThreadRailMeta.drawerSubtitle(
            model: nil, tokens: nil, updatedAt: nil, now: now, calendar: calendar)
        XCTAssertEqual(subtitle, "")
    }

    func testDrawerSubtitleFullTriple() {
        let subtitle = ThreadRailMeta.drawerSubtitle(
            model: "gpt-5", tokens: 1_234, updatedAt: date(2026, 7, 16, 14, 5),
            now: now, calendar: calendar)
        XCTAssertEqual(subtitle, "today 14:05 · gpt-5 · 1.2k tok")
    }

    func testDrawerSubtitleSkipsMissingMiddlePart() {
        let subtitle = ThreadRailMeta.drawerSubtitle(
            model: nil, tokens: 999, updatedAt: date(2026, 7, 15, 23, 59),
            now: now, calendar: calendar)
        XCTAssertEqual(subtitle, "yesterday · 999 tok")
    }

    func testDrawerSubtitleModelOnlyStripsProviderPrefixAndZeroTokensHidden() {
        let subtitle = ThreadRailMeta.drawerSubtitle(
            model: "openai/gpt-5", tokens: 0, updatedAt: nil, now: now, calendar: calendar)
        XCTAssertEqual(subtitle, "gpt-5")
    }

    func testDrawerSubtitleOlderDateUsesMonthDay() {
        let subtitle = ThreadRailMeta.drawerSubtitle(
            model: nil, tokens: nil, updatedAt: date(2026, 7, 8, 9, 30),
            now: now, calendar: calendar)
        XCTAssertEqual(subtitle, "Jul 8")
    }
}
