import XCTest
@testable import Codescribe

@MainActor
final class VoiceLabTests: XCTestCase {
    func testSmoothPresetValuesMatchOperatorDefaultExactly() throws {
        let smooth = try XCTUnwrap(presetValues(.smooth))

        XCTAssertEqual(smooth.bufferDelayMs, 1038)
        XCTAssertEqual(smooth.typingCps, 10.6, accuracy: 0.0001)
        XCTAssertEqual(smooth.emitWordsMax, 5)
        XCTAssertEqual(smooth.interimSeconds, 8.0, accuracy: 0.0001)
    }

    func testDetectPresetRecognizesAllFiveStatesWithTolerance() throws {
        for preset in [PreviewTimingPreset.smooth, .snappy, .relaxed] {
            let values = try XCTUnwrap(presetValues(preset))
            XCTAssertEqual(
                detectPreset(PreviewTimingConfiguration(overlayEnabled: true, values: values)),
                preset
            )
        }

        XCTAssertEqual(
            detectPreset(
                PreviewTimingConfiguration(overlayEnabled: false, values: PreviewTimingValues.smooth)
            ),
            .off
        )

        let withinTolerance = PreviewTimingValues(
            bufferDelayMs: 1048,
            typingCps: 10.74,
            emitWordsMax: 5,
            interimSeconds: 8.14
        )
        XCTAssertEqual(
            detectPreset(
                PreviewTimingConfiguration(overlayEnabled: true, values: withinTolerance)
            ),
            .smooth
        )

        let custom = PreviewTimingValues(
            bufferDelayMs: 1100,
            typingCps: 10.6,
            emitWordsMax: 5,
            interimSeconds: 8.0
        )
        XCTAssertEqual(
            detectPreset(PreviewTimingConfiguration(overlayEnabled: true, values: custom)),
            .custom
        )
    }

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
                timestampMs: 42
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
            CsLexiconEntry(variant: "luks tri", canonical: "Loctree"),
        ])
        XCTAssertEqual(
            lexicon,
            [VoiceLabLexiconRow(id: 0, variant: "luks tri", canonical: "Loctree")]
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
            timestampMs: 84
        )
        let entry = CsLexiconEntry(variant: "before", canonical: "after")
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
            timestampMs: 42
        )
        let revised = CsQualityRecord(
            id: "correction-1",
            revision: 2,
            rawText: "uni agentka",
            variant: "uni agentka",
            editedText: "Junie Prime",
            action: "edit",
            timestampMs: 84
        )
        var records = [original]
        var lexicon = [CsLexiconEntry(variant: "uni agentka", canonical: "Junie")]
        var calls: [(String, String)] = []
        let engine = MockSettingsEngine(
            qualityRecordsLoader: { records },
            lexiconEntriesLoader: { lexicon },
            voiceLabEditObserver: { id, canonical in
                calls.append((id, canonical))
                records = [revised]
                lexicon = [CsLexiconEntry(variant: "uni agentka", canonical: canonical)]
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
            timestampMs: 42
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

    func testSmoothPresetUsesOneAtomicSettingsBatch() {
        var batches: [[CsConfigEntry]] = []
        let engine = MockSettingsEngine(updateConfigManyObserver: { entries in
            batches.append(entries)
        })
        let model = SettingsViewModel(engine: engine)

        model.applyPreviewTimingPreset(.smooth)

        XCTAssertEqual(batches.count, 1)
        let values = Dictionary(uniqueKeysWithValues: batches[0].map { ($0.key, $0.value) })
        XCTAssertEqual(values["TRANSCRIPTION_OVERLAY_ENABLED"], "1")
        XCTAssertEqual(values["CODESCRIBE_BUFFER_DELAY_MS"], "1038")
        XCTAssertEqual(values["CODESCRIBE_TYPING_CPS"], "10.6")
        XCTAssertEqual(values["CODESCRIBE_EMIT_WORDS_MAX"], "5")
        XCTAssertEqual(values["CODESCRIBE_BUFFERED_INTERIM_SEC"], "8.0")
    }
}
