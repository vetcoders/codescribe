import XCTest

@testable import Codescribe

final class DictionaryRecentTakesTests: XCTestCase {
  private func makeRoot() throws -> URL {
    let root = FileManager.default.temporaryDirectory
      .appendingPathComponent("recent-takes-\(UUID().uuidString)", isDirectory: true)
    try FileManager.default.createDirectory(
      at: root.appendingPathComponent("sessions"), withIntermediateDirectories: true)
    return root
  }

  private func writeSidecar(
    _ root: URL, session: String, generatedAt: String?, words: [(String, Int, Int)],
    terminal: String? = nil, speechSamples: Int = 0, coveredSamples: Int = 0,
    refusals: [String] = [], speech: Double? = nil
  ) throws {
    // The ledger block mirrors a production sidecar: refusals are bare reason
    // codes, one per refused receipt, and coverage counts Silero speech samples.
    var ledger: [String: Any] = [
      "refusals": refusals,
      "coverage": ["speech_samples": speechSamples, "covered_samples": coveredSamples],
    ]
    if let terminal { ledger["terminal"] = ["status": terminal] }
    var object: [String: Any] = [
      "schema_version": 2,
      "session_id": session,
      "words": words.map { ["word": $0.0, "sample_start": $0.1, "sample_end": $0.2, "pin": "whisper"] },
      "ledger": ledger,
    ]
    if let generatedAt { object["generated_at"] = generatedAt }
    if let speech { object["vad_speech_pct"] = speech }
    let data = try JSONSerialization.data(withJSONObject: object)
    try data.write(
      to: root.appendingPathComponent("sessions/\(session).wav.truth.json"))
  }

  func testReadsTakesNewestFirstWithWordsDestinationAndLedgerState() throws {
    let root = try makeRoot()
    defer { try? FileManager.default.removeItem(at: root) }
    try writeSidecar(
      root, session: "agent-channel-3-aaaa", generatedAt: "2026-10-20T22:59:20.104496+00:00",
      words: [("Wiesz,", 90_240, 107_520), ("tak", 110_000, 1_495_680)],
      terminal: "refused", speechSamples: 1_000_000, coveredSamples: 297_000,
      refusals: Array(repeating: "replayed_range_identity", count: 5) + ["replaced_by_whisper"],
      speech: 67.3)
    try writeSidecar(
      root, session: "b1c63d87-dictation", generatedAt: "2026-10-21T00:05:00+00:00",
      words: [("Dobra.", 0, 48_000)], terminal: "sealed", speechSamples: 40_000,
      coveredSamples: 40_000)
    try writeSidecar(root, session: "agent-channel-1-empty", generatedAt: nil, words: [])

    let takes = try RecentTakesReader.read(root: root.path)
    XCTAssertEqual(takes.count, 3)

    // Explicit timestamps order the rows; the sidecar without one falls back
    // to its file date, which is "now" and therefore newest.
    XCTAssertEqual(takes[0].id, "agent-channel-1-empty")
    XCTAssertEqual(takes[0].wordCount, 0)
    XCTAssertEqual(takes[0].text, "")
    XCTAssertEqual(takes[0].destination, "Agent · channel 1")
    // No ledger terminal and no measured speech: nothing to claim.
    XCTAssertNil(takes[0].ledgerStatus)
    XCTAssertNil(takes[0].anchoredSpeechPercent)
    XCTAssertFalse(takes[0].ledgerOpen)

    XCTAssertEqual(takes[1].id, "b1c63d87-dictation")
    XCTAssertEqual(takes[1].destination, "Dictation")
    XCTAssertEqual(takes[1].durationSeconds, 1, accuracy: 0.001)
    XCTAssertFalse(takes[1].ledgerOpen)
    XCTAssertEqual(takes[1].anchoredSpeechPercent ?? 0, 100, accuracy: 0.001)

    let agent = takes[2]
    XCTAssertEqual(agent.id, "agent-channel-3-aaaa")
    XCTAssertEqual(agent.text, "Wiesz, tak")
    XCTAssertEqual(agent.wordCount, 2)
    // Six refusal receipts on a two-word take say nothing about words: the
    // row reports how much speech the ledger anchored and that it stayed open.
    XCTAssertEqual(agent.ledgerStatus, "refused")
    XCTAssertTrue(agent.ledgerOpen)
    XCTAssertEqual(agent.anchoredSpeechPercent ?? 0, 29.7, accuracy: 0.001)
    XCTAssertEqual(agent.speechPercent ?? 0, 67.3, accuracy: 0.001)
    // No WAV beside the sidecar: the duration comes from the last sample at 48 kHz.
    XCTAssertEqual(agent.durationSeconds, 1_495_680.0 / 48_000.0, accuracy: 0.001)
    XCTAssertEqual(
      agent.recordedAt, RecentTakesReader.parseDate("2026-10-20T22:59:20.104496+00:00"))
  }

  func testWavHeaderDurationBeatsSampleFallbackAndZeroedHeaderIsIgnored() throws {
    let root = try makeRoot()
    defer { try? FileManager.default.removeItem(at: root) }
    try writeSidecar(
      root, session: "take-a", generatedAt: "2026-10-21T00:00:00+00:00",
      words: [("a", 0, 48_000)])
    try writeSidecar(
      root, session: "take-b", generatedAt: "2026-10-21T00:00:01+00:00",
      words: [("b", 0, 96_000)])
    // 16 kHz mono 16-bit, 32,000 bytes of data: one second, not two.
    try header(rate: 16_000, blockAlign: 2, dataBytes: 32_000)
      .write(to: root.appendingPathComponent("sessions/take-a.wav"))
    // A crash leaves the data length zeroed: fall back to the sidecar's samples.
    try header(rate: 48_000, blockAlign: 2, dataBytes: 0)
      .write(to: root.appendingPathComponent("sessions/take-b.wav"))

    let takes = try RecentTakesReader.read(root: root.path)
    let a = try XCTUnwrap(takes.first { $0.id == "take-a" })
    let b = try XCTUnwrap(takes.first { $0.id == "take-b" })
    XCTAssertEqual(a.durationSeconds, 1, accuracy: 0.001)
    XCTAssertEqual(b.durationSeconds, 2, accuracy: 0.001)
  }

  func testMissingSessionsDirectoryReadsAsNoTakes() throws {
    let root = FileManager.default.temporaryDirectory
      .appendingPathComponent("recent-takes-none-\(UUID().uuidString)", isDirectory: true)
    XCTAssertEqual(try RecentTakesReader.read(root: root.path), [])
  }

  func testShareLabelRoundsToWholePercent() {
    XCTAssertTrue(DictionaryRecentTakes.shareLabel(29.7).hasPrefix("30"))
    XCTAssertTrue(DictionaryRecentTakes.shareLabel(29.7).contains("%"))
  }

  func testDurationLabelUsesMinutesPastSixty() {
    XCTAssertEqual(DictionaryRecentTakes.durationLabel(38.4), "38 s")
    XCTAssertEqual(DictionaryRecentTakes.durationLabel(128.9), "2:09")
  }

  /// "Open in Lab" renders only while the Lab section can be selected. On a
  /// production bundle Lab is hidden and `select(.lab)` returns early, so the
  /// card must not offer a button that does nothing.
  @MainActor
  func testOpenInLabExistsOnlyWhileLabIsAvailable() throws {
    var opened = 0
    XCTAssertNil(DictionaryRecentTakes.openLabAction(availability: .hidden) { opened += 1 })
    let action = try XCTUnwrap(
      DictionaryRecentTakes.openLabAction(availability: .available) { opened += 1 })
    action()
    XCTAssertEqual(opened, 1)
    // The default reads the Lab section itself; the test bundle is a
    // production surface, where Lab is hidden.
    XCTAssertEqual(SettingsSection.lab.availability, .hidden)
    XCTAssertNil(DictionaryRecentTakes.openLabAction { opened += 1 })
    XCTAssertEqual(opened, 1)
  }

  /// The Dictionary pane wires the card through the availability gate, not a
  /// bare `select(.lab)` closure that would render the button everywhere.
  func testDictionaryPaneGatesOpenInLabOnLabAvailability() throws {
    let panel = try String(
      contentsOf: URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .appendingPathComponent("Codescribe/Screens/Settings/VoiceLabPanel.swift"),
      encoding: .utf8)
    XCTAssertTrue(
      panel.contains("openLab: DictionaryRecentTakes.openLabAction { model.select(.lab) }"))
    XCTAssertFalse(panel.contains("DictionaryRecentTakes(configDir: model.configDir) {"))
  }

  private func header(rate: UInt32, blockAlign: UInt16, dataBytes: UInt32) -> Data {
    var data = Data()
    func put(_ value: UInt32) { withUnsafeBytes(of: value.littleEndian) { data.append(contentsOf: $0) } }
    func put16(_ value: UInt16) { withUnsafeBytes(of: value.littleEndian) { data.append(contentsOf: $0) } }
    data.append(contentsOf: Array("RIFF".utf8))
    put(36 + dataBytes)
    data.append(contentsOf: Array("WAVE".utf8))
    data.append(contentsOf: Array("fmt ".utf8))
    put(16)
    put16(1)
    put16(1)
    put(rate)
    put(rate * UInt32(blockAlign))
    put16(blockAlign)
    put16(16)
    data.append(contentsOf: Array("data".utf8))
    put(dataBytes)
    return data
  }
}
