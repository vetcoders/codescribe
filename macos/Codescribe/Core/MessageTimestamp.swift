import Foundation
import SwiftUI
import os

/// Message time remains an instant until presentation. The interface language
/// owns “Yesterday”; the user's system locale owns the clock's 12/24-hour form.
///
/// Every conversation bubble parses and labels its time on each layout pass,
/// and a live resize lays the whole list out on every step. Building ICU
/// formatters per call cost 24 ms per pass for 60 bubbles (2026-10-14), more
/// than a frame; the parsers and clocks are built once per locale and zone.
enum MessageTimestamp {
  /// `ISO8601DateFormatter` is not declared Sendable; the lock owns both
  /// parsers. A message's timestamp never changes, so a parsed instant is kept;
  /// the memo starts over past `memoLimit` entries.
  private struct Parsers {
    let fractional = makeParser([.withInternetDateTime, .withFractionalSeconds])
    let whole = makeParser([.withInternetDateTime])
    var memo: [String: Date?] = [:]
  }

  static let memoLimit = 4_096
  private static let parsers = OSAllocatedUnfairLock(uncheckedState: Parsers())

  private static func makeParser(_ options: ISO8601DateFormatter.Options) -> ISO8601DateFormatter {
    let formatter = ISO8601DateFormatter()
    formatter.formatOptions = options
    return formatter
  }

  private struct ClockKey: Hashable, Sendable {
    let locale: Locale
    let timeZone: TimeZone
  }

  /// `DateFormatter` is thread safe and Sendable; one per locale and zone.
  private static let clocks = OSAllocatedUnfairLock(initialState: [ClockKey: DateFormatter]())

  /// A changed region or time zone preference rebuilds the clocks, as a fresh
  /// formatter per call used to.
  private static let flushesOnPreferenceChange: Bool = {
    for name in [NSLocale.currentLocaleDidChangeNotification, .NSSystemTimeZoneDidChange] {
      _ = NotificationCenter.default.addObserver(forName: name, object: nil, queue: nil) { _ in
        clocks.withLock { $0.removeAll() }
      }
    }
    return true
  }()

  private static func clock(locale: Locale, timeZone: TimeZone) -> DateFormatter {
    _ = flushesOnPreferenceChange
    let key = ClockKey(locale: locale, timeZone: timeZone)
    return clocks.withLock { cache in
      if let clock = cache[key] { return clock }
      let clock = DateFormatter()
      clock.locale = locale
      clock.timeZone = timeZone
      clock.dateStyle = .none
      clock.timeStyle = .short
      cache[key] = clock
      return clock
    }
  }

  static func date(fromISO8601 value: String) -> Date? {
    parsers.withLockUnchecked { parsers in
      if let known = parsers.memo[value] { return known }
      let date = parsers.fractional.date(from: value) ?? parsers.whole.date(from: value)
      if parsers.memo.count >= memoLimit { parsers.memo.removeAll(keepingCapacity: true) }
      parsers.memo[value] = date
      return date
    }
  }

  static func label(
    for date: Date?, now: Date, calendar: Calendar = .autoupdatingCurrent,
    locale: Locale = .autoupdatingCurrent,
    yesterday: String = String(localized: "Yesterday", comment: "Message sent yesterday")
  ) -> String {
    guard let date else { return "—" }
    let time = clock(locale: locale, timeZone: calendar.timeZone).string(from: date)
    if calendar.isDate(date, inSameDayAs: now) { return time }
    if let previousDay = calendar.date(byAdding: .day, value: -1, to: now),
      calendar.isDate(date, inSameDayAs: previousDay)
    {
      return "\(yesterday) \(time)"
    }
    let day = date.formatted(
      Date.ISO8601FormatStyle(timeZone: calendar.timeZone).year().month().day())
    return "\(day) \(time)"
  }

  /// Short form for tight places (the sidebar roster): the clock for today,
  /// the word alone for yesterday, a short numeric date for anything older.
  static func compactLabel(
    for date: Date?, now: Date, calendar: Calendar = .autoupdatingCurrent,
    locale: Locale = .autoupdatingCurrent,
    yesterday: String = String(localized: "Yesterday", comment: "Message sent yesterday")
  ) -> String {
    guard let date else { return "—" }
    if calendar.isDate(date, inSameDayAs: now) {
      return clock(locale: locale, timeZone: calendar.timeZone).string(from: date)
    }
    if let previousDay = calendar.date(byAdding: .day, value: -1, to: now),
      calendar.isDate(date, inSameDayAs: previousDay)
    {
      return yesterday
    }
    return date.formatted(
      Date.FormatStyle(
        date: .numeric, time: .omitted, locale: locale, calendar: calendar,
        timeZone: calendar.timeZone))
  }

  /// Day separator between conversation turns: the word for today and
  /// yesterday, otherwise the calendar date of the full `label` path. It names
  /// the day once, so the turns themselves can carry the compact time.
  static func dayLabel(
    for date: Date, now: Date, calendar: Calendar = .autoupdatingCurrent,
    today: String = String(localized: "Today", comment: "Day separator between conversation turns"),
    yesterday: String = String(localized: "Yesterday", comment: "Message sent yesterday")
  ) -> String {
    if calendar.isDate(date, inSameDayAs: now) { return today }
    if let previousDay = calendar.date(byAdding: .day, value: -1, to: now),
      calendar.isDate(date, inSameDayAs: previousDay)
    {
      return yesterday
    }
    return date.formatted(
      Date.ISO8601FormatStyle(timeZone: calendar.timeZone).year().month().day())
  }

  /// A turn opens a new day when it and the previous dated turn fall on
  /// different calendar days. A missing time never starts or breaks a day.
  static func startsNewDay(
    previous: Date?, current: Date?, calendar: Calendar = .autoupdatingCurrent
  ) -> Bool {
    guard let previous, let current else { return false }
    return !calendar.isDate(previous, inSameDayAs: current)
  }
}

struct MessageTimestampLabel: View {
  let date: Date?
  /// Tight places (the sidebar roster, the turn meta rows) take the short
  /// form; the full date and time stay in its tooltip and accessibility label.
  var compact: Bool = false

  var body: some View {
    // History left open across midnight must acquire its new day label.
    TimelineView(.periodic(from: .now, by: 60)) { context in
      let full = MessageTimestamp.label(for: date, now: context.date)
      if compact {
        Text(verbatim: MessageTimestamp.compactLabel(for: date, now: context.date))
          .monospacedDigit()
          .lineLimit(1)
          .fixedSize(horizontal: true, vertical: false)
          .help(full)
          .accessibilityLabel(Text(verbatim: full))
      } else {
        Text(verbatim: full)
          .monospacedDigit()
          .lineLimit(1)
          .fixedSize(horizontal: true, vertical: false)
      }
    }
  }
}

/// Names the calendar day once where the conversation crosses midnight, so
/// each turn can show only its compact time. Same quiet mono caption as the
/// turn meta rows, between two hairlines of the bubbles' border tone.
struct MessageDaySeparator: View {
  let date: Date

  var body: some View {
    TimelineView(.periodic(from: .now, by: 60)) { context in
      HStack(spacing: 10) {
        hairline
        Text(verbatim: MessageTimestamp.dayLabel(for: date, now: context.date))
          .font(CSFont.mono(10, .medium))
          .foregroundStyle(CSColor.textTertiary)
          .lineLimit(1)
          .fixedSize()
        hairline
      }
      .accessibilityElement(children: .combine)
      .accessibilityAddTraits(.isHeader)
    }
  }

  private var hairline: some View {
    Rectangle()
      .fill(Color.primary.opacity(0.07))
      .frame(height: 1)
      .frame(maxWidth: .infinity)
  }
}
