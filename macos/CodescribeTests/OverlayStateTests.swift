import AppKit
import XCTest
@testable import Codescribe

private final class OverlayStateTestEngine: DictationEngine {
    var pastedText: String?
    var pasteCallCount = 0
    var pasteOutcome: CsPasteOutcome = .pasted
    var pasteFrontmostAppNameValue: String?
    var deferredText: String?
    var deferOutcome: CsPasteOutcome = .deferredInsertArmed
    var deferredInsertShortcutValue: String? = "⌘⌥V"
    var deferredInsertFailureValue: String?
    var copiedTaggedText: String?
    var onCopyTagged: (() -> Void)?
    var formattedResult: Result<String, Error> = .success("")
    var formattedLevels: [FormattingPolicyOption] = []
    var onFormat: ((FormattingPolicyOption) -> Void)?
    var onPaste: (() -> Void)?
    var onDefer: (() -> Void)?
    var pasteTargetAppNameValue: String?
    var onPasteTargetRead: (() -> Void)?
    var persistedPolicy = OverlayPolicySnapshot(
        autoPasteEnabled: true,
        autoFormatLevel: .correction
    )
    var persistAutoPasteWrites = true
    var autoPasteWrites: [Bool] = []
    var policyReadCount = 0
    var sentAssistiveTexts: [String] = []
    var assistiveSendResult = true
    var onAssistiveSend: (() -> Void)?

    func setListener(_ listener: CsTranscriptionListener) {}
    func startRecording(language: CsLanguage?) async throws {}
    func stopRecording() async throws -> String { "" }
    func isRecording() async -> Bool { false }
    func initModel() async throws {}
    func isModelLoaded() -> Bool { true }
    func isFormattingAvailable() -> Bool { true }
    func currentOverlayPolicy() -> OverlayPolicySnapshot? {
        policyReadCount += 1
        return persistedPolicy
    }
    func setAutoPasteEnabled(_ enabled: Bool) {
        autoPasteWrites.append(enabled)
        guard persistAutoPasteWrites else { return }
        persistedPolicy = OverlayPolicySnapshot(
            autoPasteEnabled: enabled,
            autoFormatLevel: persistedPolicy.autoFormatLevel
        )
    }
    func formatText(
        text: String,
        language: CsLanguage?,
        level: FormattingPolicyOption
    ) async throws -> String {
        formattedLevels.append(level)
        onFormat?(level)
        switch formattedResult {
        case .success(let text): return text
        case .failure(let error): throw error
        }
    }
    func pasteText(text: String) async throws -> CsPasteResult {
        pastedText = text
        pasteCallCount += 1
        onPaste?()
        return CsPasteResult(
            outcome: pasteOutcome,
            targetAppName: pasteTargetAppNameValue,
            frontmostAppName: pasteFrontmostAppNameValue,
            deferredInsertShortcut: deferredInsertShortcutValue,
            deferredInsertFailure: deferredInsertFailureValue
        )
    }
    func deferText(text: String) async throws -> CsPasteResult {
        deferredText = text
        onDefer?()
        return CsPasteResult(
            outcome: deferOutcome,
            targetAppName: pasteTargetAppNameValue,
            frontmostAppName: "Codescribe",
            deferredInsertShortcut: deferredInsertShortcutValue,
            deferredInsertFailure: deferredInsertFailureValue
        )
    }
    func copyTaggedTranscript(text: String) async throws {
        copiedTaggedText = text
        onCopyTagged?()
    }
    func pasteTargetAppName() async -> String? {
        onPasteTargetRead?()
        return pasteTargetAppNameValue
    }
    func sendAssistiveTranscript(text: String) async throws -> Bool {
        sentAssistiveTexts.append(text)
        onAssistiveSend?()
        return assistiveSendResult
    }
    func transcribeFile(path: String) async throws -> CsTranscription {
        CsTranscription(text: "", language: "en")
    }
}

private final class OverlayStateTestClock {
    var now: TimeInterval = 0
}

@MainActor
final class OverlayStateTests: XCTestCase {
    private func makeFinalizedState(
        clock: OverlayStateTestClock,
        text: String = "ready transcript"
    ) -> OverlayState {
        let state = OverlayState(nowProvider: { clock.now })
        state.handleRecordingPreparing()
        state.handleRecordingStarted()
        state.applyFinal(utteranceId: 1, text)
        state.finishControllerRecording()
        return state
    }

    func testInsertActionPresentationNamesKnownTargetAndFallsBackHonestly() {
        let known = OverlayInsertActionPresentation(targetAppName: "Ghostty")
        XCTAssertEqual(known.targetAppName, "Ghostty")
        XCTAssertEqual(known.title, "Insert → Ghostty")
        XCTAssertEqual(known.help, "Insert at the cursor in Ghostty")

        let blank = OverlayInsertActionPresentation(targetAppName: "  ")
        XCTAssertNil(blank.targetAppName)
        XCTAssertEqual(blank.title, "Insert")

        let unknown = OverlayInsertActionPresentation(targetAppName: nil)
        XCTAssertEqual(unknown.title, "Insert")
        XCTAssertEqual(unknown.help, "Insert at the cursor in the previous app")
    }

    func testApprovedOverlayActionPresentationIsLiteralAndLevelBounded() {
        XCTAssertEqual(OverlayActionPresentation.sendTitle, "To Agent")
        XCTAssertEqual(OverlayActionPresentation.sendHelp, "Send transcript to the agent")
        XCTAssertEqual(
            OverlayActionPresentation.manualFormatLevels.map(\.rawValue),
            ["correction", "smart", "max"]
        )
        XCTAssertEqual(
            OverlayActionPresentation.manualFormatLevels.map(\.visibleName),
            ["Correction", "Smart", "Max"]
        )
    }

    func testOverlayPolicyRefreshesAtSessionEntryFromPersistedTruth() {
        let state = OverlayState()
        let engine = OverlayStateTestEngine()
        engine.persistedPolicy = OverlayPolicySnapshot(
            autoPasteEnabled: false,
            autoFormatLevel: .off
        )
        state.engine = engine

        state.handleRecordingPreparing()
        XCTAssertFalse(state.autoPasteEnabled)
        XCTAssertEqual(state.autoFormatLevel, .off)
        XCTAssertEqual(engine.policyReadCount, 1)

        engine.persistedPolicy = OverlayPolicySnapshot(
            autoPasteEnabled: true,
            autoFormatLevel: .max
        )
        state.handleRecordingStarted()
        XCTAssertTrue(state.autoPasteEnabled)
        XCTAssertEqual(state.autoFormatLevel, .max)
        XCTAssertEqual(engine.policyReadCount, 2)
    }

    func testAutoPasteWriteReconcilesSuccessAndFailureWithoutDelivery() {
        for persists in [true, false] {
            let state = OverlayState()
            let engine = OverlayStateTestEngine()
            engine.persistedPolicy = OverlayPolicySnapshot(
                autoPasteEnabled: false,
                autoFormatLevel: .off
            )
            engine.persistAutoPasteWrites = persists
            state.engine = engine
            state.handleRecordingPreparing()

            state.setAutoPasteEnabled(true)

            XCTAssertEqual(engine.autoPasteWrites, [true])
            XCTAssertEqual(state.autoPasteEnabled, persists)
            XCTAssertEqual(state.autoFormatLevel, .off)
            XCTAssertEqual(engine.policyReadCount, 2)
            XCTAssertEqual(engine.pasteCallCount, 0)
        }
    }

    func testAssistiveFenceMakesAutoPasteControlUnavailableAndNonWriting() {
        let state = OverlayState()
        let engine = OverlayStateTestEngine()
        state.engine = engine
        state.setAutoPasteControlAvailable(false)

        state.setAutoPasteEnabled(false)

        XCTAssertFalse(state.autoPasteControlAvailable)
        XCTAssertTrue(engine.autoPasteWrites.isEmpty)
        XCTAssertEqual(engine.pasteCallCount, 0)
    }

    func testPasteTargetRefreshesAtPreparingAndStartedSessionEntry() async {
        let state = OverlayState()
        let engine = OverlayStateTestEngine()
        state.engine = engine

        let preparingRead = expectation(description: "preparing target read")
        engine.pasteTargetAppNameValue = "Ghostty"
        engine.onPasteTargetRead = { preparingRead.fulfill() }
        state.handleRecordingPreparing()
        await fulfillment(of: [preparingRead], timeout: 1)
        await Task.yield()
        XCTAssertEqual(state.insertActionPresentation.title, "Insert → Ghostty")

        let startedRead = expectation(description: "started target read")
        engine.pasteTargetAppNameValue = nil
        engine.onPasteTargetRead = { startedRead.fulfill() }
        state.handleRecordingStarted()
        await fulfillment(of: [startedRead], timeout: 1)
        await Task.yield()
        XCTAssertEqual(state.insertActionPresentation.title, "Insert")
        XCTAssertEqual(
            state.insertActionPresentation.help,
            "Insert at the cursor in the previous app"
        )
    }

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
        let formatCalled = expectation(description: "format called")
        engine.onFormat = { _ in formatCalled.fulfill() }
        state.engine = engine
        state.formattedText = "raw source transcript"
        state.mode = .formatted

        state.formatTranscript(level: .correction)
        await fulfillment(of: [formatCalled], timeout: 1)
        await Task.yield()

        XCTAssertEqual(state.formattedText, "raw source transcript")
        XCTAssertEqual(state.formatFailureStatus, "raw — formatting failed")
        XCTAssertEqual(state.activeText, "raw source transcript")
        XCTAssertFalse(state.canRevert)
    }

    func testSuccessfulFormatStoresOneExactSourceAndSecondSuccessReplacesIt() async {
        let state = OverlayState()
        let engine = OverlayStateTestEngine()
        state.engine = engine
        state.formattedText = "  source bytes stay exact  "
        state.mode = .formatted

        let firstCalled = expectation(description: "first format")
        engine.formattedResult = .success("first formatted result")
        engine.onFormat = { _ in firstCalled.fulfill() }
        state.formatTranscript(level: .smart)
        await fulfillment(of: [firstCalled], timeout: 1)
        await Task.yield()

        XCTAssertTrue(state.canRevert)
        XCTAssertEqual(state.formattedText, "first formatted result")

        let secondCalled = expectation(description: "second format")
        engine.formattedResult = .success("second formatted result")
        engine.onFormat = { _ in secondCalled.fulfill() }
        state.formatTranscript(level: .max)
        await fulfillment(of: [secondCalled], timeout: 1)
        await Task.yield()

        XCTAssertEqual(state.formattedText, "second formatted result")
        state.revertFormat()
        XCTAssertEqual(state.formattedText, "first formatted result")
        XCTAssertFalse(state.canRevert, "Revert is a one-use slot, not a history stack")
    }

    func testEmptyAndIdenticalFormatResultsCreateNoRevert() async {
        for result in ["   \n", "source transcript"] {
            let state = OverlayState()
            let engine = OverlayStateTestEngine()
            let called = expectation(description: "format \(result.debugDescription)")
            engine.formattedResult = .success(result)
            engine.onFormat = { _ in called.fulfill() }
            state.engine = engine
            state.formattedText = "source transcript"
            state.mode = .formatted

            state.formatTranscript(level: .correction)
            await fulfillment(of: [called], timeout: 1)
            await Task.yield()

            XCTAssertEqual(state.formattedText, "source transcript")
            XCTAssertFalse(state.canRevert)
        }
    }

    func testFailedSecondFormatKeepsPriorSuccessfulRevert() async {
        let state = OverlayState()
        let engine = OverlayStateTestEngine()
        state.engine = engine
        state.formattedText = "source transcript"
        state.mode = .formatted

        let successCalled = expectation(description: "successful format")
        engine.formattedResult = .success("formatted transcript")
        engine.onFormat = { _ in successCalled.fulfill() }
        state.formatTranscript(level: .smart)
        await fulfillment(of: [successCalled], timeout: 1)
        await Task.yield()

        let failureCalled = expectation(description: "failed second format")
        engine.formattedResult = .failure(NSError(domain: "OverlayStateTests", code: 2))
        engine.onFormat = { _ in failureCalled.fulfill() }
        state.formatTranscript(level: .max)
        await fulfillment(of: [failureCalled], timeout: 1)
        await Task.yield()

        XCTAssertEqual(state.formattedText, "formatted transcript")
        XCTAssertTrue(state.canRevert)
        state.revertFormat()
        XCTAssertEqual(state.formattedText, "source transcript")
    }

    func testManualFormatPassesEveryExplicitLevelAndKeepsAutoFormatOff() async {
        let state = OverlayState()
        let engine = OverlayStateTestEngine()
        engine.persistedPolicy = OverlayPolicySnapshot(
            autoPasteEnabled: false,
            autoFormatLevel: .off
        )
        state.engine = engine
        state.handleRecordingPreparing()
        state.formattedText = "source transcript"
        state.mode = .formatted

        for level in OverlayActionPresentation.manualFormatLevels {
            let called = expectation(description: "format \(level.rawValue)")
            engine.formattedResult = .success("\(level.rawValue) output")
            engine.onFormat = { selected in
                XCTAssertEqual(selected, level)
                called.fulfill()
            }

            state.formatTranscript(level: level)
            await fulfillment(of: [called], timeout: 1)
            await Task.yield()

            XCTAssertEqual(state.formattedText, "\(level.rawValue) output")
            XCTAssertEqual(state.autoFormatLevel, .off)
            XCTAssertEqual(engine.pasteCallCount, 0)
        }

        XCTAssertEqual(engine.formattedLevels, [.correction, .smart, .max])
        XCTAssertTrue(engine.autoPasteWrites.isEmpty)

        state.formatTranscript(level: .off)
        await Task.yield()

        XCTAssertEqual(engine.formattedLevels, [.correction, .smart, .max])
        XCTAssertEqual(state.formattedText, "max output")
        XCTAssertEqual(engine.pasteCallCount, 0)
    }

    func testAutoHideDelayIsFiveSeconds() {
        XCTAssertEqual(OverlayState.autoHideDelaySeconds, 5)
    }

    func testInjectedClockFiresFiveSecondsAfterFinalization() {
        let clock = OverlayStateTestClock()
        let state = makeFinalizedState(clock: clock)
        var closeCount = 0
        state.onClose = { closeCount += 1 }

        clock.now = 4.9
        state.fireAutoHideNowForTests()
        XCTAssertEqual(closeCount, 0)

        clock.now = 5
        state.fireAutoHideNowForTests()
        XCTAssertEqual(closeCount, 1)
    }

    func testTextEditReanchorsAutoHide() {
        let clock = OverlayStateTestClock()
        let state = makeFinalizedState(clock: clock)
        var closeCount = 0
        state.onClose = { closeCount += 1 }

        clock.now = 4
        state.userEditedTranscript("ready transcript with correction")
        clock.now = 5
        state.fireAutoHideNowForTests()
        XCTAssertEqual(closeCount, 0)

        clock.now = 9
        state.fireAutoHideNowForTests()
        XCTAssertEqual(closeCount, 1)
    }

    func testWindowDragReanchorsAutoHide() {
        let clock = OverlayStateTestClock()
        let state = makeFinalizedState(clock: clock)
        var closeCount = 0
        state.onClose = { closeCount += 1 }

        clock.now = 4
        state.userDraggedOverlay()
        clock.now = 5
        state.fireAutoHideNowForTests()
        XCTAssertEqual(closeCount, 0)

        clock.now = 9
        state.fireAutoHideNowForTests()
        XCTAssertEqual(closeCount, 1)
    }

    func testWindowResizeReanchorsAutoHide() {
        let clock = OverlayStateTestClock()
        let state = makeFinalizedState(clock: clock)
        var closeCount = 0
        state.onClose = { closeCount += 1 }

        clock.now = 4
        state.userResizedOverlay()
        clock.now = 5
        state.fireAutoHideNowForTests()
        XCTAssertEqual(closeCount, 0)

        clock.now = 9
        state.fireAutoHideNowForTests()
        XCTAssertEqual(closeCount, 1)
    }

    func testHoverPausesAndPointerExitStartsFreshCountdown() {
        let clock = OverlayStateTestClock()
        let state = makeFinalizedState(clock: clock)
        var closeCount = 0
        state.onClose = { closeCount += 1 }

        clock.now = 4
        state.setPointerHovering(true)
        clock.now = 100
        state.fireAutoHideNowForTests()
        XCTAssertEqual(closeCount, 0)

        state.setPointerHovering(false)
        clock.now = 104.9
        state.fireAutoHideNowForTests()
        XCTAssertEqual(closeCount, 0)
        clock.now = 105
        state.fireAutoHideNowForTests()
        XCTAssertEqual(closeCount, 1)
    }

    func testCopyKeepsOverlayVisibleAndRearmsAutoHide() {
        let clock = OverlayStateTestClock()
        let state = makeFinalizedState(clock: clock)
        var closeCount = 0
        state.onClose = { closeCount += 1 }

        clock.now = 4
        state.copyToPasteboard()
        XCTAssertEqual(closeCount, 0)
        XCTAssertEqual(NSPasteboard.general.string(forType: .string), "ready transcript")

        clock.now = 5
        state.fireAutoHideNowForTests()
        XCTAssertEqual(closeCount, 0)
        clock.now = 9
        state.fireAutoHideNowForTests()
        XCTAssertEqual(closeCount, 1)
    }

    func testPasteUsesEditedTextKeepsOverlayVisibleAndRearmsAutoHide() async {
        let clock = OverlayStateTestClock()
        let state = makeFinalizedState(clock: clock, text: "original delivered transcript here")
        let engine = OverlayStateTestEngine()
        let pasteCalled = expectation(description: "paste called")
        engine.onPaste = { pasteCalled.fulfill() }
        var closeCount = 0
        state.engine = engine
        state.onClose = { closeCount += 1 }
        state.insertCaretInCodescribeProbe = { false }
        state.userEditedTranscript("original delivered transcript here with user fix")

        clock.now = 4
        state.pasteToPreviousApp()
        await fulfillment(of: [pasteCalled], timeout: 1)
        await Task.yield()

        XCTAssertEqual(engine.pastedText, "original delivered transcript here with user fix")
        XCTAssertEqual(closeCount, 0)
        clock.now = 5
        state.fireAutoHideNowForTests()
        XCTAssertEqual(closeCount, 0)
        clock.now = 9
        state.fireAutoHideNowForTests()
        XCTAssertEqual(closeCount, 1)
    }

    func testInsertArmsDeferredSlotWithoutCopyWhenCaretIsInCodescribe() async {
        let clock = OverlayStateTestClock()
        let state = makeFinalizedState(clock: clock, text: "guarded transcript")
        let engine = OverlayStateTestEngine()
        let deferCalled = expectation(description: "deferred insert armed")
        engine.onDefer = { deferCalled.fulfill() }
        engine.pasteTargetAppNameValue = "Pensieve"
        state.engine = engine
        state.insertCaretInCodescribeProbe = { true }

        state.pasteToPreviousApp()
        await fulfillment(of: [deferCalled], timeout: 1)
        await Task.yield()

        XCTAssertEqual(engine.deferredText, "guarded transcript")
        XCTAssertNil(engine.copiedTaggedText)
        XCTAssertNil(engine.pastedText, "guard must not fall through to synthetic paste")
        XCTAssertEqual(
            state.toast,
            "Couldn't reach Pensieve — put your cursor where you want the text "
                + "and press ⌘⌥V. Your clipboard is untouched."
        )
    }

    func testInsertFallsBackToTaggedCopyWhenHotkeyRegistrationFails() async {
        let clock = OverlayStateTestClock()
        let state = makeFinalizedState(clock: clock, text: "fallback transcript")
        let engine = OverlayStateTestEngine()
        engine.deferOutcome = .copiedToClipboard
        engine.deferredInsertFailureValue = "Paste Here hotkey registration failed"
        engine.pasteTargetAppNameValue = "Pensieve"
        let deferCalled = expectation(description: "deferred insert fallback")
        engine.onDefer = { deferCalled.fulfill() }
        state.engine = engine
        state.insertCaretInCodescribeProbe = { true }

        state.pasteToPreviousApp()
        await fulfillment(of: [deferCalled], timeout: 1)
        await Task.yield()

        XCTAssertEqual(
            state.toast,
            "Paste Here hotkey registration failed — copied with tags instead. "
                + "Clipboard replaced; press Cmd+V where you want it."
        )
    }

    func testInsertShowsCopiedToastWhenControllerGuardDegrades() async {
        let clock = OverlayStateTestClock()
        let state = makeFinalizedState(clock: clock, text: "belt and braces transcript")
        let engine = OverlayStateTestEngine()
        engine.pasteOutcome = .copiedToClipboard
        engine.deferredInsertShortcutValue = nil
        engine.pasteTargetAppNameValue = "Pensieve"
        engine.pasteFrontmostAppNameValue = "Alacritty"
        let pasteCalled = expectation(description: "paste called")
        engine.onPaste = { pasteCalled.fulfill() }
        state.engine = engine
        state.insertCaretInCodescribeProbe = { false }

        state.pasteToPreviousApp()
        await fulfillment(of: [pasteCalled], timeout: 1)
        await Task.yield()

        XCTAssertEqual(engine.pastedText, "belt and braces transcript")
        XCTAssertEqual(
            state.toast,
            "Copied — your cursor is in Alacritty, not Pensieve. "
                + "Clipboard replaced; press Cmd+V where you want it."
        )
    }

    func testInsertShowsAccessibilityPermissionToastWhenEventPostingDenied() async {
        let clock = OverlayStateTestClock()
        let state = makeFinalizedState(clock: clock, text: "permission transcript")
        let engine = OverlayStateTestEngine()
        engine.pasteOutcome = .accessibilityPermissionNeeded
        engine.deferredInsertFailureValue = "Paste Here hotkey registration failed"
        engine.pasteTargetAppNameValue = "Pensieve"
        engine.pasteFrontmostAppNameValue = "Pensieve"
        let pasteCalled = expectation(description: "permission fallback called")
        engine.onPaste = { pasteCalled.fulfill() }
        state.engine = engine
        state.insertCaretInCodescribeProbe = { false }

        state.pasteToPreviousApp()
        await fulfillment(of: [pasteCalled], timeout: 1)
        await Task.yield()

        XCTAssertEqual(engine.pastedText, "permission transcript")
        XCTAssertEqual(
            state.toast,
            "Paste Here hotkey registration failed — copied with tags instead. "
                + "Clipboard replaced; press Cmd+V where you want it."
        )
    }

    func testFormatCancelsAutoHideWithoutRearming() async {
        let clock = OverlayStateTestClock()
        let state = makeFinalizedState(clock: clock)
        let engine = OverlayStateTestEngine()
        let formatCalled = expectation(description: "format called")
        engine.formattedResult = .success("formatted result")
        engine.onFormat = { _ in formatCalled.fulfill() }
        state.engine = engine
        var closeCount = 0
        state.onClose = { closeCount += 1 }

        clock.now = 4
        state.formatTranscript(level: .correction)
        await fulfillment(of: [formatCalled], timeout: 1)
        await Task.yield()
        XCTAssertEqual(state.formattedText, "formatted result")

        clock.now = 100
        state.fireAutoHideNowForTests()
        XCTAssertEqual(closeCount, 0)
    }

    func testRevertRestoresExactTextAndStartsFreshFiveSecondDeadline() async {
        let clock = OverlayStateTestClock()
        let state = makeFinalizedState(clock: clock, text: "exact source bytes")
        let engine = OverlayStateTestEngine()
        let formatCalled = expectation(description: "format called")
        engine.formattedResult = .success("formatted result")
        engine.onFormat = { _ in formatCalled.fulfill() }
        state.engine = engine
        var closeCount = 0
        state.onClose = { closeCount += 1 }
        state.userEditedTranscript("  exact source bytes  ")

        clock.now = 4
        state.formatTranscript(level: .max)
        await fulfillment(of: [formatCalled], timeout: 1)
        await Task.yield()
        clock.now = 100
        state.revertFormat()

        XCTAssertEqual(state.formattedText, "  exact source bytes  ")
        XCTAssertFalse(state.canRevert)
        clock.now = 104.9
        state.fireAutoHideNowForTests()
        XCTAssertEqual(closeCount, 0)
        clock.now = 105
        state.fireAutoHideNowForTests()
        XCTAssertEqual(closeCount, 1)
    }

    func testCloseIsImmediateAndAgentButtonUsesControllerDelivery() async {
        let clock = OverlayStateTestClock()
        let engine = OverlayStateTestEngine()
        let state = OverlayState(nowProvider: { clock.now })
        state.engine = engine
        state.applyIndicatorMode(.assistive)
        state.handleRecordingPreparing()
        state.handleRecordingStarted()
        state.applyFinal(utteranceId: 1, "ready transcript")
        state.finishControllerRecording()
        var closeCount = 0
        var sentText: String?
        state.onClose = { closeCount += 1 }
        state.onSendToAgent = { sentText = $0 }
        let delivered = expectation(description: "agent button delivered")
        engine.onAssistiveSend = { delivered.fulfill() }

        state.sendToAgent()
        await fulfillment(of: [delivered], timeout: 1)
        XCTAssertEqual(sentText, "ready transcript")
        XCTAssertEqual(engine.sentAssistiveTexts, ["ready transcript"])
        XCTAssertEqual(closeCount, 1)

        state.close()
        XCTAssertEqual(closeCount, 2, "Close button and brand CloseDot share this action")
    }

    func testUntouchedAgentFinalAutoSendsAtDeadline() async {
        let clock = OverlayStateTestClock()
        let engine = OverlayStateTestEngine()
        let state = OverlayState(nowProvider: { clock.now })
        state.engine = engine
        state.applyIndicatorMode(.assistive)
        state.handleRecordingPreparing()
        state.handleRecordingStarted()
        state.applyFinal(utteranceId: 1, "untouched final")
        state.finishControllerRecording()
        let delivered = expectation(description: "untouched final delivered")
        engine.onAssistiveSend = { delivered.fulfill() }

        clock.now = 5
        state.fireAutoHideNowForTests()
        await fulfillment(of: [delivered], timeout: 1)
        XCTAssertEqual(engine.sentAssistiveTexts, ["untouched final"])
    }

    func testContextMarkerLandsAtCapturedWordPositionAndSurvivesFinalPass() {
        let state = OverlayState()
        state.applyIndicatorMode(.assistive)
        state.handleRecordingPreparing()
        state.handleRecordingStarted()
        state.applyPreview("alpha beta")

        state.applyContextMarker(position: 5, marker: "{selection_1}")
        XCTAssertEqual(state.liveText, "alpha {selection_1} beta")

        state.applyFinal(utteranceId: 1, "alpha beta")
        state.applyFinalTranscript("alpha beta")
        state.finishControllerRecording()
        XCTAssertEqual(state.formattedText, "alpha {selection_1} beta")
        XCTAssertEqual(state.activeText, "alpha {selection_1} beta")
    }

    func testContextMarkerInsideWordStaysUnpaddedForLosslessTitles() {
        let state = OverlayState()
        state.applyIndicatorMode(.assistive)
        state.handleRecordingPreparing()
        state.handleRecordingStarted()
        state.applyPreview("bardzo mnie drażni")

        // Position 9 splits "mnie" between "mn" and "ie": no padding, so the
        // split stays lossless for downstream title derivation.
        state.applyContextMarker(position: 9, marker: "{selection_1}")
        XCTAssertEqual(state.liveText, "bardzo mn{selection_1}ie drażni")
    }

    func testContextMarkersAtSamePositionKeepCaptureOrder() {
        let state = OverlayState()
        state.handleRecordingPreparing()
        state.handleRecordingStarted()
        state.applyPreview("alpha")

        state.applyContextMarker(position: 5, marker: "{selection_1}")
        state.applyContextMarker(position: 5, marker: "{selection_2}")
        state.applyContextMarker(position: 5, marker: "{selection_3}")

        XCTAssertEqual(
            state.liveText,
            "alpha {selection_1} {selection_2} {selection_3}"
        )
    }

    func testAnyAgentFinalEditPermanentlyVetoesAutoSendUntilButton() async {
        let clock = OverlayStateTestClock()
        let engine = OverlayStateTestEngine()
        let state = OverlayState(nowProvider: { clock.now })
        state.engine = engine
        state.applyIndicatorMode(.assistive)
        state.handleRecordingPreparing()
        state.handleRecordingStarted()
        state.applyFinal(utteranceId: 1, "original final")
        state.finishControllerRecording()
        state.userEditedTranscript("edited final")
        state.userEditedTranscript("original final")

        clock.now = 5
        state.fireAutoHideNowForTests()
        await Task.yield()
        XCTAssertTrue(engine.sentAssistiveTexts.isEmpty)

        let delivered = expectation(description: "edited final delivered by button")
        engine.onAssistiveSend = { delivered.fulfill() }
        state.sendToAgent()
        await fulfillment(of: [delivered], timeout: 1)
        XCTAssertEqual(engine.sentAssistiveTexts, ["original final"])
    }

    func testNoSpeechAutoHidesAfterFiveSeconds() {
        let clock = OverlayStateTestClock()
        let state = OverlayState(nowProvider: { clock.now })
        var closeCount = 0
        state.onClose = { closeCount += 1 }
        state.handleRecordingPreparing()
        state.handleRecordingStarted()
        state.finishControllerRecording()

        XCTAssertEqual(state.mode, .noSpeech)
        clock.now = 5
        state.fireAutoHideNowForTests()
        XCTAssertEqual(closeCount, 1)
    }

    func testErrorAutoHidesAfterFiveSeconds() {
        let clock = OverlayStateTestClock()
        let state = OverlayState(nowProvider: { clock.now })
        var closeCount = 0
        state.onClose = { closeCount += 1 }

        state.handleError(message: "engine unavailable")
        XCTAssertEqual(state.mode, .error)
        clock.now = 5
        state.fireAutoHideNowForTests()
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
        XCTAssertEqual(closeCount, 0, "quality capture must not change Copy's stay-visible contract")
        // The async commit to quality + lexicon happens off-main; test reaches here without wait.
    }

    func testOverlayOffNeverOrdersPanelFront() {
        var factoryCount = 0
        var frontCount = 0
        let controller = OverlayController(
            state: OverlayState(),
            engine: nil,
            overlayEnabledProvider: { false },
            assistiveStatusProvider: { false },
            panelFactory: { _, _ in
                factoryCount += 1
                return NSPanel()
            },
            orderPanelFront: { _ in frontCount += 1 },
            orderPanelOut: { _ in }
        )

        controller.showForRecording()
        XCTAssertEqual(factoryCount, 0)
        XCTAssertEqual(frontCount, 0)
        XCTAssertTrue(controller.state.autoPasteControlAvailable)
    }

    func testAgentModesAlwaysOrderOverlayFrontWhenToggleIsOn() {
        for mode in ["Chat", "Selection"] {
            var frontCount = 0
            let controller = OverlayController(
                state: OverlayState(),
                engine: nil,
                overlayEnabledProvider: { true },
                assistiveStatusProvider: { true },
                panelFactory: { _, _ in NSPanel() },
                orderPanelFront: { _ in frontCount += 1 },
                orderPanelOut: { _ in }
            )

            controller.showForRecording()
            XCTAssertEqual(frontCount, 1, "\(mode) shares the canonical overlay path")
            XCTAssertFalse(controller.state.autoPasteControlAvailable)
        }
    }

    func testMidHoldAssistiveUpgradeKeepsOverlayVisibleAndFlipsSemantics() {
        var frontCount = 0
        var outCount = 0
        let controller = OverlayController(
            state: OverlayState(),
            engine: nil,
            overlayEnabledProvider: { true },
            assistiveStatusProvider: { false },
            panelFactory: { _, _ in NSPanel() },
            orderPanelFront: { _ in frontCount += 1 },
            orderPanelOut: { _ in outCount += 1 }
        )

        controller.showForRecording()
        XCTAssertEqual(frontCount, 1)
        XCTAssertEqual(outCount, 0)

        controller.handleIndicatorModeChange(.assistive)
        XCTAssertEqual(outCount, 0)
        XCTAssertEqual(controller.state.indicatorMode, .assistive)
        XCTAssertFalse(controller.state.autoPasteControlAvailable)
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
