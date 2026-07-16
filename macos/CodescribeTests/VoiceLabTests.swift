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
                rawText: "uni agentka",
                editedText: "Junie",
                action: "copy",
                timestampMs: 42
            ),
        ])
        XCTAssertEqual(
            corrections,
            [
                VoiceLabCorrectionRow(
                    id: 0,
                    rawText: "uni agentka",
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
            rawText: "before",
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
