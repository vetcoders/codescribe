import CoreFoundation
import Foundation
import SwiftUI

/// A read-only view of the engine's persisted slot receipts. A selected take is
/// explicit: correction text is never used to guess its acoustic identity.
struct DictionaryWordPins: View {
  let configDir: String
  @State private var expanded = false
  @State private var session = ""
  @State private var refresh = 0
  @State private var snapshot = DictionaryPinSnapshot()
  @State private var selected: String?
  @State private var failed = false

  var body: some View {
    DisclosureGroup(isExpanded: $expanded) {
      VStack(alignment: .leading, spacing: 12) {
        HStack {
          Picker("Recording", selection: $session) {
            Text("Latest recorded slots").tag("")
            ForEach(snapshot.sessions, id: \.self) { Text($0).tag($0) }
          }
          Button("Refresh") { refresh += 1 }
        }
        if failed {
          Text("Word pins could not be read. No timing was inferred.")
            .foregroundStyle(.secondary)
        } else if snapshot.pins.isEmpty {
          Text("No recorded word slots for this take.")
            .foregroundStyle(.secondary)
        } else {
          Text(snapshot.session).font(.caption.monospaced()).textSelection(.enabled)
          Text("Select a pin to inspect its PCM range and source.")
            .font(.caption).foregroundStyle(.secondary)
          ScrollView(.vertical) {
            LazyVStack(alignment: .leading, spacing: 6) {
              ForEach(snapshot.pins) { pin in
                Button { selected = pin.id } label: {
                  HStack(spacing: 10) {
                    Text(verbatim: "#\(pin.index)").font(.caption.monospaced()).frame(width: 38)
                    Text(pin.text).lineLimit(1).frame(width: 140, alignment: .leading)
                    GeometryReader { geometry in
                      let span = max(1, pin.occurrenceEnd - pin.occurrenceStart)
                      let left = Double(pin.start - pin.occurrenceStart) / Double(span)
                      let length = Double(pin.end - pin.start) / Double(span)
                      ZStack(alignment: .leading) {
                        Capsule().fill(Color.secondary.opacity(0.15)).frame(height: 3)
                        Capsule().fill(Color.accentColor)
                          .frame(width: max(3, geometry.size.width * length), height: 8)
                          .offset(x: geometry.size.width * left)
                      }
                      .frame(height: 22)
                    }
                    .frame(minWidth: 100, maxWidth: .infinity, minHeight: 22, maxHeight: 22)
                    Text(pin.timeRange).font(.caption.monospaced()).frame(width: 130)
                    Text(pin.producer).font(.caption).frame(width: 75, alignment: .leading)
                  }
                  .padding(6)
                  .background(selected == pin.id ? Color.accentColor.opacity(0.15) : .clear)
                  .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(Text(pin.text))
                .accessibilityValue(Text(verbatim: "#\(pin.index) · \(pin.timeRange) · \(pin.producer)"))
              }
            }
          }
          .frame(maxHeight: 280)
          if let pin = snapshot.pins.first(where: { $0.id == selected }) {
            VStack(alignment: .leading, spacing: 5) {
              Text(pin.text).font(.headline)
              LabeledContent("PCM range", value: "\(pin.start)–\(pin.end) · \(pin.rate) Hz")
              LabeledContent("Capture epoch", value: String(pin.epoch))
              LabeledContent("Observation", value: "\(pin.producer) / \(pin.request) / \(pin.generation)")
              LabeledContent("Occurrence", value: "\(pin.occurrenceStart)–\(pin.occurrenceEnd)")
              DisclosureGroup("Recorded adjudication") {
                Text(pin.adjudication).font(.caption.monospaced()).textSelection(.enabled)
              }
            }
            .font(.caption.monospaced())
            .textSelection(.enabled)
          }
          Text("Bars show each pin inside its occurrence. Slot timing does not certify delivery. Missing Silero or CTC evidence is not reconstructed.")
            .font(.caption).foregroundStyle(.secondary)
        }
      }
      .padding(.top, 10)
      .task(id: "\(configDir)|\(session)|\(refresh)|\(expanded)") {
        guard expanded else { return }
        let root = configDir
        let requested = session
        while !Task.isCancelled {
          let result = await Task.detached(priority: .utility) {
            try? DictionaryPinSnapshot.read(root: root, requested: requested)
          }.value
          guard !Task.isCancelled else { return }
          failed = result == nil
          snapshot = result ?? DictionaryPinSnapshot()
          if !snapshot.pins.contains(where: { $0.id == selected }) {
            selected = snapshot.pins.first?.id
          }
          try? await Task.sleep(for: .seconds(2))
        }
      }
    } label: {
      Label("Word pins and PCM slots", systemImage: "pin.circle")
    }
    .accessibilityIdentifier("dictionary-word-pins")
  }
}

struct DictionaryPin: Identifiable, Sendable {
  let id: String
  let index: Int
  let text: String
  let producer: String
  let start: UInt64
  let end: UInt64
  let occurrenceStart: UInt64
  let occurrenceEnd: UInt64
  let epoch: UInt64
  let request: UInt64
  let generation: UInt64
  let rate: UInt64
  let adjudication: String

  var timeRange: String {
    String(format: "%.3f–%.3f s", Double(start) / Double(rate), Double(end) / Double(rate))
  }
}

struct DictionaryPinSnapshot: Sendable {
  var sessions: [String] = []
  var session = ""
  var pins: [DictionaryPin] = []

  nonisolated static func read(root: String, requested: String) throws -> Self {
    let directory = URL(fileURLWithPath: root).appendingPathComponent("sessions", isDirectory: true)
    let manager = FileManager.default
    guard manager.fileExists(atPath: directory.path) else { return Self() }
    let suffix = ".slots.jsonl"
    let files = try manager.contentsOfDirectory(
      at: directory, includingPropertiesForKeys: [.contentModificationDateKey, .fileSizeKey],
      options: [.skipsHiddenFiles]
    ).filter { $0.lastPathComponent.hasSuffix(suffix) }.sorted {
      let a = (try? $0.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate) ?? .distantPast
      let b = (try? $1.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate) ?? .distantPast
      return a > b
    }
    let candidates = Array(files.prefix(30))
    var result = Self(sessions: candidates.map { String($0.lastPathComponent.dropLast(suffix.count)) })
    let url: URL?
    if requested.isEmpty {
      url = candidates.first { ((try? $0.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0) > 0 }
    } else {
      url = candidates.first { $0.lastPathComponent == requested + suffix }
    }
    guard let url else { return result }
    let size = try url.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
    guard size <= 8_000_000 else { throw CocoaError(.fileReadTooLarge) }
    let data = try Data(contentsOf: url)
    guard data.count <= 8_000_000 else { throw CocoaError(.fileReadTooLarge) }
    result.session = String(url.lastPathComponent.dropLast(suffix.count))
    // Append-only receipts: latest snapshot for each exact physical occurrence.
    var occurrences: [String: [String: Any]] = [:]
    let lines = data.split(separator: 10)
    for (index, line) in lines.enumerated() {
      guard let row = (try? JSONSerialization.jsonObject(with: Data(line))) as? [String: Any] else {
        if index == lines.count - 1 && data.last != 10 { break }
        throw CocoaError(.fileReadCorruptFile)
      }
      guard row["schema"] as? String == "codescribe.occurrence_slots.v1",
        row["session"] as? String == result.session,
        let epoch = number(row["capture_epoch"]), let a = number(row["sample_start"]),
        let b = number(row["sample_end"]), a < b
      else { throw CocoaError(.fileReadCorruptFile) }
      occurrences["\(epoch):\(a):\(b)"] = row
    }
    for (identity, row) in occurrences {
      guard let a = number(row["sample_start"]), let b = number(row["sample_end"]),
        let epoch = number(row["capture_epoch"]), let rate = number(row["sample_rate_hz"]), rate > 0,
        let slots = row["slots"] as? [[String: Any]]
      else { throw CocoaError(.fileReadCorruptFile) }
      let finality = row["word_finality"] as? [[String: Any]] ?? []
      for (index, slot) in slots.enumerated() {
        guard let start = number(slot["sample_start"]), let end = number(slot["sample_end"]),
          a <= start, start < end, end <= b,
          let text = slot["text"] as? String, let producer = slot["producer"] as? String,
          let observation = slot["observation"] as? [String: Any],
          let request = number(observation["request"]), let generation = number(observation["generation"])
        else { throw CocoaError(.fileReadCorruptFile) }
        let evidence = finality.filter { item in
          (item["targets"] as? [[String: Any]] ?? []).contains { target in
            guard let observed = target["observation"] as? [String: Any],
              let occurrence = observed["occurrence"] as? [String: Any],
              occurrence["session"] as? String == result.session,
              number(occurrence["capture_epoch"]) == epoch,
              number(occurrence["sample_start"]) == a,
              number(occurrence["sample_end"]) == b
            else { return false }
            return number(target["sample_start"]) == start && number(target["sample_end"]) == end
              && number(observed["request"]) == request && number(observed["generation"]) == generation
              && (observed["producer"] as? String)?.lowercased() == producer.lowercased()
          }
        }
        let encoded = try JSONSerialization.data(withJSONObject: evidence, options: [.prettyPrinted, .sortedKeys])
        result.pins.append(DictionaryPin(
          id: "\(result.session):\(identity):\(producer):\(request):\(generation):\(start):\(end)",
          index: index, text: text, producer: producer,
          start: start, end: end, occurrenceStart: a, occurrenceEnd: b, epoch: epoch,
          request: request, generation: generation, rate: rate,
          adjudication: String(decoding: encoded, as: UTF8.self)))
      }
    }
    guard Set(result.pins.map(\.id)).count == result.pins.count else {
      throw CocoaError(.fileReadCorruptFile)
    }
    result.pins.sort { ($0.epoch, $0.start, $0.id) < ($1.epoch, $1.start, $1.id) }
    return result
  }

  nonisolated private static func number(_ value: Any?) -> UInt64? {
    guard let value = value as? NSNumber, CFGetTypeID(value) != CFBooleanGetTypeID(),
      value.doubleValue.isFinite, value.doubleValue >= 0,
      value.doubleValue.rounded(.towardZero) == value.doubleValue,
      value.doubleValue < Double(UInt64.max)
    else { return nil }
    return value.uint64Value
  }
}
