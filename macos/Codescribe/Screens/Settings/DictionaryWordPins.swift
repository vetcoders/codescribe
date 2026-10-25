import CoreFoundation
import Foundation
import SwiftUI

/// A read-only view of the engine's persisted slot receipts. A selected take is
/// explicit: correction text is never used to guess its acoustic identity.
struct DictionaryWordPins: View {
  let configDir: String
  /// Lab opens the latest take with its recorded acoustic evidence.
  @State private var expanded = DictionaryWordPins.expandedOnArrival
  static let expandedOnArrival = true
  @State private var session = ""
  @State private var refresh = 0
  @State private var snapshot: DictionaryPinSnapshot
  /// False until the first read of the current take has landed. Before
  /// that the section claims nothing: an empty snapshot is not "no slots".
  @State private var loaded: Bool
  @State private var selected: String?
  @State private var failed = false

  /// The last snapshot read per config dir and take, kept across visits.
  /// Returning to the Dictionary used to paint the empty state ("No recorded
  /// word slots…") and the cards under it for a frame or two, then the list
  /// landed and pushed everything down (Founder, 2026-10-20, slowed-down
  /// screencast). A revisit now opens on what it showed last and refreshes
  /// in place; only the very first visit waits, and it waits quietly.
  @MainActor private static var remembered: [String: DictionaryPinSnapshot] = [:]

  static func rememberedKey(configDir: String, session: String) -> String {
    "\(configDir)|\(session)"
  }

  @MainActor static func remember(
    _ snapshot: DictionaryPinSnapshot, configDir: String, session: String
  ) {
    remembered[rememberedKey(configDir: configDir, session: session)] = snapshot
  }

  @MainActor static func recall(configDir: String, session: String) -> DictionaryPinSnapshot? {
    remembered[rememberedKey(configDir: configDir, session: session)]
  }

  @MainActor static func forgetAll() { remembered.removeAll() }

  @MainActor init(configDir: String) {
    self.configDir = configDir
    let recalled = Self.recall(configDir: configDir, session: "")
    _snapshot = State(initialValue: recalled ?? DictionaryPinSnapshot())
    _loaded = State(initialValue: recalled != nil)
  }

  var body: some View {
    DisclosureGroup(isExpanded: $expanded) {
      VStack(alignment: .leading, spacing: 12) {
        HStack {
          Picker("Recording", selection: $session) {
            Text("Latest recorded take").tag("")
            ForEach(snapshot.sessions, id: \.self) { Text($0).tag($0) }
          }
          Button("Refresh") { refresh += 1 }
        }
        if failed {
          Text("Word pins could not be read. No timing was inferred.")
            .foregroundStyle(CSColor.secondaryInk)
        } else if !loaded {
          // First visit: the receipts are still being read. Hold the room
          // the list takes so nothing below jumps when it lands.
          Text("Reading the recorded slots…")
            .font(.caption).foregroundStyle(CSColor.secondaryInk)
            .frame(maxWidth: .infinity, minHeight: 280, alignment: .topLeading)
            .accessibilityIdentifier("dictionary-word-pins-reading")
        } else if snapshot.pins.isEmpty {
          Text("No recorded word slots for this take.")
            .foregroundStyle(CSColor.secondaryInk)
          speechBand
        } else {
          Text(snapshot.session).font(CSFont.mono(11)).textSelection(.enabled)
          Text("Pins in this take: \(snapshot.pins.count)")
            .font(.caption).foregroundStyle(CSColor.secondaryInk)
          Text("Select a pin to inspect its PCM range and source.")
            .font(.caption).foregroundStyle(CSColor.secondaryInk)
          ScrollView(.vertical) {
            LazyVStack(alignment: .leading, spacing: 6) {
              ForEach(snapshot.pins) { pin in
                Button {
                  selected = pin.id
                } label: {
                  HStack(spacing: 10) {
                    Text(verbatim: "#\(pin.index)").font(CSFont.mono(11)).frame(width: 38)
                    Text(pin.text).lineLimit(1).frame(width: 140, alignment: .leading)
                    GeometryReader { geometry in
                      let span = snapshot.timelineEnd(epoch: pin.epoch)
                      let left = Double(pin.start) / Double(span)
                      let length = Double(pin.end - pin.start) / Double(span)
                      ZStack(alignment: .leading) {
                        Capsule().fill(Color.secondary.opacity(0.15)).frame(height: 3)
                        Capsule().fill(CSColor.chromeAccent)
                          .frame(width: max(3, geometry.size.width * length), height: 8)
                          .offset(x: geometry.size.width * left)
                      }
                      .frame(height: 22)
                    }
                    .frame(minWidth: 100, maxWidth: .infinity, minHeight: 22, maxHeight: 22)
                    Text(pin.timeRange).font(CSFont.mono(11)).frame(width: 130)
                    Text(pin.producer).font(.caption).frame(width: 75, alignment: .leading)
                    Text(pin.speechState.label)
                      .font(.caption).foregroundStyle(pin.speechState.color)
                      .frame(width: 105, alignment: .leading)
                  }
                  .padding(6)
                  .background(selected == pin.id ? CSColor.chromeAccent.opacity(0.15) : .clear)
                  .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(Text(pin.text))
                .accessibilityValue(
                  Text(
                    verbatim:
                      "#\(pin.index) · \(pin.timeRange) · \(pin.producer) · \(pin.speechState.label)"
                  ))
              }
            }
            .csAutohidingScrollers()
          }
          .frame(maxHeight: 280)
          speechBand
          if let pin = snapshot.pins.first(where: { $0.id == selected }) {
            VStack(alignment: .leading, spacing: 5) {
              Text(pin.text).font(.headline)
              LabeledContent("PCM range", value: "\(pin.start)–\(pin.end) · \(pin.rate) Hz")
              LabeledContent("Capture epoch", value: String(pin.epoch))
              LabeledContent("Acoustic evidence", value: pin.speechState.label)
              LabeledContent(
                "Observation", value: "\(pin.producer) / \(pin.request) / \(pin.generation)")
              LabeledContent("Occurrence", value: "\(pin.occurrenceStart)–\(pin.occurrenceEnd)")
              DisclosureGroup("Recorded adjudication") {
                Text(pin.adjudication).font(CSFont.mono(11)).textSelection(.enabled)
              }
            }
            .font(CSFont.mono(11))
            .textSelection(.enabled)
          }
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
          loaded = true
          if let result { Self.remember(result, configDir: root, session: requested) }
          if !snapshot.pins.contains(where: { $0.id == selected }) {
            selected = snapshot.pins.first?.id
          }
          try? await Task.sleep(for: .seconds(2))
        }
      }
    } label: {
      Label("Word pins and PCM slots", systemImage: "pin.circle")
    }
    .accessibilityIdentifier("lab-word-pins")
  }

  private var speechBand: some View {
    VStack(alignment: .leading, spacing: 6) {
      Text("Silero speech · whole take")
        .font(.caption).foregroundStyle(CSColor.secondaryInk)
      ForEach(snapshot.epochs, id: \.self) { epoch in
        let end = snapshot.timelineEnd(epoch: epoch)
        let ranges = snapshot.speechRanges.filter { $0.epoch == epoch }
        HStack(spacing: 10) {
          Text(verbatim: "#\(epoch)").font(CSFont.mono(11)).frame(width: 38)
          Text(verbatim: "Silero").font(CSFont.mono(11)).frame(width: 140, alignment: .leading)
          GeometryReader { geometry in
            ZStack(alignment: .leading) {
              Capsule().fill(Color.secondary.opacity(0.15)).frame(height: 4)
              ForEach(Array(ranges.enumerated()), id: \.offset) { _, range in
                Rectangle().fill(Color.green.opacity(0.8))
                  .frame(
                    width: max(
                      1, geometry.size.width * Double(range.end - range.start) / Double(end)),
                    height: 10
                  )
                  .offset(x: geometry.size.width * Double(range.start) / Double(end))
              }
            }
            .frame(height: 16)
          }
          .frame(minWidth: 100, maxWidth: .infinity, minHeight: 16, maxHeight: 16)
          Group {
            if let rate = snapshot.sampleRates[epoch] {
              Text(verbatim: String(format: "%.3f s", Double(end) / Double(rate)))
            } else {
              Text(verbatim: "—")
            }
          }
          .font(CSFont.mono(11)).frame(width: 130)
          Color.clear.frame(width: 75, height: 1).accessibilityHidden(true)
          Color.clear.frame(width: 105, height: 1).accessibilityHidden(true)
        }
        .padding(.horizontal, 6)
        if snapshot.speechObserved[epoch] == nil {
          Text("No VAD recorded for this capture epoch.")
            .font(.caption).foregroundStyle(CSColor.secondaryInk)
        }
      }
    }
    .accessibilityIdentifier("lab-word-pins-silero-band")
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
  var speechState: DictionaryPinSpeechState = .noVAD

  var timeRange: String {
    String(format: "%.3f–%.3f s", Double(start) / Double(rate), Double(end) / Double(rate))
  }
}

struct DictionaryPinSnapshot: Sendable {
  var sessions: [String] = []
  var session = ""
  var pins: [DictionaryPin] = []
  var speechRanges: [RecordedPinRange] = []
  var speechObserved: [UInt64: UInt64] = [:]
  var sampleRates: [UInt64: UInt64] = [:]

  var epochs: [UInt64] { Array(Set(pins.map(\.epoch)).union(speechObserved.keys)).sorted() }

  func timelineEnd(epoch: UInt64) -> UInt64 {
    max(
      1,
      max(
        speechObserved[epoch] ?? 0,
        pins.filter { $0.epoch == epoch }.map(\.occurrenceEnd).max() ?? 0))
  }

  nonisolated static func read(root: String, requested: String) throws -> Self {
    let directory = URL(fileURLWithPath: root).appendingPathComponent("sessions", isDirectory: true)
    let manager = FileManager.default
    guard manager.fileExists(atPath: directory.path) else { return Self() }
    let suffixes = [".slots.jsonl", ".trail.jsonl"]
    // Read each resource date once; a comparator must never perform filesystem IO.
    let files = try manager.contentsOfDirectory(
      at: directory, includingPropertiesForKeys: [.creationDateKey, .contentModificationDateKey],
      options: [.skipsHiddenFiles]
    ).compactMap { url -> (String, URL, Date)? in
      guard let suffix = suffixes.first(where: { url.lastPathComponent.hasSuffix($0) }) else {
        return nil
      }
      let values = try? url.resourceValues(forKeys: [.creationDateKey, .contentModificationDateKey])
      let recorded = values?.creationDate ?? values?.contentModificationDate ?? .distantPast
      return (String(url.lastPathComponent.dropLast(suffix.count)), url, recorded)
    }.sorted { $0.2 > $1.2 }
    var result = Self()
    var seen = Set<String>()
    for (session, _, _) in files where seen.insert(session).inserted {
      result.sessions.append(session)
    }
    guard
      let session = requested.isEmpty
        ? result.sessions.first : result.sessions.first(where: { $0 == requested })
    else {
      return result
    }
    result.session = session
    var occurrences: [String: [String: Any]] = [:]
    var rates: [UInt64: UInt64] = [:]
    var subFloor: [RecordedPinRange] = []
    let trail = directory.appendingPathComponent(session + ".trail.jsonl")
    if manager.fileExists(atPath: trail.path) {
      try readLines(trail) { row in
        guard row["schema"] as? String == "codescribe.decision-trail.v3",
          row["session"] as? String == session, let epoch = number(row["capture_epoch"])
        else { throw CocoaError(.fileReadCorruptFile) }
        let operation = row["operation"] as? [String: Any]
        if let rate = number(operation?["capture_rate_hz"]), rate > 0 {
          if let previous = rates[epoch], previous != rate {
            throw CocoaError(.fileReadCorruptFile)
          }
          rates[epoch] = rate
        }
        if let speech = (operation?["speech"] ?? row["speech"]) as? [String: Any],
          speech["producer"] as? String == "silero_boundaries",
          speech["session"] as? String == session,
          number(speech["capture_epoch"]) == epoch,
          let observed = number(speech["observed_samples"]),
          observed >= (result.speechObserved[epoch] ?? 0)
        {
          guard let ranges = speech["ranges"] as? [[String: Any]] else {
            throw CocoaError(.fileReadCorruptFile)
          }
          let measured = try ranges.map { range -> RecordedPinRange in
            guard range["session"] as? String == session,
              number(range["capture_epoch"]) == epoch,
              let a = number(range["sample_start"]), let b = number(range["sample_end"]),
              a < b, b <= observed
            else { throw CocoaError(.fileReadCorruptFile) }
            return RecordedPinRange(epoch: epoch, start: a, end: b)
          }
          result.speechObserved[epoch] = observed
          result.speechRanges.removeAll { $0.epoch == epoch }
          result.speechRanges.append(contentsOf: measured)
        }
        if row["kind"] as? String == "qualification",
          let evidence = row["evidence"] as? [String: Any],
          let calibration = row["calibration"] as? [String: Any],
          let version = calibration["version"] as? String,
          evidence["evidence_calibration_version"] as? String == version,
          let integral = evidence["energy_integral"] as? Double,
          let floor = calibration["min_energy_integral"] as? Double,
          integral.isFinite, floor.isFinite, integral >= 0, integral < floor,
          let occurrence = evidence["occurrence"] as? [String: Any],
          occurrence["session"] as? String == session,
          number(occurrence["capture_epoch"]) == epoch,
          let a = number(occurrence["sample_start"]), let b = number(occurrence["sample_end"]),
          a < b
        {
          subFloor.append(RecordedPinRange(epoch: epoch, start: a, end: b))
        }
        if row["kind"] as? String == "slot_end", let operation,
          let observation = operation["observation"] as? [String: Any],
          let occurrence = observation["occurrence"] as? [String: Any],
          occurrence["session"] as? String == session,
          number(occurrence["capture_epoch"]) == epoch,
          let a = number(occurrence["sample_start"]), let b = number(occurrence["sample_end"]),
          a < b,
          let rate = rates[epoch], let slots = operation["result_slots"] as? [[String: Any]]
        {
          // A take is every completed occurrence snapshot, not the last operation.
          occurrences["\(epoch):\(a):\(b)"] = [
            "capture_epoch": epoch, "sample_start": a, "sample_end": b,
            "sample_rate_hz": rate, "slots": slots,
          ]
        }
      }
    }
    let slots = directory.appendingPathComponent(session + ".slots.jsonl")
    if manager.fileExists(atPath: slots.path) {
      try readLines(slots) { row in
        guard row["schema"] as? String == "codescribe.occurrence_slots.v1",
          row["session"] as? String == session,
          let epoch = number(row["capture_epoch"]), let a = number(row["sample_start"]),
          let b = number(row["sample_end"]), a < b
        else { throw CocoaError(.fileReadCorruptFile) }
        // Seals carry finality receipts for the same exact occurrence.
        occurrences["\(epoch):\(a):\(b)"] = row
      }
    }
    for (identity, row) in occurrences {
      guard let a = number(row["sample_start"]), let b = number(row["sample_end"]),
        let epoch = number(row["capture_epoch"]), let rate = number(row["sample_rate_hz"]),
        rate > 0,
        let slots = row["slots"] as? [[String: Any]]
      else { throw CocoaError(.fileReadCorruptFile) }
      if let previous = rates[epoch], previous != rate {
        throw CocoaError(.fileReadCorruptFile)
      }
      rates[epoch] = rate
      let finality = row["word_finality"] as? [[String: Any]] ?? []
      for (index, slot) in slots.enumerated() {
        guard let start = number(slot["sample_start"]), let end = number(slot["sample_end"]),
          a <= start, start < end, end <= b,
          let text = slot["text"] as? String, let producer = slot["producer"] as? String,
          let observation = slot["observation"] as? [String: Any],
          let request = number(observation["request"]),
          let generation = number(observation["generation"])
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
              && number(observed["request"]) == request
              && number(observed["generation"]) == generation
              && (observed["producer"] as? String)?.lowercased() == producer.lowercased()
          }
        }
        let encoded = try JSONSerialization.data(
          withJSONObject: evidence, options: [.prettyPrinted, .sortedKeys])
        result.pins.append(
          DictionaryPin(
            id:
              "\(result.session):\(identity):\(producer):\(request):\(generation):\(start):\(end)",
            index: index, text: text, producer: producer,
            start: start, end: end, occurrenceStart: a, occurrenceEnd: b, epoch: epoch,
            request: request, generation: generation, rate: rate,
            adjudication: String(decoding: encoded, as: UTF8.self)))
      }
    }
    guard Set(result.pins.map(\.id)).count == result.pins.count else {
      throw CocoaError(.fileReadCorruptFile)
    }
    result.sampleRates = rates
    for index in result.pins.indices {
      let pin = result.pins[index]
      if result.speechRanges.contains(where: {
        $0.epoch == pin.epoch && $0.start < pin.end && pin.start < $0.end
      }) {
        result.pins[index].speechState = .speech
      } else if subFloor.contains(where: {
        $0.epoch == pin.epoch && $0.start <= pin.start && pin.end <= $0.end
      }) {
        result.pins[index].speechState = .subFloor
      } else if let observed = result.speechObserved[pin.epoch], pin.end <= observed {
        result.pins[index].speechState = .noSpeech
      }
    }
    result.pins.sort { ($0.epoch, $0.start, $0.id) < ($1.epoch, $1.start, $1.id) }
    return result
  }

  /// Stream the entire take. Bound one row rather than silently truncating long takes.
  /// An append still in flight is ignored; a malformed completed row is an error.
  nonisolated private static func readLines(
    _ url: URL, consume: ([String: Any]) throws -> Void
  ) throws {
    let handle = try FileHandle(forReadingFrom: url)
    defer { try? handle.close() }
    var pending = Data()
    while let chunk = try handle.read(upToCount: 65_536), !chunk.isEmpty {
      pending.append(chunk)
      while let end = pending.firstIndex(of: 10) {
        let line = pending[..<end]
        guard line.count <= 8_000_000,
          let row = try JSONSerialization.jsonObject(with: Data(line)) as? [String: Any]
        else { throw CocoaError(.fileReadCorruptFile) }
        try consume(row)
        pending.removeSubrange(...end)
      }
      guard pending.count <= 8_000_000 else { throw CocoaError(.fileReadTooLarge) }
    }
    if !pending.isEmpty,
      let row = (try? JSONSerialization.jsonObject(with: pending)) as? [String: Any]
    {
      try consume(row)
    }
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

/// A persisted range on one capture epoch; text never participates in its identity.
struct RecordedPinRange: Sendable {
  let epoch: UInt64
  let start: UInt64
  let end: UInt64
}

enum DictionaryPinSpeechState: Sendable {
  case speech
  case subFloor
  case noSpeech
  case noVAD

  var label: String {
    switch self {
    case .speech: String(localized: "Speech")
    case .subFloor: String(localized: "Sub-floor energy")
    case .noSpeech: String(localized: "No speech on range")
    case .noVAD: String(localized: "No VAD recorded")
    }
  }

  var color: Color {
    switch self {
    case .speech: CSColor.oliveLight
    case .subFloor: CSColor.amberLight
    case .noSpeech, .noVAD: CSColor.secondaryInk
    }
  }
}
