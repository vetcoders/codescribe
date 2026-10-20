import AppKit
import SwiftUI

/// One take the way the user remembers it: when, how long, what was said and
/// where it went. Nothing here runs a recognizer or opens the ledger: the row
/// is read from the take's truth sidecar (`sessions/<session>.wav.truth.json`)
/// that the controller writes when it retains the session audio.
struct RecentTake: Identifiable, Sendable, Equatable {
  /// The session id, which also names the retained audio and its sidecar.
  let id: String
  let recordedAt: Date
  let durationSeconds: Double
  let wordCount: Int
  /// The admitted words in order, joined with spaces.
  let text: String
  /// Ledger refusals recorded for the take; zero when the sidecar lists none.
  let refusedWords: Int
  /// Silero speech share of the take in percent, when the sidecar measured it.
  let speechPercent: Double?

  /// Where the take went, derived from the session name the controller
  /// assigns: `agent-channel-<n>-<uuid>` for the agent bus, anything else is
  /// dictation into an app.
  var destination: String {
    if let channel = RecentTake.agentChannel(session: id) {
      return String(localized: "Agent · channel \(channel)", comment: "Recent take destination")
    }
    return String(localized: "Dictation", comment: "Recent take destination")
  }

  static func agentChannel(session: String) -> Int? {
    let prefix = "agent-channel-"
    guard session.hasPrefix(prefix) else { return nil }
    let rest = session.dropFirst(prefix.count)
    guard let dash = rest.firstIndex(of: "-") else { return nil }
    return Int(rest[..<dash])
  }
}

/// Reads recent takes from the sessions directory under the config dir.
enum RecentTakesReader {
  /// The capture clock the sidecar's sample offsets count in when the WAV
  /// header is unreadable (a take cut short by a crash has a zeroed header).
  static let fallbackSampleRate = 48_000.0

  nonisolated static func read(root: String, limit: Int = 10) throws -> [RecentTake] {
    let directory = URL(fileURLWithPath: root).appendingPathComponent("sessions", isDirectory: true)
    let manager = FileManager.default
    guard manager.fileExists(atPath: directory.path) else { return [] }
    let suffix = ".wav.truth.json"
    let files = try manager.contentsOfDirectory(
      at: directory, includingPropertiesForKeys: [.contentModificationDateKey],
      options: [.skipsHiddenFiles]
    )
    .filter { $0.lastPathComponent.hasSuffix(suffix) }
    .map { url -> (URL, Date) in
      let modified =
        (try? url.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate)
        ?? .distantPast
      return (url, modified)
    }
    .sorted { $0.1 > $1.1 }
    .prefix(limit)

    return files.compactMap { url, modified in
      guard let data = try? Data(contentsOf: url) else { return nil }
      let session = String(url.lastPathComponent.dropLast(suffix.count))
      let audio = directory.appendingPathComponent(session + ".wav")
      return parse(data, session: session, audio: audio, fallbackDate: modified)
    }
  }

  nonisolated static func parse(_ data: Data, session: String, audio: URL?, fallbackDate: Date)
    -> RecentTake?
  {
    guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
      return nil
    }
    let words = (object["words"] as? [[String: Any]]) ?? []
    let texts = words.compactMap { $0["word"] as? String }.filter { !$0.isEmpty }
    let lastSample = words.compactMap { ($0["sample_end"] as? NSNumber)?.doubleValue }.max() ?? 0
    let recordedAt = (object["generated_at"] as? String).flatMap(parseDate) ?? fallbackDate
    let refusals = ((object["ledger"] as? [String: Any])?["refusals"] as? [Any])?.count ?? 0
    let speech = (object["vad_speech_pct"] as? NSNumber)?.doubleValue
    let duration = audio.flatMap(wavDuration) ?? (lastSample / fallbackSampleRate)
    return RecentTake(
      id: session, recordedAt: recordedAt, durationSeconds: duration,
      wordCount: texts.count, text: texts.joined(separator: " "),
      refusedWords: refusals, speechPercent: speech)
  }

  nonisolated static func parseDate(_ text: String) -> Date? {
    let fractional = ISO8601DateFormatter()
    fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    if let date = fractional.date(from: text) { return date }
    let plain = ISO8601DateFormatter()
    plain.formatOptions = [.withInternetDateTime]
    return plain.date(from: text)
  }

  /// Duration from the RIFF header: sample rate at byte 24, block align at 32,
  /// data length at 40. A header with a zero rate or length is not trusted.
  nonisolated static func wavDuration(_ url: URL) -> Double? {
    guard let handle = try? FileHandle(forReadingFrom: url) else { return nil }
    defer { try? handle.close() }
    guard let header = try? handle.read(upToCount: 44), header.count == 44 else { return nil }
    func word(_ offset: Int) -> UInt32 {
      header[offset..<offset + 4].reversed().reduce(0) { ($0 << 8) | UInt32($1) }
    }
    func half(_ offset: Int) -> UInt32 {
      header[offset..<offset + 2].reversed().reduce(0) { ($0 << 8) | UInt32($1) }
    }
    let rate = word(24)
    let blockAlign = half(32)
    let dataBytes = word(40)
    guard rate > 0, blockAlign > 0, dataBytes > 0 else { return nil }
    return Double(dataBytes) / Double(rate * blockAlign)
  }
}

/// Settings › Dictionary leads with the takes of this Mac in human terms:
/// time, length, words, destination and the words themselves. The acoustic
/// forensics of those takes (pins, PCM slots, Silero ranges) stay in Lab.
struct DictionaryRecentTakes: View {
  let configDir: String
  /// Jumps to the Lab desk, whose word pins default to the latest take.
  var openLab: (() -> Void)? = nil

  @State private var takes: [RecentTake] = []
  @State private var loaded = false
  @State private var refresh = 0

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      SettingsSectionHeaderRow(String(localized: "Recent takes")) {
        Button(String(localized: "Refresh")) { refresh += 1 }
          .buttonStyle(.plain)
          .font(CSFont.ui(11, .semibold))
          .foregroundStyle(CSColor.chromeAccent)
          .accessibilityIdentifier("dictionary-recent-takes-refresh")
      }
      freshness
        .padding(.top, CSSpace.control)
      ForEach(takes) { take in
        card(take)
          .padding(.top, CSSpace.control)
      }
    }
    .accessibilityIdentifier("dictionary-recent-takes")
    .task(id: "\(configDir)|\(refresh)") {
      let root = configDir
      let result = await Task.detached(priority: .utility) {
        (try? RecentTakesReader.read(root: root)) ?? []
      }.value
      guard !Task.isCancelled else { return }
      takes = result
      loaded = true
    }
  }

  @ViewBuilder
  private var freshness: some View {
    if let latest = takes.first {
      Text("Last take on this Mac: \(Self.dateLabel(latest.recordedAt))")
        .font(CSFont.ui(11.5, .medium))
        .foregroundStyle(Color.secondary)
        .accessibilityIdentifier("dictionary-last-take")
    } else if loaded {
      Text("No take has been recorded on this Mac yet.")
        .font(CSFont.ui(11.5, .medium))
        .foregroundStyle(Color.secondary)
        .accessibilityIdentifier("dictionary-last-take")
    } else {
      Text("Reading takes…")
        .font(CSFont.ui(11.5, .medium))
        .foregroundStyle(Color.secondary)
    }
  }

  private func card(_ take: RecentTake) -> some View {
    VStack(alignment: .leading, spacing: CSSpace.sm) {
      HStack(alignment: .firstTextBaseline, spacing: CSSpace.md) {
        Text(Self.dateLabel(take.recordedAt))
          .font(CSFont.ui(12.5, .semibold))
        Text(take.destination)
          .font(CSFont.ui(11.5))
          .foregroundStyle(Color.secondary)
        Text(Self.durationLabel(take.durationSeconds))
          .font(CSFont.mono(11))
          .foregroundStyle(Color.secondary)
        Text("\(take.wordCount) words")
          .font(CSFont.ui(11.5))
          .foregroundStyle(Color.secondary)
        Spacer()
        Button(String(localized: "Copy text")) {
          NSPasteboard.general.clearContents()
          NSPasteboard.general.setString(take.text, forType: .string)
        }
        .buttonStyle(.plain)
        .font(CSFont.ui(11, .semibold))
        .foregroundStyle(CSColor.chromeAccent)
        .disabled(take.text.isEmpty)
        if let openLab {
          Button(String(localized: "Open in Lab"), action: openLab)
            .buttonStyle(.plain)
            .font(CSFont.ui(11, .semibold))
            .foregroundStyle(CSColor.chromeAccent)
        }
      }
      if take.text.isEmpty {
        Text("No words were admitted for this take.")
          .font(CSFont.ui(12))
          .foregroundStyle(Color.secondary)
      } else {
        Text(take.text)
          .font(CSFont.ui(12.5))
          .lineLimit(4)
          .textSelection(.enabled)
      }
      if take.refusedWords > 0 {
        Text("\(take.refusedWords) words were refused by the ledger. Open Lab to see which.")
          .font(CSFont.ui(11))
          .foregroundStyle(Color.orange)
      }
    }
    .padding(CSSpace.md)
    .frame(maxWidth: .infinity, alignment: .leading)
    .background(
      RoundedRectangle(cornerRadius: 8, style: .continuous)
        .fill(Color.primary.opacity(0.04))
    )
  }

  static func dateLabel(_ date: Date) -> String {
    date.formatted(date: .abbreviated, time: .shortened)
  }

  static func durationLabel(_ seconds: Double) -> String {
    let whole = Int(seconds.rounded())
    if whole >= 60 {
      return String(format: "%d:%02d", whole / 60, whole % 60)
    }
    return String(format: "%d s", whole)
  }
}
