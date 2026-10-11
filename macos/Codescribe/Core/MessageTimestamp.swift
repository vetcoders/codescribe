import Foundation
import SwiftUI

/// Message time remains an instant until presentation. The interface language
/// owns “Yesterday”; the user's system locale owns the clock's 12/24-hour form.
enum MessageTimestamp {
  static func date(fromISO8601 value: String) -> Date? {
    let formatter = ISO8601DateFormatter()
    formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    if let date = formatter.date(from: value) { return date }
    formatter.formatOptions = [.withInternetDateTime]
    return formatter.date(from: value)
  }

  static func label(
    for date: Date?, now: Date, calendar: Calendar = .autoupdatingCurrent,
    locale: Locale = .autoupdatingCurrent,
    yesterday: String = String(localized: "Yesterday", comment: "Message sent yesterday")
  ) -> String {
    guard let date else { return "—" }
    let clock = DateFormatter()
    clock.locale = locale
    clock.timeZone = calendar.timeZone
    clock.dateStyle = .none
    clock.timeStyle = .short
    let time = clock.string(from: date)
    if calendar.isDate(date, inSameDayAs: now) { return time }
    if let previousDay = calendar.date(byAdding: .day, value: -1, to: now),
      calendar.isDate(date, inSameDayAs: previousDay)
    {
      return "\(yesterday) \(time)"
    }
    let day = DateFormatter()
    day.locale = Locale(identifier: "en_US_POSIX")
    day.calendar = Calendar(identifier: .gregorian)
    day.timeZone = calendar.timeZone
    day.dateFormat = "yyyy-MM-dd"
    return "\(day.string(from: date)) \(time)"
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
