import AppKit
import SwiftUI
import XCTest
@testable import Codescribe

@MainActor
final class SettingsTruthTests: XCTestCase {
    func testRailKeyboardFocusMapsToDSHairlineWithoutChangingActiveFill() {
        XCTAssertEqual(
            settingsRailItemVisualState(isActive: true, isKeyboardFocused: false),
            SettingsRailItemVisualState(showsActiveFill: true, showsHairline: true)
        )
        XCTAssertEqual(
            settingsRailItemVisualState(isActive: true, isKeyboardFocused: true),
            SettingsRailItemVisualState(showsActiveFill: true, showsHairline: true)
        )
        XCTAssertEqual(
            settingsRailItemVisualState(isActive: false, isKeyboardFocused: true),
            SettingsRailItemVisualState(showsActiveFill: false, showsHairline: true)
        )
        XCTAssertEqual(
            settingsRailItemVisualState(isActive: false, isKeyboardFocused: false),
            SettingsRailItemVisualState(showsActiveFill: false, showsHairline: false)
        )
    }

    func testSectionAvailabilityKeepsPromisesHonest() {
        for section in [
            SettingsSection.creator, .shortcuts, .keys, .prompts, .engine, .audio, .voiceLab, .user,
        ] {
            XCTAssertEqual(section.availability, .available)
            XCTAssertTrue(section.isInteractive)
        }
    }

    /// The full route map: stable id, one visible title owner, and the explicit
    /// panel destination SettingsView's detail switch consumes. All eight rail
    /// sections, including the consolidation cuts engine→Dictation,
    /// voiceLab→Dictionary, keys→Providers.
    func testSettingsSectionRoutesTitlesAndDestinationsOwnTheRail() {
        let expectations: [(SettingsSection, String, String, SettingsPanelDestination)] = [
            (.creator, "creator", "Creator", .creator),
            (.shortcuts, "shortcuts", "Hotkeys", .shortcuts),
            (.keys, "keys", "Providers", .providers),
            (.prompts, "prompts", "Prompts", .prompts),
            (.engine, "engine", "Dictation", .dictation),
            (.audio, "audio", "Audio", .audio),
            (.voiceLab, "voiceLab", "Dictionary", .dictionary),
            (.user, "user", "User", .user),
        ]

        XCTAssertEqual(SettingsSection.allCases.count, expectations.count)
        for (section, id, title, destination) in expectations {
            XCTAssertEqual(section.rawValue, id)
            XCTAssertEqual(section.id, id)
            XCTAssertEqual(section.title, title)
            XCTAssertEqual(section.destination, destination)
        }
        // No two sections may share a destination or a visible title.
        XCTAssertEqual(
            Set(SettingsSection.allCases.map(\.destination)).count,
            SettingsSection.allCases.count
        )
        XCTAssertEqual(
            Set(SettingsSection.allCases.map(\.title)).count,
            SettingsSection.allCases.count
        )
    }

    /// Dictation owns every transcription-behavior write and each control keeps
    /// its exact promoted key/value contract after the IA move.
    func testDictationControlsWriteExactPromotedKeysAndValues() {
        var writes: [(key: String, value: String)] = []
        let model = SettingsViewModel(engine: MockSettingsEngine { key, value in
            writes.append((key, value))
        })

        model.setSttEngine("whisper")
        model.setLayeredTranscription(true)
        model.setLayeredTranscription(false)
        model.setToggleSilenceSeconds(3.5)
        model.setPreviewBufferDelayMs(1038)
        model.setPreviewTypingCps(10.6)
        model.setPreviewEmitWordsMax(5)
        model.setPreviewInterimSeconds(8.0)

        XCTAssertEqual(writes.map(\.key), [
            "CODESCRIBE_STT_ENGINE",
            "CODESCRIBE_LAYERED_TRANSCRIPTION",
            "CODESCRIBE_LAYERED_TRANSCRIPTION",
            "TOGGLE_SILENCE_SEC",
            "CODESCRIBE_BUFFER_DELAY_MS",
            "CODESCRIBE_TYPING_CPS",
            "CODESCRIBE_EMIT_WORDS_MAX",
            "CODESCRIBE_BUFFERED_INTERIM_SEC",
        ])
        XCTAssertEqual(writes.map(\.value), [
            "whisper", "phase1", "off", "3.5", "1038", "10.6", "5", "8.0",
        ])
    }

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

        model.applyPreviewTimingPreset(.off)
        XCTAssertEqual(batches.count, 2)
        XCTAssertEqual(batches[1].map(\.key), ["TRANSCRIPTION_OVERLAY_ENABLED"])
        XCTAssertEqual(batches[1].map(\.value), ["0"])
    }

    /// Providers owns the one lane-edit grammar. Every lane preserves its exact
    /// endpoint/model keys, and whitespace/empty input keeps the reset semantics
    /// (an empty write clears the JSON override).
    func testProviderLaneEditorsPreserveExactKeysAndEmptyResetSemantics() {
        var writes: [(key: String, value: String)] = []
        let model = SettingsViewModel(engine: MockSettingsEngine { key, value in
            writes.append((key, value))
        })

        let lanes: [(lane: LLMLane, endpointKey: String, modelKey: String)] = [
            (.assistive, "LLM_ASSISTIVE_ENDPOINT", "LLM_ASSISTIVE_MODEL"),
            (.formatting, "LLM_FORMATTING_ENDPOINT", "LLM_FORMATTING_MODEL"),
            (.main, "LLM_ENDPOINT", "LLM_MODEL"),
        ]

        for expectation in lanes {
            XCTAssertEqual(expectation.lane.endpointKey, expectation.endpointKey)
            XCTAssertEqual(expectation.lane.modelKey, expectation.modelKey)

            writes.removeAll()
            model.setLLMEndpoint(" https://example.test/v1 ", for: expectation.lane)
            model.setLLMModel("model-x", for: expectation.lane)
            model.setLLMEndpoint("   ", for: expectation.lane)
            model.setLLMModel("", for: expectation.lane)

            XCTAssertEqual(writes.map(\.key), [
                expectation.endpointKey,
                expectation.modelKey,
                expectation.endpointKey,
                expectation.modelKey,
            ])
            XCTAssertEqual(
                writes.map(\.value),
                ["https://example.test/v1", "model-x", "", ""]
            )
        }
    }

    /// The four consolidated owners all render non-empty at the supported
    /// compact content width using the current DS components.
    func testFourOwnerPanelsRenderNonEmptyAtCompactWidth() throws {
        let size = CGSize(width: 620, height: 1_600)
        let panels: [(name: String, view: AnyView)] = [
            ("dictation", AnyView(EnginePanel(model: SettingsViewModel.preview(.engine)))),
            ("audio", AnyView(AudioPanel(model: SettingsViewModel.preview(.audio)))),
            ("dictionary", AnyView(VoiceLabPanel(model: SettingsViewModel.preview(.voiceLab)))),
            ("providers", AnyView(KeysPanel(model: SettingsViewModel.preview(.keys)))),
        ]

        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("codescribe-settings-captures", isDirectory: true)
        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true
        )

        for panel in panels {
            let hostingView = NSHostingView(rootView: panel.view.frame(
                width: size.width,
                height: size.height,
                alignment: .topLeading
            ))
            hostingView.frame = CGRect(origin: .zero, size: size)
            hostingView.layoutSubtreeIfNeeded()

            guard let bitmap = hostingView.bitmapImageRepForCachingDisplay(in: hostingView.bounds) else {
                return XCTFail("Could not allocate \(panel.name) panel bitmap")
            }
            hostingView.cacheDisplay(in: hostingView.bounds, to: bitmap)
            guard let png = bitmap.representation(using: .png, properties: [:]) else {
                return XCTFail("Could not encode \(panel.name) panel PNG")
            }
            XCTAssertGreaterThan(png.count, 20_000, "\(panel.name) panel rendered (near-)empty")

            try png.write(to: directory.appendingPathComponent("owner-\(panel.name)-compact.png"))
        }
    }

    func testHealthStateMatrix() {
        XCTAssertEqual(
            healthState(stt: true, keys: .available, agent: true),
            SettingsHealthState(level: .healthy, message: "systems ready", targetSection: nil)
        )
        XCTAssertEqual(
            healthState(stt: true, keys: .missing, agent: false),
            SettingsHealthState(
                level: .degraded,
                message: "assistive lane: no key",
                targetSection: .keys
            )
        )
        XCTAssertEqual(
            healthState(stt: false, keys: .available, agent: true),
            SettingsHealthState(
                level: .offline,
                message: "speech engine: unavailable",
                targetSection: .engine
            )
        )
        XCTAssertEqual(
            healthState(stt: true, keys: .available, agent: false),
            SettingsHealthState(
                level: .offline,
                message: "assistive lane: not ready",
                targetSection: .engine
            )
        )
        XCTAssertEqual(
            healthState(stt: nil, keys: .available, agent: true),
            SettingsHealthState(
                level: .unknown,
                message: "system health: unknown",
                targetSection: .engine
            )
        )
    }

    func testCreatorLanguagePresentationKeepsTruthfulIdentityAndAccessibility() {
        let choices = LanguageIdentityPresentation.choices

        XCTAssertEqual(choices.map(\.title), ["Multilingual", "Polish", "English"])
        XCTAssertEqual(choices.map(\.isFineTuned), [false, true, true])
        XCTAssertEqual(
            choices.map(\.accessibilityLabel),
            ["Multilingual", "Polish, Fine-tuned", "English, Fine-tuned"]
        )
        XCTAssertEqual(choices[1].accessibilityValue(isSelected: true), "Selected")
        XCTAssertEqual(choices[2].accessibilityValue(isSelected: false), "Not selected")
        // The dictionary name derives from the SettingsSection title owner, so a
        // rail rename (e.g. Dictionary → Teacher) flows through automatically.
        XCTAssertEqual(
            LanguageIdentityPresentation.supportingCopy,
            "Programming vocabulary and your \(SettingsSection.voiceLab.title) entries enrich the selected language."
        )
        XCTAssertEqual(
            LanguageIdentityPresentation.supportingCopy,
            "Programming vocabulary and your Dictionary entries enrich the selected language."
        )
        XCTAssertFalse(LanguageIdentityPresentation.supportingCopy.contains("model weights"))
    }

    func testCreatorLanguageSelectionWritesStableRuntimeCodes() {
        var writes: [(key: String, value: String)] = []
        let engine = MockSettingsEngine { key, value in
            writes.append((key, value))
        }
        let model = SettingsViewModel(engine: engine)

        model.setLanguage(.auto)
        model.setLanguage(.polish)
        model.setLanguage(.english)

        XCTAssertEqual(writes.map(\.key), [
            "WHISPER_LANGUAGE", "WHISPER_LANGUAGE", "WHISPER_LANGUAGE",
        ])
        XCTAssertEqual(writes.map(\.value), ["auto", "pl", "en"])
    }

    func testFormattingPolicyNamesAliasesAndWritesAreNormalized() {
        XCTAssertEqual(
            FormattingPolicyOption.allCases.map(\.visibleName),
            ["Off", "Correction", "Smart", "Max"]
        )
        XCTAssertEqual(FormattingPolicyOption(storedValue: "raw"), .off)
        XCTAssertEqual(FormattingPolicyOption(storedValue: "medium"), .correction)
        XCTAssertEqual(FormattingPolicyOption(storedValue: "creative"), .max)
        XCTAssertNil(FormattingPolicyOption(storedValue: "aggressive"))

        var writes: [(String, String)] = []
        let model = SettingsViewModel(engine: MockSettingsEngine { key, value in
            writes.append((key, value))
        })
        for value in ["raw", "medium", "smart", "creative"] {
            model.setFormattingLevel(value)
        }
        model.setFormattingLevel("aggressive")

        XCTAssertEqual(writes.map(\.0), Array(repeating: "FORMATTING_LEVEL", count: 4))
        XCTAssertEqual(writes.map(\.1), ["off", "correction", "smart", "max"])
        XCTAssertNotNil(model.lastError)
    }

    func testCreatorPanelRendersAtCompactAndLargeWidths() throws {
        for (name, width) in [("compact", 620.0), ("large", 900.0)] {
            let size = CGSize(width: width, height: 900)
            let model = SettingsViewModel(engine: MockSettingsEngine())
            let hostingView = NSHostingView(rootView: CreatorPanel(model: model).frame(
                width: size.width,
                height: size.height,
                alignment: .topLeading
            ))
            hostingView.frame = CGRect(origin: .zero, size: size)
            hostingView.layoutSubtreeIfNeeded()

            guard let bitmap = hostingView.bitmapImageRepForCachingDisplay(in: hostingView.bounds) else {
                return XCTFail("Could not allocate \(name) CreatorPanel bitmap")
            }
            hostingView.cacheDisplay(in: hostingView.bounds, to: bitmap)
            guard let png = bitmap.representation(using: .png, properties: [:]) else {
                return XCTFail("Could not encode \(name) CreatorPanel PNG")
            }
            XCTAssertGreaterThan(png.count, 20_000)

            let directory = FileManager.default.temporaryDirectory
                .appendingPathComponent("codescribe-settings-captures", isDirectory: true)
            try FileManager.default.createDirectory(
                at: directory,
                withIntermediateDirectories: true
            )
            try png.write(to: directory.appendingPathComponent("creator-language-\(name).png"))
        }
    }

    func testPromptPanelRendersAllFormattingOwners() throws {
        let size = CGSize(width: 900, height: 1_900)
        let model = SettingsViewModel(engine: MockSettingsEngine())
        let hostingView = NSHostingView(rootView: PromptPanel(model: model).frame(
            width: size.width,
            height: size.height,
            alignment: .topLeading
        ))
        hostingView.frame = CGRect(origin: .zero, size: size)
        hostingView.layoutSubtreeIfNeeded()

        guard let bitmap = hostingView.bitmapImageRepForCachingDisplay(in: hostingView.bounds) else {
            return XCTFail("Could not allocate PromptPanel bitmap")
        }
        hostingView.cacheDisplay(in: hostingView.bounds, to: bitmap)
        guard let png = bitmap.representation(using: .png, properties: [:]) else {
            return XCTFail("Could not encode PromptPanel PNG")
        }
        XCTAssertGreaterThan(png.count, 40_000)

        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("codescribe-settings-captures", isDirectory: true)
        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true
        )
        try png.write(to: directory.appendingPathComponent("prompt-owners.png"))
    }

    func testTaggingToggleWritesPromotedConfigKey() {
        var writes: [(key: String, value: String)] = []
        let engine = MockSettingsEngine { key, value in
            writes.append((key, value))
        }
        let model = SettingsViewModel(engine: engine)

        model.setTranscriptTaggingEnabled(true)
        model.setTranscriptTaggingEnabled(false)

        XCTAssertEqual(writes.map(\.key), [
            "TRANSCRIPT_TAGGING_ENABLED", "TRANSCRIPT_TAGGING_ENABLED",
        ])
        XCTAssertEqual(writes.map(\.value), ["1", "0"])
    }

    func testTranscriptTagTemplateWritesPromotedConfigKeyAndAllowsStaticAttributes() {
        var writes: [(key: String, value: String)] = []
        let engine = MockSettingsEngine { key, value in
            writes.append((key, value))
        }
        let model = SettingsViewModel(engine: engine)

        model.setTranscriptTagTemplate("<codescribe warn=\"may contain misspelling\">{text}</codescribe>")

        XCTAssertEqual(writes.map(\.key), ["TRANSCRIPT_TAG_TEMPLATE"])
        XCTAssertEqual(
            writes.map(\.value),
            ["<codescribe warn=\"may contain misspelling\">{text}</codescribe>"]
        )
    }

    func testTranscriptTagTemplatePreviewWarnsAndAppendsWhenTextPlaceholderMissing() {
        let model = SettingsViewModel()

        model.setTranscriptTagTemplate("<codescribe conf=\"{conf}\" flags=\"{flags}\">")

        XCTAssertEqual(
            model.transcriptTagPreview,
            "<codescribe conf=\"medium\" flags=\"possible_hallucination_logprob\">\n…"
        )
        XCTAssertEqual(
            model.transcriptTagTemplateWarning,
            "Missing {text}; delivered transcript will be appended after the template."
        )
    }

    func testRestoreTranscriptTagTemplateWritesDefault() {
        var writes: [(key: String, value: String)] = []
        let engine = MockSettingsEngine { key, value in
            writes.append((key, value))
        }
        let model = SettingsViewModel(engine: engine)

        model.restoreDefaultTranscriptTagTemplate()

        XCTAssertEqual(writes.map(\.key), ["TRANSCRIPT_TAG_TEMPLATE"])
        XCTAssertEqual(writes.map(\.value), [defaultTranscriptTagTemplate])
    }

    func testResetPreviewMapsLiveCountsIntoConcreteConfirmationCopy() {
        let preview = CsResetPreview(
            audioFiles: 5_000,
            transcriptDays: 42,
            threads: 17,
            totalBytes: 536_870_912
        )
        let model = SettingsViewModel(
            engine: MockSettingsEngine(resetPreviewValue: preview)
        )

        model.refreshResetPreview()

        XCTAssertEqual(model.resetPreview.audioFiles, 5_000)
        XCTAssertEqual(
            model.resetImpactDescription(includeKeys: false, includePrompts: false),
            "Moves 5000 recordings from 42 days, 17 threads (512.0 MB) to Trash. "
                + "Your assistive.txt and three formatting prompt files will be preserved. "
                + "Codescribe will relaunch as a fresh install."
        )
        XCTAssertTrue(resetConfirmationMatches("RESET"))
        XCTAssertFalse(resetConfirmationMatches("reset"))
        XCTAssertFalse(resetConfirmationMatches(" RESET"))
    }

    func testPromptSourceLabelsExposeFileFallbackAndReadErrorTruth() {
        XCTAssertEqual(promptSourceLabel("custom_file"), "Custom file")
        XCTAssertEqual(promptSourceLabel("built_in_fallback"), "Built-in fallback")
        XCTAssertEqual(promptSourceLabel("read_error"), "Read error")
    }

    func testPromptRestoreTargetsOnlyTheConfirmedPrompt() {
        var restored: [String] = []
        let engine = MockSettingsEngine(
            promptRestoreObserver: { restored.append($0) }
        )
        let model = SettingsViewModel(engine: engine)

        XCTAssertNotNil(model.restoreFormattingPromptToDefault(.correction))
        XCTAssertNotNil(model.restoreFormattingPromptToDefault(.smart))
        XCTAssertNotNil(model.restoreFormattingPromptToDefault(.max))

        XCTAssertEqual(restored, ["correction", "smart", "max"])
    }

    func testFormattingPromptSnapshotsExposeDistinctPathsAndProvenance() throws {
        let model = SettingsViewModel(engine: MockSettingsEngine())
        let snapshots = try FormattingPolicyOption.editablePrompts.map { level in
            try XCTUnwrap(model.formattingPromptSnapshot(level: level))
        }

        XCTAssertEqual(
            snapshots.map { URL(fileURLWithPath: $0.path).lastPathComponent },
            ["formatting.txt", "formatting-smart.txt", "formatting-max.txt"]
        )
        XCTAssertEqual(
            snapshots.map(\.source),
            ["custom_file", "built_in_fallback", "built_in_fallback"]
        )
    }

    func testFailedPromptSaveDoesNotClaimARefreshedSnapshot() {
        let engine = MockSettingsEngine(
            promptSaveObserver: { _, _ in
                throw NSError(domain: "PromptWrite", code: 1)
            }
        )
        let model = SettingsViewModel(engine: engine)

        XCTAssertNil(model.saveAssistivePrompt("replacement"))
        XCTAssertNotNil(model.lastError)
        XCTAssertEqual(model.assistivePromptSnapshot().content, CsSettings.sampleAssistivePrompt)
    }

    func testAppResetPreservesPromptsUnlessSeparateOptInIsEnabled() {
        var calls: [(keys: Bool, prompts: Bool)] = []
        let engine = MockSettingsEngine(
            resetAppDataObserver: { calls.append(($0, $1)) }
        )
        let model = SettingsViewModel(engine: engine)

        // Exercise the bridge contract directly: SettingsViewModel relaunches
        // after success, which is intentionally not invoked in XCTest.
        try? engine.resetAppData(includeKeys: false, includePrompts: false)
        try? engine.resetAppData(includeKeys: true, includePrompts: true)

        XCTAssertEqual(calls.map(\.keys), [false, true])
        XCTAssertEqual(calls.map(\.prompts), [false, true])
        XCTAssertTrue(
            model.resetImpactDescription(includeKeys: false, includePrompts: true)
                .contains("assistive.txt and three formatting prompt files will also move to Trash")
        )
    }

    func testClearMcpConfigurationUsesDedicatedEngineContract() {
        var calls = 0
        let model = SettingsViewModel(
            engine: MockSettingsEngine(
                clearMcpConfigurationObserver: { calls += 1 }
            )
        )

        model.clearMcpConfiguration()

        XCTAssertEqual(calls, 1)
    }
}
