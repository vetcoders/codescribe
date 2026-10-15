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
}

struct MessageTimestampLabel: View {
  let date: Date?

  var body: some View {
    // History left open across midnight must acquire its new day label.
    TimelineView(.periodic(from: .now, by: 60)) { context in
      Text(verbatim: MessageTimestamp.label(for: date, now: context.date))
        .monospacedDigit()
        .lineLimit(1)
        .fixedSize(horizontal: true, vertical: false)
    }
  }
}
