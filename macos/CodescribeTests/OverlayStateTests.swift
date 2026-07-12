import AppKit
import XCTest
@testable import Codescribe

@MainActor
final class OverlayStateTests: XCTestCase {
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
        XCTAssertEqual(closeCount, 2)
        XCTAssertEqual(sentText, "ready transcript")
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
