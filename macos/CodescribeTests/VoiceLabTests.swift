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
  func testDictionaryCountersAreThreeSeparateInflectedValues() {
    XCTAssertEqual(
      dictionaryCounters(corrections: 0, unchangedTakes: 0, activeRules: 0),
      ["0 corrections", "0 unchanged takes", "0 active rules"])
    XCTAssertEqual(
      dictionaryCounters(corrections: 1, unchangedTakes: 1, activeRules: 1),
      ["1 correction", "1 unchanged take", "1 active rule"])
    XCTAssertEqual(
      dictionaryCounters(corrections: 4, unchangedTakes: 46, activeRules: 7),
      ["4 corrections", "46 unchanged takes", "7 active rules"])
  }

  func testRuleProvenanceLineNamesOnlyTheSourcesThatExist() {
    XCTAssertNil(lexiconProvenanceLine(fromCorrections: 0, addedByHand: 0, other: 0))
    XCTAssertEqual(
      lexiconProvenanceLine(fromCorrections: 3, addedByHand: 0, other: 0), "3 from corrections")
    XCTAssertEqual(
      lexiconProvenanceLine(fromCorrections: 1, addedByHand: 1, other: 0),
      "1 from correction · 1 added by hand")
    XCTAssertEqual(
      lexiconProvenanceLine(fromCorrections: 0, addedByHand: 2, other: 5),
      "2 added by hand · 5 from earlier versions")
  }

  func testTelemetryCoverageLineCountsRecordsWithoutTelemetry() {
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

  func testArchivedAudioLookupRefusesToGuessBetweenIdenticalTakes() throws {
    let root = FileManager.default.temporaryDirectory
      .appendingPathComponent(UUID().uuidString, isDirectory: true)
    let day = root.appendingPathComponent("transcriptions/2026-10-08", isDirectory: true)
    try FileManager.default.createDirectory(at: day, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: root) }

    for stem in ["090000_first_raw", "110000_second_raw"] {
      try "test test".write(
        to: day.appendingPathComponent("\(stem).txt"), atomically: true, encoding: .utf8)
      try Data([0, 1, 2]).write(to: day.appendingPathComponent("\(stem).m4a"))
    }
    // A numbered text twin of the same take is one take, not two.
    try "test test".write(
      to: day.appendingPathComponent("090000_first_raw_1.txt"), atomically: true, encoding: .utf8)

    XCTAssertEqual(archivedAudioLookup(configDir: root.path, rawText: "test test"), .ambiguous(2))
    XCTAssertNil(archivedAudioURL(configDir: root.path, rawText: "test test"))
    XCTAssertEqual(archivedAudioLookup(configDir: root.path, rawText: "nothing"), .missing)
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

  func testStageDiffsSeparateFormattingFromTheManualCorrection() {
    // Smart/Max rewrote the raw STT; the human then fixed one word. Neither
    // stage is charged to the other.
    let stages = correctionStageDiffs(
      raw: "no to jedziemy z koksem",
      delivered: "No to jedziemy z koksem.",
      edited: "No to jedziemy z Codescribe.")
    XCTAssertEqual(stages.map(\.stage), [.formatting, .manual])
    XCTAssertTrue(stages[0].spans.allSatisfy { !$0.tier.isMajor }, "formatting only recased")
    XCTAssertEqual(stages[1].spans.map(\.raw), ["koksem."])
    XCTAssertEqual(stages[1].spans.map(\.edited), ["Codescribe."])

    // Formatting off: delivered equals raw, so the only stage is the correction.
    XCTAssertEqual(
      correctionStageDiffs(raw: "uni agentka", delivered: "uni agentka", edited: "Junie")
        .map(\.stage), [.manual])
    // A pure formatter rewrite without a manual edit is not a recognition error.
    XCTAssertEqual(
      correctionStageDiffs(raw: "raw words", delivered: "Raw words.", edited: "Raw words.")
        .map(\.stage), [.formatting])
    // Whitespace is not a stage.
    XCTAssertTrue(correctionStageDiffs(raw: "a  b", delivered: "a b", edited: "a b").isEmpty)
  }

  func testDiffSpanKindNamesAdditionRemovalAndReplacement() {
    let spans = voiceLabDiffSpans(raw: "one two three four", edited: "one 2 three four five")
    let kinds = spans.map(diffSpanKind)
    XCTAssertTrue(kinds.contains(.replaced), "\(spans)")
    XCTAssertTrue(kinds.contains(.added), "\(spans)")
    XCTAssertEqual(
      voiceLabDiffSpans(raw: "keep this word", edited: "keep word").map(diffSpanKind), [.removed])
    XCTAssertEqual(DiffSpanKind.added.label, "Added")
    XCTAssertEqual(DiffSpanKind.removed.label, "Removed")
    XCTAssertEqual(DiffSpanKind.replaced.label, "Replaced")
  }

  func testCorrectionLabelsReadAsVersionsAndCharacters() {
    XCTAssertEqual(
      fullComparisonLabel(rawCount: 120, editedCount: 118), "Full comparison · 120 → 118 characters")
    XCTAssertEqual(
      correctionFooter(action: "revision", revision: 3, timestamp: "8 Oct 2026, 13:50"),
      "Version 3 · 8 Oct 2026, 13:50")
    XCTAssertEqual(
      correctionFooter(action: "copy", revision: 1, timestamp: "8 Oct 2026, 13:50"),
      "copied · Version 1 · 8 Oct 2026, 13:50")
    XCTAssertEqual(LexiconSourceLabel.text(for: "correction"), "from a correction")
    XCTAssertEqual(LexiconSourceLabel.text(for: "manual"), "added by hand")
  }

  func testLearnMessagesNameTheScopeAndTheRealGrowth() {
    XCTAssertEqual(
      learnScopeMessage(corrections: 12),
      "Codescribe reviews all 12 saved corrections and the suggested rules, then adds the new vocabulary rules it can derive to My rules. Existing rules, corrections and their history stay as they are."
    )
    XCTAssertEqual(
      learnResultMessage(added: 2, fromSuggestions: 0, activeRules: 9),
      "Added 2 rules from corrections · 9 active rules")
    XCTAssertEqual(
      learnResultMessage(added: 1, fromSuggestions: 1, activeRules: 1),
      "Added 1 rule from corrections and suggestions · 1 active rule")
    XCTAssertEqual(
      learnResultMessage(added: 0, fromSuggestions: 3, activeRules: 9),
      "No new rules: everything eligible is already in My rules · 9 active rules")
  }

  func testRetranscribeReasonExplainsEveryDisabledState() {
    let url = URL(fileURLWithPath: "/tmp/take.m4a")
    XCTAssertNil(
      retranscribeUnavailableReason(asrMode: "local_power", lookup: .found(url), pending: false))
    XCTAssertEqual(
      retranscribeUnavailableReason(asrMode: "local_power", lookup: .found(url), pending: true),
      "Retranscribing…")
    XCTAssertEqual(
      retranscribeUnavailableReason(asrMode: "apple_only", lookup: .found(url), pending: false),
      "Retranscribe needs the Local power or Cloud mode.")
    XCTAssertEqual(
      retranscribeUnavailableReason(asrMode: "cloud", lookup: .missing, pending: false),
      "No archived recording for this correction.")
    XCTAssertEqual(
      retranscribeUnavailableReason(asrMode: "cloud", lookup: .ambiguous(2), pending: false),
      "2 archived recordings share this exact transcript, so Codescribe cannot tell which one is this take."
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
    // The mock reports the same rule before and after: nothing new was learned.
    XCTAssertEqual(msg, "No new rules: everything eligible is already in My rules · 1 active rule")
  }

  /// With more corrections than the page loads, Learn and the counters must
  /// still quote the whole store (review, 2026-10-09).
  func testVoiceLabRefreshKeepsTheCorpusSizeBeyondThePageCap() {
    let records = (0..<60).map { index in
      CsQualityRecord(
        id: "corr-\(index)",
        revision: 1,
        rawText: "raw \(index)",
        variant: "raw \(index)",
        editedText: "edited \(index)",
        action: "copy",
        editProvenance: nil,
        timestampMs: UInt64(1_700_000_000_000 + index),
        avgLogprob: nil,
        speechPct: nil,
        confidenceFlags: []
      )
    }
    let engine = MockSettingsEngine(qualityRecords: records, lexiconEntries: [])
    let model = SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe())

    model.refreshVoiceLab()

    XCTAssertEqual(model.qualityRecords.count, 50, "the page stays capped")
    XCTAssertEqual(model.totalQualityCorrections, 60, "the corpus count is not")
    XCTAssertEqual(
      learnScopeMessage(corrections: Int(clamping: model.totalQualityCorrections)),
      "Codescribe reviews all 60 saved corrections and the suggested rules, then adds the new vocabulary rules it can derive to My rules. Existing rules, corrections and their history stay as they are."
    )
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
    let model = SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe())

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
    let model = SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe())
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
    let model = SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe())
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
