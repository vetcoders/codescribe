import XCTest
@testable import Codescribe

@MainActor
final class SettingsTruthTests: XCTestCase {
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
            model.resetImpactDescription(includeKeys: false),
            "Moves 5000 recordings from 42 days, 17 threads (512.0 MB) to Trash. "
                + "Codescribe will relaunch as a fresh install."
        )
        XCTAssertTrue(resetConfirmationMatches("RESET"))
        XCTAssertFalse(resetConfirmationMatches("reset"))
        XCTAssertFalse(resetConfirmationMatches(" RESET"))
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
