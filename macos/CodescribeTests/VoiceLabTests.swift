import XCTest

@testable import Codescribe

// Preview timing tests moved to SettingsTruthTests with the Dictation IA cut;
// this file owns only the textual corrections + custom dictionary surface.
@MainActor
final class VoiceLabTests: XCTestCase {
  func testVoiceLabMappingsPreserveLiveBridgeDataAndEmptyState() {
    XCTAssertTrue(qualityCorrectionRows([]).isEmpty)
    XCTAssertTrue(customLexiconRows([]).isEmpty)

    let corrections = qualityCorrectionRows([
      CsQualityRecord(
        id: "correction-42",
        revision: 1,
        rawText: "uni agentka",
        variant: "uni agentka",
        editedText: "Junie",
        action: "copy",
        timestampMs: 42,
        avgLogprob: nil,
        speechPct: nil,
        confidenceFlags: []
      )
    ])
    XCTAssertEqual(
      corrections,
      [
        VoiceLabCorrectionRow(
          id: "correction-42",
          revision: 1,
          rawText: "uni agentka",
          variant: "uni agentka",
          editedText: "Junie",
          action: "copy",
          timestampMs: 42,
          avgLogprob: nil,
          speechPct: nil,
          confidenceFlags: []
        )
      ]
    )

    let lexicon = customLexiconRows([
      CsLexiconEntry(variant: "luks tri", canonical: "Loctree", source: "correction")
    ])
    XCTAssertEqual(
      lexicon,
      [VoiceLabLexiconRow(id: 0, variant: "luks tri", canonical: "Loctree", source: "correction")]
    )
  }

  func testVoiceLabRefreshPullsFreshBridgeSnapshots() {
    let record = CsQualityRecord(
      id: "correction-84",
      revision: 1,
      rawText: "before",
      variant: "before",
      editedText: "after",
      action: "send",
      timestampMs: 84,
      avgLogprob: nil,
      speechPct: nil,
      confidenceFlags: []
    )
    let entry = CsLexiconEntry(variant: "before", canonical: "after", source: "correction")
    let engine = MockSettingsEngine(
      qualityRecords: [record],
      lexiconEntries: [entry]
    )
    let model = SettingsViewModel(engine: engine)

    model.refreshVoiceLab()

    XCTAssertEqual(model.qualityRecords, [record])
    XCTAssertEqual(model.customLexiconEntries, [entry])
    XCTAssertNil(model.voiceLabReadError)
  }

  func testVoiceLabEditorBeginAndCancelAreDeterministic() {
    let row = VoiceLabCorrectionRow(
      id: "correction-1",
      revision: 2,
      rawText: "raw",
      variant: "uni agentka",
      editedText: "Junie",
      action: "copy",
      timestampMs: 42,
      avgLogprob: -1.4,
      speechPct: 0.72,
      confidenceFlags: ["low_logprob"]
    )
    var editor = VoiceLabEditorState()

    editor.begin(row)
    XCTAssertEqual(editor.correctionID, "correction-1")
    XCTAssertEqual(editor.canonical, "Junie")

    editor.cancel()
    XCTAssertNil(editor.correctionID)
    XCTAssertEqual(editor.canonical, "")
  }

  func testCorrectionConfidenceUsesWhisperAndSileroTruth() {
    let row = VoiceLabCorrectionRow(
      id: "correction-low",
      revision: 1,
      rawText: "raw whisper output",
      variant: "formatted output",
      editedText: "corrected output",
      action: "edit",
      timestampMs: 42,
      avgLogprob: -1.4,
      speechPct: 0.72,
      confidenceFlags: ["silero_dropped_tail_hallucinations"]
    )

    XCTAssertTrue(row.isLowConfidence)
    XCTAssertTrue(row.confidenceSummary.contains("Whisper logprob -1.40"))
    XCTAssertTrue(row.confidenceSummary.contains("Silero/VAD speech 72%"))
    XCTAssertTrue(row.confidenceSummary.contains("silero_dropped_tail_hallucinations"))
  }

  func testArchivedAudioLookupRequiresExactRawTranscript() throws {
    let root = FileManager.default.temporaryDirectory
      .appendingPathComponent(UUID().uuidString, isDirectory: true)
    let day = root.appendingPathComponent("transcriptions/2026-08-04", isDirectory: true)
    try FileManager.default.createDirectory(at: day, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: root) }

    let transcript = day.appendingPathComponent("083000_real-words_raw.txt")
    let audio = day.appendingPathComponent("083000_real-words_raw.m4a")
    try "raw whisper words".write(to: transcript, atomically: true, encoding: .utf8)
    try Data([0, 1, 2]).write(to: audio)

    XCTAssertEqual(
      archivedAudioURL(configDir: root.path, rawText: "raw whisper words")?
        .resolvingSymlinksInPath().path,
      audio.resolvingSymlinksInPath().path
    )
    XCTAssertNil(archivedAudioURL(configDir: root.path, rawText: "formatted words"))
  }

  func testArchivedAudioLookupFindsTakeWhenRawTextFileIsNumbered() throws {
    let root = FileManager.default.temporaryDirectory
      .appendingPathComponent(UUID().uuidString, isDirectory: true)
    let day = root.appendingPathComponent("transcriptions/2026-08-18", isDirectory: true)
    try FileManager.default.createDirectory(at: day, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: root) }

    let transcript = day.appendingPathComponent("105438_take_raw_1.txt")
    let audio = day.appendingPathComponent("105438_take_raw.m4a")
    try "numbered raw".write(to: transcript, atomically: true, encoding: .utf8)
    try Data([0, 1, 2]).write(to: audio)

    XCTAssertEqual(
      archivedAudioURL(configDir: root.path, rawText: "numbered raw")?
        .resolvingSymlinksInPath().path,
      audio.resolvingSymlinksInPath().path
    )
  }

  func testSuccessfulVoiceLabEditRefreshesResolvedProjection() {
    let original = CsQualityRecord(
      id: "correction-1",
      revision: 1,
      rawText: "uni agentka",
      variant: "uni agentka",
      editedText: "Junie",
      action: "copy",
      timestampMs: 42,
      avgLogprob: nil,
      speechPct: nil,
      confidenceFlags: []
    )
    let revised = CsQualityRecord(
      id: "correction-1",
      revision: 2,
      rawText: "uni agentka",
      variant: "uni agentka",
      editedText: "Junie Prime",
      action: "edit",
      timestampMs: 84,
      avgLogprob: nil,
      speechPct: nil,
      confidenceFlags: []
    )
    var records = [original]
    var lexicon = [CsLexiconEntry(variant: "uni agentka", canonical: "Junie", source: "correction")]
    var calls: [(String, String)] = []
    let engine = MockSettingsEngine(
      qualityRecordsLoader: { records },
      lexiconEntriesLoader: { lexicon },
      voiceLabEditObserver: { id, canonical in
        calls.append((id, canonical))
        records = [revised]
        lexicon = [
          CsLexiconEntry(variant: "uni agentka", canonical: canonical, source: "correction")
        ]
        return CsVoiceLabSaveResult(record: revised, pairsLearned: 1, lexiconError: nil)
      }
    )
    let model = SettingsViewModel(engine: engine)
    model.refreshVoiceLab()

    XCTAssertTrue(model.finalizeVoiceLabCorrection(id: original.id, canonical: " Junie Prime "))
    XCTAssertEqual(calls.map { "\($0.0):\($0.1)" }, ["correction-1:Junie Prime"])
    XCTAssertEqual(model.qualityRecords, [revised])
    XCTAssertEqual(model.customLexiconEntries, lexicon)
    XCTAssertTrue(model.voiceLabEditPending.isEmpty)
    XCTAssertNil(model.voiceLabEditErrors[original.id])
    XCTAssertEqual(model.voiceLabEditNotes[original.id], "Saved — 1 rule learned")
  }

  func testVoiceLabSaveNoteTellsTheLearningTruth() {
    let record = CsQualityRecord(
      id: "correction-1",
      revision: 2,
      rawText: "raw",
      variant: "variant",
      editedText: "edited",
      action: "edit",
      timestampMs: 1,
      avgLogprob: nil,
      speechPct: nil,
      confidenceFlags: []
    )
    XCTAssertEqual(
      SettingsViewModel.voiceLabSaveNote(
        CsVoiceLabSaveResult(record: record, pairsLearned: 0, lexiconError: nil)
      ),
      "Saved; no dictionary rule derived"
    )
    XCTAssertEqual(
      SettingsViewModel.voiceLabSaveNote(
        CsVoiceLabSaveResult(record: record, pairsLearned: 3, lexiconError: nil)
      ),
      "Saved — 3 rules learned"
    )
    XCTAssertEqual(
      SettingsViewModel.voiceLabSaveNote(
        CsVoiceLabSaveResult(record: record, pairsLearned: 0, lexiconError: "disk broke")
      ),
      "Saved — dictionary learning failed: disk broke"
    )
  }

  func testFailedVoiceLabEditKeepsOldCanonicalVisibleAndSurfacesError() {
    let original = CsQualityRecord(
      id: "correction-1",
      revision: 1,
      rawText: "uni agentka",
      variant: "uni agentka",
      editedText: "Junie",
      action: "copy",
      timestampMs: 42,
      avgLogprob: nil,
      speechPct: nil,
      confidenceFlags: []
    )
    let engine = MockSettingsEngine(
      qualityRecords: [original],
      voiceLabEditObserver: { _, _ in
        throw NSError(domain: "VoiceLabWrite", code: 1)
      }
    )
    let model = SettingsViewModel(engine: engine)
    model.refreshVoiceLab()

    XCTAssertFalse(model.finalizeVoiceLabCorrection(id: original.id, canonical: "Broken"))
    XCTAssertEqual(model.qualityRecords, [original])
    XCTAssertNotNil(model.voiceLabEditErrors[original.id])
    XCTAssertNotNil(model.lastError)
    XCTAssertTrue(model.voiceLabEditPending.isEmpty)
  }

  func testDictionaryHeadlineHonestyForCorrectionSource() {
    XCTAssertEqual(
      dictionaryHeadline(correctionsRecorded: 0, rulesLearned: 0),
      "0 corrections recorded · 0 rules in dictionary"
    )
    XCTAssertEqual(
      dictionaryHeadline(correctionsRecorded: 74, rulesLearned: 3),
      "74 corrections recorded · 3 rules in dictionary"
    )
    XCTAssertTrue(
      dictionarySubtitle(
        correctionsRecorded: 74,
        rulesLearned: 3,
        taughtFromCorrections: 3,
        totalEntries: 5
      )
      .contains("3 with correction provenance")
    )
    XCTAssertEqual(
      dictionarySubtitle(
        correctionsRecorded: 10,
        rulesLearned: 0,
        taughtFromCorrections: 0,
        totalEntries: 0
      ),
      "10 corrections on disk · dictionary empty — press Teach to mine rules from the store."
    )
    XCTAssertFalse(
      dictionaryHeadline(correctionsRecorded: 1, rulesLearned: 0)
        .contains("voice taught")
    )
  }

  func testTeachDictionarySurfacesHonestMessage() throws {
    // Product: Teach is a real Settings surface, not a decorative button.
    // Mock returns a zero-delta teach result; ViewModel still writes a
    // non-empty status line with live-rule counts.
    let engine = MockSettingsEngine(
      lexiconEntries: [
        CsLexiconEntry(variant: "luks tri", canonical: "Loctree", source: "correction")
      ]
    )
    let model = SettingsViewModel(engine: engine)
    model.teachDictionaryFromStore()
    // Teach hops global -> main queues; pump the main run loop until the
    // completion lands (synchronous unwrap can never observe it).
    let deadline = Date().addingTimeInterval(2)
    while model.voiceLabTeachMessage == nil, Date() < deadline {
      RunLoop.main.run(until: Date().addingTimeInterval(0.01))
    }
    let msg = try XCTUnwrap(model.voiceLabTeachMessage)
    XCTAssertTrue(msg.contains("live rules"), "expected live-rules count, got: \(msg)")
    XCTAssertTrue(msg.hasPrefix("Taught"), "expected Taught status, got: \(msg)")
  }
}
