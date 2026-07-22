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
            ),
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
                    timestampMs: 42
                ),
            ]
        )

        let lexicon = customLexiconRows([
            CsLexiconEntry(variant: "luks tri", canonical: "Loctree", source: "correction"),
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
            timestampMs: 42
        )
        var editor = VoiceLabEditorState()

        editor.begin(row)
        XCTAssertEqual(editor.correctionID, "correction-1")
        XCTAssertEqual(editor.canonical, "Junie")

        editor.cancel()
        XCTAssertNil(editor.correctionID)
        XCTAssertEqual(editor.canonical, "")
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
                lexicon = [CsLexiconEntry(variant: "uni agentka", canonical: canonical, source: "correction")]
                return revised
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
            dictionaryHeadline(correctionSourcedCount: 0),
            "Corrections recorded — teaching starts from your next short fix."
        )
        XCTAssertEqual(
            dictionaryHeadline(correctionSourcedCount: 2),
            "See what your voice taught."
        )
        XCTAssertTrue(
            dictionarySubtitle(correctionSourcedCount: 2, totalEntries: 5)
                .contains("2 learned-from-voice")
        )
        XCTAssertEqual(
            dictionarySubtitle(correctionSourcedCount: 0, totalEntries: 0),
            "Corrections and custom words come from the live local quality loop."
        )
    }
}
