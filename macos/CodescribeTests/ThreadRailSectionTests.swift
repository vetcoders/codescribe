import XCTest

@testable import Codescribe

/// Pure-function coverage for the thread rail's recency sections and the
/// row metadata formatter (ThreadRail.swift). Fixed UTC calendar + injected
/// `now` keep every boundary deterministic regardless of host timezone.
final class ThreadRailSectionTests: XCTestCase {
    private struct DeliveryTurn: Decodable {
        let backendId: String
        let messageCount: Int
        let updatedAt: String

        enum CodingKeys: String, CodingKey {
            case backendId = "backend_id"
            case messageCount = "message_count"
            case updatedAt = "updated_at"
        }
    }

    private struct DeliveryReceipt: Decodable {
        let verifiedAt: String
        let backendId: String
        let indexRows: Int
        let first: DeliveryTurn
        let second: DeliveryTurn

        enum CodingKeys: String, CodingKey {
            case verifiedAt = "verified_at"
            case backendId = "backend_id"
            case indexRows = "index_rows"
            case first
            case second
        }
    }

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

    private func iso8601(_ value: String) throws -> Date {
        let fractional = ISO8601DateFormatter()
        fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = fractional.date(from: value) { return date }
        let wholeSeconds = ISO8601DateFormatter()
        guard let date = wholeSeconds.date(from: value) else {
            throw NSError(
                domain: "ThreadRailSectionTests", code: 1,
                userInfo: [NSLocalizedDescriptionKey: "Invalid ISO-8601 timestamp: \(value)"])
        }
        return date
    }

    private var w2ReceiptPath: String {
        if let configured = ProcessInfo.processInfo.environment["CODESCRIBE_W2_RECEIPT_PATH"] {
            return configured
        }
        let canonicalArtifact = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(
                ".vibecrafted/artifacts/vetcoders/codescribe/2026_0719/reports/e2e/W2-A-evidence/delivery-receipt.json"
            )
            .path
        if FileManager.default.fileExists(atPath: canonicalArtifact) {
            return canonicalArtifact
        }
        return URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()  // CodescribeTests
            .deletingLastPathComponent()  // macos
            .deletingLastPathComponent()  // repository root
            .appendingPathComponent("target/w2-a-evidence/delivery-receipt.json")
            .path
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

    /// Cross-surface W2-A canary: a Rust hard-degrade/two-turn test exports the
    /// actual ThreadDeliveryReceipt, and this XCTest feeds that persisted
    /// `updated_at` into the production rail bucketing function.
    func testPersistedDeliveryReceiptMapsToToday() throws {
        let path = w2ReceiptPath
        guard FileManager.default.fileExists(atPath: path) else {
            throw XCTSkip("W2-A receipt is generated only by the delivery verifier")
        }
        let data = try Data(contentsOf: URL(fileURLWithPath: path))
        let receipt = try JSONDecoder().decode(DeliveryReceipt.self, from: data)
        let firstUpdatedAt = try iso8601(receipt.first.updatedAt)
        let secondUpdatedAt = try iso8601(receipt.second.updatedAt)
        let verifiedAt = try iso8601(receipt.verifiedAt)

        XCTAssertEqual(receipt.indexRows, 1)
        XCTAssertEqual(receipt.backendId, receipt.first.backendId)
        XCTAssertEqual(receipt.backendId, receipt.second.backendId)
        XCTAssertEqual(receipt.first.messageCount, 2)
        XCTAssertEqual(receipt.second.messageCount, 4)
        XCTAssertGreaterThan(secondUpdatedAt, firstUpdatedAt)
        XCTAssertEqual(
            ThreadSection.section(for: secondUpdatedAt, now: verifiedAt, calendar: calendar),
            .today)
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

    func testPlaceholderRowUsesPresentedFirstMessageExcerpt() {
        let wire = "INSTRUKCJA_UŻYTKOWNIKA:\n<<<\nCompare insulin protocols\n>\n\nZAZNACZONY_TEKST: brak dostępnego zaznaczenia.\n"
        let thread = ChatThread(
            title: "<<<",
            meta: "today 11:45",
            messages: [ChatMessage(role: .you, timestamp: "11:45", text: wire)],
            updatedAt: date(2026, 7, 16, 11, 45)
        )

        let title = ThreadRowTitle.displayTitle(for: thread, now: now, calendar: calendar)

        XCTAssertEqual(title, "Compare insulin protocols")
        XCTAssertFalse(title.contains("<<<"))
    }

    func testPlaceholderRowWithoutLoadedMessagesUsesRelativeDateLabel() {
        let thread = ChatThread(
            title: "<<<",
            meta: "today 11:45",
            updatedAt: date(2026, 7, 16, 11, 45)
        )

        XCTAssertEqual(
            ThreadRowTitle.displayTitle(for: thread, now: now, calendar: calendar),
            "Today 11:45"
        )
        XCTAssertNil(ThreadTitlePolicy.normalized("<<<"))
        XCTAssertNil(ThreadTitlePolicy.normalized("<<< 2026-07-20"))
    }
}
