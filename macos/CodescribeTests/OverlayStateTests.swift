import AppKit
import XCTest
@testable import Codescribe

private final class OverlayStateTestEngine: DictationEngine {
    var pastedText: String?
    var formattedResult: Result<String, Error> = .success("")

    func setListener(_ listener: CsTranscriptionListener) {}
    func startRecording(language: CsLanguage?) async throws {}
    func stopRecording() async throws -> String { "" }
    func isRecording() async -> Bool { false }
    func initModel() async throws {}
    func isModelLoaded() -> Bool { true }
    func isFormattingAvailable() -> Bool { true }
    func formatText(text: String, language: CsLanguage?) async throws -> String {
        switch formattedResult {
        case .success(let text): return text
        case .failure(let error): throw error
        }
    }
    func pasteText(text: String) async throws { pastedText = text }
    func transcribeFile(path: String) async throws -> CsTranscription {
        CsTranscription(text: "", language: "en")
    }
}

@MainActor
final class OverlayStateTests: XCTestCase {
    func testAudioLevelMeterOrdersFiniteEnergyAndRejectsInvalidInput() throws {
        let meter = AudioLevelMeter()
        XCTAssertNil(meter.gain)

        meter.push(rms: 0)
        let silence = try XCTUnwrap(meter.gain)
        meter.reset()
        meter.push(rms: 0.01)
        let quiet = try XCTUnwrap(meter.gain)
        meter.reset()
        meter.push(rms: 0.8)
        let loud = try XCTUnwrap(meter.gain)

        XCTAssertTrue(silence.isFinite && quiet.isFinite && loud.isFinite)
        XCTAssertLessThan(silence, quiet)
        XCTAssertLessThan(quiet, loud)

        meter.reset()
        meter.push(rms: .nan)
        XCTAssertNil(meter.gain)
    }

    func testNoLevelFallbackRemainsExplicitlyAmbient() {
        let state = OverlayState()
        state.handleRecordingPreparing()
        state.handleRecordingStarted()

        XCTAssertNil(state.levelMeter.gain)
        XCTAssertFalse(state.hasMeasuredAudioLevel)
        XCTAssertEqual(state.statusText, "recording · ambient")
    }

    func testAudioLevelLifecycleDropsLateSamplesAndResets() {
        let state = OverlayState()

        state.applyAudioLevel(0.8)
        XCTAssertNil(state.levelMeter.gain, "levels before capture must be ignored")

        state.handleRecordingPreparing()
        state.applyAudioLevel(0.2)
        state.handleRecordingStarted()
        XCTAssertNotNil(state.levelMeter.gain)
        XCTAssertTrue(state.hasMeasuredAudioLevel)
        XCTAssertEqual(state.statusText, "recording")

        state.handleRecordingFinalising()
        XCTAssertNil(state.levelMeter.gain)
        XCTAssertFalse(state.hasMeasuredAudioLevel)

        state.applyAudioLevel(0.9)
        XCTAssertNil(state.levelMeter.gain, "late levels during finalisation must be ignored")

        state.finishControllerRecording()
        state.applyAudioLevel(0.9)
        XCTAssertNil(state.levelMeter.gain, "late levels after finalisation must be ignored")

        state.handleRecordingPreparing()
        state.handleRecordingStarted()
        XCTAssertNil(state.levelMeter.gain, "a new session must not inherit old amplitude")
        XCTAssertEqual(state.statusText, "recording · ambient")
    }

    func testTwoUtterancesAppendAndPreviewOnlyReplacesActiveTail() {
        let state = OverlayState()
        state.handleRecordingPreparing()
        state.handleRecordingStarted()

        state.applyPreview("first draft")
        state.applyFinal(utteranceId: 1, "first utterance")
        state.applyPreview("second draft")

        XCTAssertEqual(state.committedUtterances, ["first utterance"])
        XCTAssertEqual(state.liveText, "first utterance second draft")

        state.applyPreview("second draft revised")
        XCTAssertEqual(state.liveText, "first utterance second draft revised")

        state.applyFinal(utteranceId: 2, "second utterance")
        XCTAssertEqual(state.committedUtterances, ["first utterance", "second utterance"])
        XCTAssertEqual(state.preview, "")
        XCTAssertEqual(state.liveText, "first utterance second utterance")
    }

    func testSessionFinalisedStartsFinalPassUntilControllerStops() {
        let state = OverlayState()
        state.handleRecordingPreparing()
        state.handleRecordingStarted()
        state.applyFinal(utteranceId: 1, "captured text")

        state.handleRecordingFinalising()
        XCTAssertEqual(state.statusText, "transcribing")

        state.applySessionFinalised()
        XCTAssertEqual(state.mode, .listening)
        XCTAssertEqual(state.statusText, "final pass")

        state.finishControllerRecording()
        XCTAssertEqual(state.mode, .formatted)
        XCTAssertEqual(state.statusText, "done")
        XCTAssertEqual(state.formattedText, "captured text")
    }

    func testFailurePhaseIsExplicit() {
        let state = OverlayState()

        state.handleError(message: "engine unavailable")

        XCTAssertEqual(state.mode, .error)
        XCTAssertEqual(state.statusText, "failed")
    }

    func testFormatFailureShowsMarkerButKeepsTextClean() async {
        let state = OverlayState()
        let engine = OverlayStateTestEngine()
        engine.formattedResult = .failure(NSError(domain: "OverlayStateTests", code: 1))
        state.engine = engine
        state.formattedText = "raw source transcript"
        state.mode = .formatted

        state.formatTranscript()
        try? await Task.sleep(nanoseconds: 10_000_000)

        XCTAssertEqual(state.formattedText, "raw source transcript")
        XCTAssertEqual(state.formatFailureStatus, "raw — formatting failed")
        XCTAssertEqual(state.activeText, "raw source transcript")
    }

    func testCopyAndSendDismissTheOverlay() {
        let state = OverlayState()
        var closeCount = 0
        var sentText: String?
        state.onClose = { closeCount += 1 }
        state.onSendToAgent = { sentText = $0 }
        state.formattedText = "ready transcript"
        state.mode = .formatted

        state.copyToPasteboard()
        XCTAssertEqual(closeCount, 1)
        XCTAssertEqual(NSPasteboard.general.string(forType: .string), "ready transcript")

        state.sendToAgent()
        XCTAssertEqual(sentText, "ready transcript")
        // Since 845cec0 sendToAgent delegates dismissal to the onSendToAgent
        // closure (OverlayController wires the hide there) — no direct onClose.
        XCTAssertEqual(closeCount, 1)
    }

    func testPasteUsesEditedTextAndDismissesOverlay() async {
        let state = OverlayState()
        let engine = OverlayStateTestEngine()
        var closeCount = 0
        state.engine = engine
        state.onClose = { closeCount += 1 }
        state.applyFinalTranscript("original delivered transcript here")
        state.formattedText = "original delivered transcript here with user fix"
        state.mode = .formatted

        state.pasteToPreviousApp()
        try? await Task.sleep(nanoseconds: 10_000_000)

        XCTAssertEqual(engine.pastedText, "original delivered transcript here with user fix")
        XCTAssertEqual(closeCount, 1)
    }

    func testCaptureQualityIfEditedHitsAsyncPathOnUserEditWithoutBlocking() {
        // D-02: exercise quality capture decision (delivered != edited on .formatted)
        // and the fire-and-forget Task.detached (Copy/Send/Close must not block on I/O).
        // Uses applyFinalTranscript which seeds deliveredText (the pre-edit value).
        let state = OverlayState()
        var closeCount = 0
        state.onClose = { closeCount += 1 }

        // Seed delivered (raw from final transcript) then user edits formatted.
        state.applyFinalTranscript("original delivered transcript here")
        state.formattedText = "original delivered transcript here with user fix"
        state.mode = .formatted

        // Copy triggers captureQualityIfEdited because texts differ; must return immediately.
        state.copyToPasteboard()
        XCTAssertEqual(closeCount, 1)
        // The async commit to quality + lexicon happens off-main; test reaches here without wait.
    }

    func testOverlayPanelUsesNonActivatingStyle() {
        let state = OverlayState()
        let panel = DictationOverlayWindow.make(
            state: state,
            textScale: TextScaleController(key: "OverlayStateTests.textScale")
        )

        XCTAssertTrue(panel.styleMask.contains(.nonactivatingPanel))
        XCTAssertTrue(panel.isFloatingPanel)
        XCTAssertFalse(panel.canBecomeMain)
    }
}
