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
        editProvenance: nil,
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
      editProvenance: nil,
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
    let model = SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe())

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
      confidenceFlags: ["possible_hallucination_logprob"]
    )

    XCTAssertTrue(row.isLowConfidence)
    XCTAssertEqual(
      row.telemetryChips,
      ["logprob -1.40", "speech 72%", "possible_hallucination_logprob"]
    )
    XCTAssertTrue(row.hasTelemetry)
    XCTAssertTrue(row.hasTextChange)
  }

  /// (a) A take whose text never changed and that recorded no telemetry is
  /// not a correction — the list and the counter skip it (Founder 2026-09-30).
  func testNoOpRecordIsNotACorrection() {
    let record = CsQualityRecord(
      id: "take-1",
      revision: 1,
      rawText: "to  jest   cały take",
      variant: "to jest cały take",
      editedText: "to jest cały take",
      action: "close-unreviewed",
      editProvenance: nil,
      timestampMs: 7,
      avgLogprob: nil,
      speechPct: nil,
      confidenceFlags: []
    )

    XCTAssertFalse(
      isQualityCorrection(
        rawText: record.rawText,
        deliveredText: record.variant,
        editedText: record.editedText,
        avgLogprob: record.avgLogprob,
        speechPct: record.speechPct,
        confidenceFlags: record.confidenceFlags
      ),
      "whitespace-only differences are not a change"
    )
    XCTAssertTrue(qualityCorrectionRows([record]).isEmpty)
  }

  /// (c) A take with confidence flags but no text delta still belongs on the
  /// list — the telemetry is the correction evidence.
  func testTelemetryOnlyRecordStaysOnTheList() {
    let record = CsQualityRecord(
      id: "take-2",
      revision: 1,
      rawText: "pełny take bez zmian",
      variant: "pełny take bez zmian",
      editedText: "pełny take bez zmian",
      action: "close",
      editProvenance: nil,
      timestampMs: 9,
      avgLogprob: nil,
      speechPct: nil,
      confidenceFlags: ["speech_gap"]
    )

    let rows = qualityCorrectionRows([record])
    XCTAssertEqual(rows.count, 1)
    XCTAssertFalse(rows[0].hasTextChange)
    XCTAssertTrue(rows[0].hasTelemetry)
    XCTAssertEqual(rows[0].telemetryChips, ["speech_gap"])
    XCTAssertTrue(voiceLabDiffSpans(raw: rows[0].rawText, edited: rows[0].editedText).isEmpty)
  }

  /// (b) A real edit yields a word-level diff with the changed span and
  /// bounded context — Polish diacritics compare as words, never bytes.
  func testCorrectionDiffShowsChangedSpansWithPolishWords() {
    let raw = "Zażółć gęślą jaźń potem nagrajemy luks tri mapa jeszcze raz dziś"
    let edited = "Zażółć gęślą jaźń potem nagrajemy Loctree mapę jeszcze raz dziś"

    let spans = voiceLabDiffSpans(raw: raw, edited: edited)

    XCTAssertEqual(spans.count, 1)
    XCTAssertEqual(spans[0].raw, "luks tri mapa")
    XCTAssertEqual(spans[0].edited, "Loctree mapę")
    XCTAssertEqual(spans[0].tier, .vocabulary)
    XCTAssertEqual(spans[0].contextBefore, "Zażółć gęślą jaźń potem nagrajemy")
    XCTAssertEqual(spans[0].contextAfter, "jeszcze raz dziś")
  }

  /// Casing-only and punctuation-only edits surface as minor tiers.
  func testCorrectionDiffClassifiesMinorTiers() {
    let casingSpans = voiceLabDiffSpans(raw: "jako", edited: "Jako")
    XCTAssertEqual(casingSpans.count, 1)
    XCTAssertEqual(casingSpans[0].tier, .casing)

    let punctuationSpans = voiceLabDiffSpans(raw: "ciebie", edited: "ciebie,")
    XCTAssertEqual(punctuationSpans.count, 1)
    XCTAssertEqual(punctuationSpans[0].tier, .punctuation)
  }

  /// (d) The card renders major tiers inline and collapses casing/punctuation
  /// spans under one dimmed "+N minor" line.
  func testCorrectionCardCollapsesMinorTiers() {
    let spans = voiceLabDiffSpans(
      raw: "wajprawter wykrył jako potem ciebie",
      edited: "Vibecrafted wykrył Jako potem ciebie,"
    )

    let major = majorDiffSpans(spans)
    let minor = minorDiffSpans(spans)

    XCTAssertEqual(major.count, 1)
    XCTAssertEqual(major[0].tier, .vocabulary)
    XCTAssertEqual(major[0].edited, "Vibecrafted")
    XCTAssertEqual(minor.count, 2)
    XCTAssertEqual(minor.map(\.tier), [.casing, .punctuation])
    XCTAssertEqual(minorAdjustmentsSummary(minor), "+2 minor (punctuation, casing)")
  }

  /// (e) The headline counts real corrections, vocabulary corrections,
  /// unchanged takes, and rules as separate numbers; missing telemetry is one
  /// aggregate list line.
  func testDictionaryHeadlineCountsCorrectionsUnchangedTakesAndRules() {
    XCTAssertEqual(
      dictionaryHeadline(
        corrections: 0, vocabularyCorrections: 0, unchangedTakes: 0, rulesLearned: 0),
      "0 corrections (0 vocabulary) · 0 unchanged takes · 0 rules in dictionary"
    )
    XCTAssertEqual(
      dictionaryHeadline(
        corrections: 4, vocabularyCorrections: 3, unchangedTakes: 46, rulesLearned: 7),
      "4 corrections (3 vocabulary) · 46 unchanged takes · 7 rules in dictionary"
    )

    let withTelemetry = VoiceLabCorrectionRow(
      id: "a",
      revision: 1,
      rawText: "raw",
      variant: "raw",
      editedText: "raw",
      action: "close",
      timestampMs: 1,
      avgLogprob: -0.4,
      speechPct: nil,
      confidenceFlags: []
    )
    let withoutTelemetry = VoiceLabCorrectionRow(
      id: "b",
      revision: 1,
      rawText: "uni agentka",
      variant: "uni agentka",
      editedText: "Junie",
      action: "copy",
      timestampMs: 2,
      avgLogprob: nil,
      speechPct: nil,
      confidenceFlags: []
    )
    XCTAssertEqual(
      missingTelemetryLine(rows: [withTelemetry, withoutTelemetry]),
      "No confidence telemetry in 1 of 2"
    )
    XCTAssertNil(missingTelemetryLine(rows: [withTelemetry]))
    XCTAssertNil(missingTelemetryLine(rows: []))
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
      editProvenance: nil,
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
      editProvenance: "manual_human",
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
    let model = SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe())
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
      editProvenance: "manual_human",
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
      editProvenance: nil,
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
    let model = SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe())
    model.refreshVoiceLab()

    XCTAssertFalse(model.finalizeVoiceLabCorrection(id: original.id, canonical: "Broken"))
    XCTAssertEqual(model.qualityRecords, [original])
    XCTAssertNotNil(model.voiceLabEditErrors[original.id])
    XCTAssertNotNil(model.lastError)
    XCTAssertTrue(model.voiceLabEditPending.isEmpty)
  }

  func testDictionaryHeadlineInflectsEachCountIndependently() {
    let cases: [(Int, Int, Int, Int, String)] = [
      (1, 1, 1, 1, "1 correction (1 vocabulary) · 1 unchanged take · 1 rule in dictionary"),
      (1, 2, 3, 4, "1 correction (2 vocabulary) · 3 unchanged takes · 4 rules in dictionary"),
      (2, 1, 3, 4, "2 corrections (1 vocabulary) · 3 unchanged takes · 4 rules in dictionary"),
      (2, 3, 1, 4, "2 corrections (3 vocabulary) · 1 unchanged take · 4 rules in dictionary"),
      (2, 3, 4, 1, "2 corrections (3 vocabulary) · 4 unchanged takes · 1 rule in dictionary"),
    ]
    for (corrections, vocabulary, takes, rules, expected) in cases {
      XCTAssertEqual(
        dictionaryHeadline(
          corrections: corrections, vocabularyCorrections: vocabulary,
          unchangedTakes: takes, rulesLearned: rules), expected)
    }
  }

  func testDictionarySubtitleInflectsEachCountIndependently() {
    let cases: [(Int, Int, Int, String)] = [
      (1, 1, 1, "1 live rule (variant→canonical) · 1 with correction provenance · 1 store row."),
      (1, 2, 3, "1 live rule (variant→canonical) · 2 with correction provenance · 3 store rows."),
      (2, 1, 3, "2 live rules (variant→canonical) · 1 with correction provenance · 3 store rows."),
      (2, 3, 1, "2 live rules (variant→canonical) · 3 with correction provenance · 1 store row."),
      (2, 0, 0, "2 live rules (variant→canonical) · 0 with correction provenance · 0 store rows."),
    ]
    for (rules, corrections, rows, expected) in cases {
      XCTAssertEqual(
        dictionarySubtitle(
          correctionsRecorded: 10, rulesLearned: rules,
          taughtFromCorrections: corrections, totalEntries: rows), expected)
    }
  }

  func testDictionaryHeadlineHonestyForCorrectionSource() {
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
      "10 corrections on disk · dictionary empty — Teach explicitly promotes eligible store pairs now."
    )
    XCTAssertFalse(
      dictionaryHeadline(
        corrections: 1, vocabularyCorrections: 0, unchangedTakes: 0, rulesLearned: 0
      )
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
    let model = SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe())
    model.teachDictionaryFromStore()
    // Teach hops global -> main queues; pump the main run loop until the
    // completion lands (synchronous unwrap can never observe it).
    let deadline = Date().addingTimeInterval(2)
    while model.voiceLabTeachMessage == nil, Date() < deadline {
      RunLoop.main.run(until: Date().addingTimeInterval(0.01))
    }
    let msg = try XCTUnwrap(model.voiceLabTeachMessage)
    XCTAssertTrue(msg.contains("1 live rule "), "expected live-rules count, got: \(msg)")
    XCTAssertTrue(msg.hasPrefix("Taught"), "expected Taught status, got: \(msg)")
  }

  func testVoiceLabRefreshLoadsRuleCandidates() {
    let candidate = CsRuleCandidate(
      target: "Vibecrafted",
      variants: ["wajprawter", "WipeRapted"],
      occurrences: 3
    )
    let engine = MockSettingsEngine(
      qualityRecords: [],
      lexiconEntries: [],
      ruleCandidates: [candidate]
    )
    let model = SettingsViewModel(engine: engine)

    model.refreshVoiceLab()

    XCTAssertEqual(model.ruleCandidates, [candidate])
  }

  /// (f) With zero candidates the Suggested rules section disappears entirely.
  func testSuggestedRulesSectionHiddenAtZeroCandidates() {
    XCTAssertFalse(ruleCandidatesSectionVisible([]))

    let candidate = CsRuleCandidate(
      target: "Vibecrafted",
      variants: ["wajprawter"],
      occurrences: 2
    )
    XCTAssertTrue(ruleCandidatesSectionVisible([candidate]))

    let engine = MockSettingsEngine(qualityRecords: [], lexiconEntries: [])
    let model = SettingsViewModel(engine: engine)
    model.refreshVoiceLab()
    XCTAssertTrue(model.ruleCandidates.isEmpty)
    XCTAssertFalse(ruleCandidatesSectionVisible(model.ruleCandidates))
  }

  func testTeachRuleCandidateCallsEngineAndRefreshes() throws {
    let candidate = CsRuleCandidate(
      target: "Vibecrafted",
      variants: ["wajprawter"],
      occurrences: 3
    )
    var taught: [(String, String, String)] = []
    let engine = MockSettingsEngine(
      qualityRecords: [],
      lexiconEntries: [],
      ruleCandidates: [candidate],
      teachSpanObserver: { variant, canonical, kind in
        taught.append((variant, canonical, kind))
        return CsQualityCommitResult(
          pairsLearned: 1,
          evidenceOnly: false,
          acknowledgement: "Saved — 1 rule learned",
          teachSeen: nil,
          teachRequired: nil
        )
      }
    )
    let model = SettingsViewModel(engine: engine)
    model.refreshVoiceLab()

    model.teachRuleCandidate(target: candidate.target, variant: candidate.variants[0])

    let deadline = Date().addingTimeInterval(2)
    while model.voiceLabTeachMessage == nil, Date() < deadline {
      RunLoop.main.run(until: Date().addingTimeInterval(0.01))
    }
    XCTAssertEqual(taught.count, 1)
    XCTAssertEqual(taught[0].0, candidate.variants[0])
    XCTAssertEqual(taught[0].1, candidate.target)
    XCTAssertEqual(taught[0].2, "vocabulary")
    let msg = try XCTUnwrap(model.voiceLabTeachMessage)
    XCTAssertEqual(msg, "Saved — 1 rule learned")
  }
}
