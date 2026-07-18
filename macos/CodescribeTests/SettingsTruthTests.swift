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
        XCTAssertEqual(
            LanguageIdentityPresentation.supportingCopy,
            "Programming vocabulary and your Voice Lab dictionary enrich the selected language."
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
