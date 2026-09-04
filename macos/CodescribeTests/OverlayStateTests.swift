import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

// Executed by `make test-swift`. The marker-rebase assertions below run for
// real; see CodescribeTests/README.md.

@MainActor
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
  func transcribeFile(path _: String) async throws -> CsTranscription {
    CsTranscription(text: "", language: "pl")
  }
}

private final class OverlayStateTestClock {
  var now: TimeInterval = 0
}

@MainActor
final class OverlayStateTests: XCTestCase {
  private var nextProjectionSequence: UInt64 = 0

  func testListenerQueuesLifecycleEventsInCallbackOrder() async {
    let channel = AsyncStream<OverlayListenerEvent>.makeStream()
    let listener = DictationListener(continuation: channel.continuation)
    listener.onRecordingPreparing()
    listener.onRecordingStarted()
    listener.onRecordingFinalising()
    listener.onRecordingStopped()

    var iterator = channel.stream.makeAsyncIterator()
    guard case .recordingPreparing = await iterator.next() else {
      return XCTFail("preparing callback lost or reordered")
    }
    guard case .recordingStarted = await iterator.next() else {
      return XCTFail("started callback lost or reordered")
    }
    guard case .recordingFinalising = await iterator.next() else {
      return XCTFail("finalising callback lost or reordered")
    }
    guard case .recordingStopped = await iterator.next() else {
      return XCTFail("stopped callback lost or reordered")
    }
  }

  /// Admit Rust-owned transcript truth through the same projection boundary as
  /// production. Tests may choose rendered documents; they do not replay the
  /// demolished Swift preview/final/patch reducer.
  private func projectText(
    _ text: String,
    to state: OverlayState,
    mode: String = "dictation",
    phase: String? = nil,
    canPaste: Bool = false,
    canInsert: Bool = false,
    canCopy: Bool? = nil,
    canRetranscribe: Bool = false,
    canFormat: Bool = false,
    terminal: Bool = false,
    includesWordEvidence: Bool = true
  ) {
    nextProjectionSequence += 1
    let sequence = nextProjectionSequence
    let sampleStart = (sequence - 1) * 16_000
    let sampleEnd = sequence * 16_000
    let projectedPhase = phase ?? (terminal ? "formatted" : "listening")
    let receipt = CsProjectedAcousticReceipt(
      acousticSerialVersion: 1,
      acousticSerial: "test-acoustic-\(sequence)",
      sessionId: "overlay-state-tests",
      captureEpoch: 1,
      sampleStart: sampleStart,
      sampleEnd: sampleEnd,
      durationMs: 1_000,
      energyIntegral: 1,
      meanRmsDbfs: -20,
      peakDbfs: -6,
      vadOpenSample: sampleStart,
      vadCloseSample: sampleEnd,
      evidenceCalibrationVersion: "test-v1",
      wordEvidenceReceipts: includesWordEvidence ? ["test-word-evidence-\(sequence)"] : [],
      layerDecisionReceipts: ["test-layer-decision-\(sequence)"],
      sealReceipt: terminal ? "test-seal-\(sequence)" : nil,
      manualEditReceipt: nil
    )
    state.applyTranscriptProjection(
      CsTranscriptProjectionEvent(
        schema: "codescribe.transcript_projection.v1",
        sequence: sequence,
        emittedAt: "2026-08-25T00:00:00Z",
        sessionId: "overlay-state-tests",
        mode: mode,
        reducerRevision: sequence,
        reducerAction: terminal
          ? "record_ledger_terminal_seal"
          : "record_ledger_projection",
        occurrenceSessionId: "overlay-state-tests",
        captureEpoch: 1,
        sampleStart: sampleStart,
        sampleEnd: sampleEnd,
        documentIndex: sequence - 1,
        label: terminal ? "terminal" : "live",
        renderedText: text,
        phase: projectedPhase,
        canPaste: canPaste,
        canInsert: canInsert,
        canCopy: canCopy ?? !text.isEmpty,
        canRetranscribe: canRetranscribe,
        canFormat: canFormat,
        terminal: terminal,
        acousticReceipts: [receipt]
      )
    )
  }

  private func makeFinalizedState(
    clock: OverlayStateTestClock,
    text: String = "ready transcript"
  ) -> OverlayState {
    let state = OverlayState(nowProvider: { clock.now })
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectText(text, to: state, terminal: true)
    state.finishControllerRecording()
    return state
  }

  func testOverlaySessionTimerTracksCaptureAndFreezesOnStop() {
    let clock = OverlayStateTestClock()
    let state = OverlayState(nowProvider: { clock.now })
    XCTAssertNil(state.elapsedCaptureSeconds())
    XCTAssertEqual(state.sessionTimerText, "00:00")

    clock.now = 100
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    XCTAssertEqual(state.elapsedCaptureSeconds(), 0)

    clock.now = 165
    XCTAssertEqual(state.elapsedCaptureSeconds(), 65)
    XCTAssertEqual(state.sessionTimerText, "01:05")

    // Native finalising freezes the clock — the final pass must not tick.
    state.handleRecordingFinalising()
    clock.now = 200
    XCTAssertEqual(state.elapsedCaptureSeconds(), 65)
    state.finishControllerRecording()
    XCTAssertEqual(state.elapsedCaptureSeconds(), 65)

    // A fresh session restarts from zero and formats hours past 59:59.
    clock.now = 300
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    XCTAssertEqual(state.elapsedCaptureSeconds(), 0)
    clock.now = 3900
    XCTAssertEqual(state.sessionTimerText, "1:00:00")
    XCTAssertTrue(state.showsSessionTimer)
  }

  func testCanonicalProjectionOwnsCanvasAndCopyFromFirstAdmittedRevision() {
    let state = OverlayState()
    XCTAssertFalse(state.canCopy)
    XCTAssertFalse(state.showsSessionTimer)

    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    XCTAssertTrue(state.showsSessionTimer)

    projectText("analyze the repo", to: state)
    XCTAssertTrue(state.canCopy)
    XCTAssertEqual(state.activeText, "analyze the repo")
    XCTAssertEqual(state.liveText, "analyze the repo")

    projectText("analyze the repo for duplicate dispatch", to: state)
    XCTAssertTrue(state.canCopy)
    XCTAssertEqual(state.activeText, "analyze the repo for duplicate dispatch")
    XCTAssertEqual(state.listeningDisplay, "analyze the repo for duplicate dispatch")
  }

  func testAdmittedProjectionPaintsWithoutSwiftRevalidatingReceipts() {
    let state = OverlayState()
    state.handleRecordingPreparing()
    state.handleRecordingStarted()

    projectText("unproven shadow", to: state, includesWordEvidence: false)

    XCTAssertTrue(state.canCopy)
    XCTAssertEqual(state.activeText, "unproven shadow")
    XCTAssertEqual(state.mode, .listening)
  }

  func testLatestProjectionIsPaintedWithoutASecondSequenceReducer() {
    let state = OverlayState()
    var successfulSignals = 0
    state.onSuccessfulDictation = { successfulSignals += 1 }
    state.handleRecordingPreparing()
    state.handleRecordingStarted()

    projectText("sealed document", to: state, terminal: true)
    projectText("late competing document", to: state)

    XCTAssertEqual(state.activeText, "late competing document")
    XCTAssertEqual(state.formattedText, "late competing document")
    XCTAssertEqual(state.mode, .listening)
    XCTAssertEqual(successfulSignals, 1)
  }

  func testPresencePolicyRisesForScreenshotAndYieldsToAlerts() {
    XCTAssertEqual(
      OverlayPresencePolicy.resolve(screenshotChord: true, shouldYield: true),
      .capture
    )
    XCTAssertEqual(
      OverlayPresencePolicy.resolve(screenshotChord: false, shouldYield: true),
      .yield
    )
    XCTAssertEqual(
      OverlayPresencePolicy.resolve(screenshotChord: false, shouldYield: false),
      .rest
    )
    XCTAssertTrue(
      OverlayPresencePolicy.shouldYield(
        frontmostBundleId: "com.apple.SecurityAgent",
        modalWindowPresent: false
      )
    )
    XCTAssertFalse(
      OverlayPresencePolicy.shouldYield(
        frontmostBundleId: "com.apple.Terminal",
        modalWindowPresent: false
      )
    )
    XCTAssertTrue(
      OverlayPresencePolicy.shouldYield(
        frontmostBundleId: "com.apple.Terminal",
        modalWindowPresent: true
      )
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

  func testNoMeasuredLevelRemainsExplicitAndDoesNotClaimAudioEvidence() {
    let state = OverlayState()
    state.handleRecordingPreparing()
    state.handleRecordingStarted()

    XCTAssertNil(state.levelMeter.gain)
    XCTAssertFalse(state.hasMeasuredAudioLevel)
    XCTAssertEqual(state.statusText, "listening")
    XCTAssertEqual(state.audioLevelAccessibilityValue, "Waiting for measured level")
  }

  func testMeasuredAudioLevelHasAnHonestAccessibilityValue() {
    let state = OverlayState()
    state.handleRecordingPreparing()
    state.handleRecordingStarted()

    state.applyAudioLevel(0.0001)
    XCTAssertEqual(state.audioLevelAccessibilityValue, "Very quiet")

    state.applyAudioLevel(0.8)
    state.applyAudioLevel(0.8)
    XCTAssertEqual(state.audioLevelAccessibilityValue, "Strong level")
  }

  func testSuccessfulDictationSignalFiresOnceAndNeverForNoSpeech() {
    let successful = OverlayState()
    var successfulSignals = 0
    successful.onSuccessfulDictation = { successfulSignals += 1 }
    successful.handleRecordingPreparing()
    successful.handleRecordingStarted()
    projectText("activation without payload", to: successful, terminal: true)
    successful.finishControllerRecording()
    successful.finishControllerRecording()
    XCTAssertEqual(successfulSignals, 1)

    let silent = OverlayState()
    var silentSignals = 0
    silent.onSuccessfulDictation = { silentSignals += 1 }
    silent.handleRecordingPreparing()
    silent.handleRecordingStarted()
    silent.applyNoSpeech(reason: "no_speech_detected")
    projectText("", to: silent, phase: "no_speech", terminal: true)
    silent.finishControllerRecording()
    XCTAssertEqual(silentSignals, 0)
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
    XCTAssertEqual(state.statusText, "listening")

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
    XCTAssertEqual(state.statusText, "listening")
  }

  func testControllerStopDoesNotInventAProjectionPhase() {
    let state = OverlayState()
    var stoppedCallbacks = 0
    state.onRecordingStopped = { stoppedCallbacks += 1 }

    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    state.applyAudioLevel(0.7)
    state.applyVad(true)
    state.handleRecordingFinalising()
    state.finishControllerRecording()

    XCTAssertEqual(state.mode, .listening)
    XCTAssertEqual(state.statusText, "listening")
    XCTAssertNil(state.errorMessage)
    XCTAssertFalse(state.warmingUp)
    XCTAssertFalse(state.transcribing)
    XCTAssertFalse(state.audioReady)
    XCTAssertFalse(state.vadActive)
    XCTAssertFalse(state.isFinalPass)
    XCTAssertFalse(state.hasMeasuredAudioLevel)
    XCTAssertNil(state.levelMeter.gain)
    XCTAssertEqual(stoppedCallbacks, 1)

    state.finishControllerRecording()
    XCTAssertEqual(stoppedCallbacks, 1, "terminal recovery must be idempotent")
  }

  func testNoSpeechSidebandDoesNotReplaceProjectedPhase() {
    let state = OverlayState()
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    state.applyNoSpeech(reason: "no_speech_detected")

    state.finishControllerRecording()
    XCTAssertEqual(state.mode, .listening)

    projectText("", to: state, phase: "no_speech", terminal: true)

    XCTAssertEqual(state.mode, .noSpeech)
    XCTAssertEqual(state.statusText, "no speech")
    XCTAssertEqual(state.noSpeechNotice, OverlayState.defaultNoSpeechNotice)
  }

  func testProjectionOwnsFinalizingAndFormattedPhases() {
    let state = OverlayState()
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectText("captured text", to: state)

    state.handleRecordingFinalising()
    XCTAssertEqual(state.statusText, "listening")

    projectText("captured text", to: state, phase: "finalizing")
    XCTAssertEqual(state.statusText, "finalizing")

    state.applySessionFinalised()
    XCTAssertEqual(state.mode, .finalizing)
    XCTAssertEqual(state.statusText, "finalizing")

    projectText("captured text", to: state, terminal: true)
    state.finishControllerRecording()
    XCTAssertEqual(state.mode, .formatted)
    XCTAssertEqual(state.statusText, "formatted")
    XCTAssertEqual(state.formattedText, "captured text")
  }

  func testFailurePhaseIsExplicit() {
    let state = OverlayState()

    state.handleError(message: "engine unavailable")
    XCTAssertEqual(state.mode, .listening)

    projectText("", to: state, phase: "error", terminal: true)

    XCTAssertEqual(state.mode, .error)
    XCTAssertEqual(state.statusText, "error")
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
    let pasteboard = NSPasteboard(
      name: NSPasteboard.Name("codescribe.tests.overlay.\(UUID().uuidString)")
    )
    var closeCount = 0
    state.onClose = { closeCount += 1 }

    clock.now = 4
    state.copyToPasteboard(pasteboard)
    XCTAssertEqual(closeCount, 0)
    XCTAssertEqual(pasteboard.string(forType: .string), "ready transcript")

    clock.now = 5
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closeCount, 0)
    clock.now = 9
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closeCount, 1)
  }

  func testPasteUsesLatestProjectedTextKeepsOverlayVisibleAndRearmsAutoHide() async {
    let clock = OverlayStateTestClock()
    let state = makeFinalizedState(clock: clock, text: "original delivered transcript here")
    let engine = OverlayStateTestEngine()
    let pasteCalled = expectation(description: "paste called")
    engine.onPaste = { pasteCalled.fulfill() }
    var closeCount = 0
    state.engine = engine
    state.onClose = { closeCount += 1 }
    state.insertCaretInCodescribeProbe = { false }
    projectText("newest projected transcript", to: state, terminal: true)

    clock.now = 4
    state.pasteToPreviousApp()
    await fulfillment(of: [pasteCalled], timeout: 1)
    await Task.yield()

    XCTAssertEqual(engine.pastedText, "newest projected transcript")
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
    XCTAssertEqual(state.toast, "⌘⌥V")
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

    XCTAssertEqual(state.toast, "copied")
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
    XCTAssertEqual(state.toast, "copied")
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
    XCTAssertEqual(state.toast, "no ax")
  }

  func testRailCopyRelaysToControllerWithoutOptimisticProjectionMutation() async {
    let state = OverlayState()
    let engine = OverlayStateTestEngine()
    state.engine = engine
    projectText(
      "projected transcript",
      to: state,
      phase: "formatted",
      canPaste: true,
      canInsert: true,
      canCopy: true,
      canRetranscribe: true,
      canFormat: true,
      terminal: true
    )
    let copied = expectation(description: "copy intent reached controller")
    engine.onCopyTagged = { copied.fulfill() }

    state.relayIntent(.copy)
    await fulfillment(of: [copied], timeout: 1)

    XCTAssertEqual(engine.copiedTaggedText, "projected transcript")
    XCTAssertEqual(state.mode, .formatted)
    XCTAssertEqual(state.formattedText, "projected transcript")
    XCTAssertEqual(state.revision, 1)
    XCTAssertTrue(state.canPaste)
    XCTAssertTrue(state.canInsert)
    XCTAssertTrue(state.canCopy)
    XCTAssertTrue(state.canRetranscribe)
    XCTAssertTrue(state.canFormat)
    XCTAssertTrue(state.terminal)
    XCTAssertNil(state.toast)
  }

  func testRailInsertRelaysToControllerWithoutOptimisticProjectionMutation() async {
    let state = OverlayState()
    let engine = OverlayStateTestEngine()
    engine.pasteOutcome = .accessibilityPermissionNeeded
    state.engine = engine
    state.insertCaretInCodescribeProbe = { false }
    projectText(
      "projected transcript",
      to: state,
      phase: "formatted",
      canPaste: true,
      canInsert: true,
      canCopy: true,
      canRetranscribe: true,
      canFormat: true,
      terminal: true
    )
    let pasted = expectation(description: "insert intent reached controller")
    engine.onPaste = { pasted.fulfill() }

    state.relayIntent(.insertPaste)
    await fulfillment(of: [pasted], timeout: 1)

    XCTAssertEqual(engine.pastedText, "projected transcript")
    XCTAssertEqual(state.mode, .formatted)
    XCTAssertEqual(state.formattedText, "projected transcript")
    XCTAssertEqual(state.revision, 1)
    XCTAssertTrue(state.canPaste)
    XCTAssertTrue(state.canInsert)
    XCTAssertTrue(state.canCopy)
    XCTAssertTrue(state.canRetranscribe)
    XCTAssertTrue(state.canFormat)
    XCTAssertTrue(state.terminal)
    XCTAssertNil(state.toast)
    XCTAssertNil(state.errorMessage)
  }

  func testCloseIsImmediateAndAgentButtonUsesControllerDelivery() async {
    let clock = OverlayStateTestClock()
    let engine = OverlayStateTestEngine()
    let state = OverlayState(nowProvider: { clock.now })
    state.engine = engine
    state.applyIndicatorMode(.assistive)
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectText("ready transcript", to: state, terminal: true)
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
    XCTAssertEqual(closeCount, 2, "explicit close intent stays immediate")
  }

  func testUntouchedAgentFinalAutoSendsAtDeadline() async {
    let clock = OverlayStateTestClock()
    let engine = OverlayStateTestEngine()
    let state = OverlayState(nowProvider: { clock.now })
    state.engine = engine
    state.applyIndicatorMode(.assistive)
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectText("untouched final", to: state, terminal: true)
    state.finishControllerRecording()
    let delivered = expectation(description: "untouched final delivered")
    engine.onAssistiveSend = { delivered.fulfill() }

    clock.now = 5
    state.fireAutoHideNowForTests()
    await fulfillment(of: [delivered], timeout: 1)
    XCTAssertEqual(engine.sentAssistiveTexts, ["untouched final"])
  }

  func testRustRenderedContextMarkerPassesThroughUnchanged() {
    let state = OverlayState()
    state.applyIndicatorMode(.assistive)
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectText("alpha {selection_1} beta", to: state)
    XCTAssertEqual(state.liveText, "alpha {selection_1} beta")

    projectText("alpha {selection_1} beta", to: state, terminal: true)
    state.finishControllerRecording()
    XCTAssertEqual(state.formattedText, "alpha {selection_1} beta")
    XCTAssertEqual(state.activeText, "alpha {selection_1} beta")
  }

  func testLatestAgentProjectionAutoSendsAtDeadline() async {
    let clock = OverlayStateTestClock()
    let engine = OverlayStateTestEngine()
    let state = OverlayState(nowProvider: { clock.now })
    state.engine = engine
    state.applyIndicatorMode(.assistive)
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectText("original final", to: state, terminal: true)
    state.finishControllerRecording()
    projectText("newest projected final", to: state, terminal: true)

    let delivered = expectation(description: "latest projected final delivered")
    engine.onAssistiveSend = { delivered.fulfill() }
    clock.now = 5
    state.fireAutoHideNowForTests()
    await fulfillment(of: [delivered], timeout: 1)
    XCTAssertEqual(engine.sentAssistiveTexts, ["newest projected final"])
  }

  func testNoSpeechAutoHidesAfterFiveSeconds() {
    let clock = OverlayStateTestClock()
    let state = OverlayState(nowProvider: { clock.now })
    var closeCount = 0
    state.onClose = { closeCount += 1 }
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    state.applyNoSpeech(reason: "no_speech_detected")
    projectText("", to: state, phase: "no_speech", terminal: true)
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
    projectText("", to: state, phase: "error", terminal: true)
    XCTAssertEqual(state.mode, .error)
    clock.now = 5
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closeCount, 1)
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

  func testAgentModesNeverConstructOrOrderOverlayFront() {
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
      XCTAssertEqual(frontCount, 0, "\(mode) is owned by the Agent composer")
      XCTAssertFalse(controller.state.autoPasteControlAvailable)
    }
  }

  func testMidHoldAssistiveUpgradeClosesOverlayAndFlipsSemantics() {
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
    XCTAssertEqual(outCount, 1)
    XCTAssertEqual(controller.state.indicatorMode, .assistive)
    XCTAssertFalse(controller.state.autoPasteControlAvailable)
  }

  func testFormattedReviewBlocksAssistiveHideWithoutFormatInFlight() {
    var outCount = 0
    let state = OverlayState()
    projectText("review take", to: state, terminal: true)
    let controller = OverlayController(
      state: state,
      engine: nil,
      overlayEnabledProvider: { true },
      assistiveStatusProvider: { false },
      panelFactory: { _, _ in NSPanel() },
      orderPanelFront: { _ in },
      orderPanelOut: { _ in outCount += 1 }
    )
    controller.show()
    XCTAssertTrue(state.blocksAssistiveOverlayHide)

    controller.handleIndicatorModeChange(.assistive)
    XCTAssertEqual(outCount, 0)
    XCTAssertNotEqual(state.indicatorMode, .assistive)
    XCTAssertTrue(state.autoPasteControlAvailable)
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
    XCTAssertFalse(panel.canBecomeKey)
  }

  // MARK: Speech Recognition TCC error rewriting

  func testSpeechAuthNotDeterminedRewritesToActionableNotice() {
    let notice = OverlayState.speechAuthNotice(
      from: "Apple STT bridge probe failed: speech_auth_not_determined"
    )
    XCTAssertNotNil(notice)
    XCTAssertTrue(notice?.contains("Speech Recognition") == true)
    XCTAssertFalse(notice?.contains("speech_auth") == true)
  }

  func testSpeechAuthDeniedRewritesToSystemSettingsHint() {
    let notice = OverlayState.speechAuthNotice(
      from: "Couldn't start recording: speech_auth_denied: enable Speech Recognition"
    )
    XCTAssertNotNil(notice)
    XCTAssertTrue(notice?.contains("System Settings") == true)
  }

  func testNonSpeechErrorsAreNotRewritten() {
    XCTAssertNil(OverlayState.speechAuthNotice(from: "Couldn't start recording: mic busy"))
  }

  // MARK: Acoustic admission refusal rewriting

  func testAdmissionCalibrationMissingRewritesToCalibrateHint() {
    let notice = OverlayState.admissionNotice(
      from:
        "Couldn't start recording: admission_calibration_missing: no acoustic calibration measured yet (/x/energy-calibration.json) — Run Calibrate microphone in Settings › Audio (about 10 seconds of normal speech)."
    )
    XCTAssertNotNil(notice)
    XCTAssertTrue(notice?.headline.contains("Calibrate microphone") == true)
    XCTAssertTrue(notice?.detail.contains("Run Calibrate microphone") == true)
    XCTAssertFalse(notice?.headline.contains("admission_") == true)
  }

  func testAdmissionWarningEnvelopePointsSettingsOwnedRefusalToAudio() {
    let notice = OverlayState.admissionNotice(
      from:
        "admission_refused: admission_seal_lane_disarmed: seal lane is off in Settings › Audio, so no utterance can commit — Enable Seal lane in Settings › Audio."
    )
    XCTAssertNotNil(notice)
    XCTAssertTrue(notice?.headline.contains("Settings › Audio") == true)
    XCTAssertTrue(notice?.detail.contains("no utterance can commit") == true)
    XCTAssertFalse(notice?.headline.contains(".env") == true)
    XCTAssertFalse(notice?.detail.contains(".env") == true)
  }

  func testAdmissionEnvOverrideNamesTheOverrideWithoutEnvFileInstructions() {
    let notice = OverlayState.admissionNotice(
      from:
        "admission_seal_lane_disarmed: seal lane is disarmed by the CODESCRIBE_SILERO_FUSION override, so no utterance can commit — CODESCRIBE_SILERO_FUSION power-user override is off; remove the override or set it to 1."
    )
    XCTAssertTrue(notice?.headline.contains("CODESCRIBE_SILERO_FUSION override") == true)
    XCTAssertTrue(notice?.detail.contains("power-user override") == true)
    XCTAssertFalse(notice?.headline.contains(".env") == true)
    XCTAssertFalse(notice?.detail.contains(".env") == true)
  }

  func testNonAdmissionErrorsAreNotRewrittenAsAdmission() {
    XCTAssertNil(OverlayState.admissionNotice(from: "Couldn't start recording: mic busy"))
    XCTAssertNil(OverlayState.admissionNotice(from: "speech_auth_denied"))
  }

  func testHandleErrorSurfacesFriendlySpeechAuthToast() {
    let state = OverlayState()
    state.handleError(
      message: "Apple STT bridge probe failed: speech_auth_not_determined (no Whisper fallback)")
    XCTAssertEqual(state.mode, .listening)
    XCTAssertTrue(state.errorMessage?.contains("Speech Recognition") == true)
    XCTAssertFalse(state.toast?.contains("speech_auth") == true)
    XCTAssertEqual(state.recoverySettingsSection, .creator)
  }

  func testAdmissionErrorRoutesRecoveryToAudioSettingsAndResetClearsIt() {
    let state = OverlayState()
    state.handleError(
      message:
        "admission_calibration_missing: no acoustic calibration measured yet — Run Calibrate microphone in Settings › Audio."
    )

    XCTAssertEqual(state.mode, .listening)
    XCTAssertEqual(state.recoverySettingsSection, .audio)
    XCTAssertEqual(state.recoverySettingsAnchor, .audioReadiness)
    XCTAssertEqual(state.errorLifecycleDetail, "Recording did not start.")

    state.handleRecordingPreparing()
    XCTAssertNil(state.recoverySettingsSection)
    XCTAssertNil(state.recoverySettingsAnchor)
  }

  func testRecoveryRoutingUsesTheClosestSettingsOwner() {
    XCTAssertEqual(
      OverlayState.recoverySettingsSection(
        from: "admission_seal_vad_unavailable: Silero VAD failed to load"
      ),
      .engine
    )
    XCTAssertEqual(
      OverlayState.recoverySettingsSection(
        from: "admission_capture_device_unavailable: no input device"
      ),
      .audio
    )
    XCTAssertEqual(
      OverlayState.recoverySettingsSection(
        from: "speech_auth_denied: Apple speech recognition is off"
      ),
      .creator
    )
    XCTAssertEqual(
      OverlayState.recoverySettingsSection(from: "Microphone access denied"),
      .audio
    )
    XCTAssertEqual(
      OverlayState.recoverySettingsSection(
        from: "admission_refused: admission_calibration_unusable: stale profile"
      ),
      .audio
    )
    XCTAssertEqual(
      OverlayState.recoverySettingsAnchor(
        from: "admission_refused: admission_calibration_unusable: stale profile"
      ),
      .audioReadiness
    )
    XCTAssertEqual(
      OverlayState.recoverySettingsAnchor(
        from: "admission_capture_device_unavailable: no input device"
      ),
      .audioInput
    )
    XCTAssertEqual(
      OverlayState.recoverySettingsSection(from: "transcription_failed: model unavailable"),
      .engine
    )
    XCTAssertEqual(
      OverlayState.recoverySettingsSection(from: "STT model could not load"),
      .engine
    )
  }

  /// Born from the 2026-08-12 Founder report: a routine
  /// `apple_final_window_overlap_normalized` warning reached `handleError`,
  /// matched none of the three literal phrases the old guard looked for, and ran
  /// `presentTerminalError` — discarding two utterances the engine log had
  /// already recorded as committed (`rendered_chars=282`). The screen said
  /// "failed" while the transcript existed.
  ///
  /// Since the bridge-side warning split, quality receipts no longer travel
  /// here at all — but the content rule this test pins is unconditional:
  /// NOTHING arriving on this channel may discard a non-empty draft.
  func testEngineWarningNeverDiscardsCommittedTranscript() {
    let state = OverlayState()
    projectText("zdanie pierwsze zdanie drugie", to: state)

    state.handleError(
      message:
        "apple_final_window_overlap_normalized: Apple final overlap removed at segment boundary")

    XCTAssertNotEqual(state.mode, .error, "an error with a draft must not discard the take")
    XCTAssertEqual(state.activeText, "zdanie pierwsze zdanie drugie")
    XCTAssertEqual(state.liveText, "zdanie pierwsze zdanie drugie")
  }

  func testTerminalFailureSidebandEndsCaptureWithoutRewritingProjection() {
    let state = OverlayState()
    var stopped = false
    state.onRecordingStopped = { stopped = true }
    state.handleRecordingStarted()
    projectText("zdanie pierwsze", to: state)

    state.handleError(message: "transcription_failed: engine gave up mid-take")

    XCTAssertEqual(state.mode, .listening, "sideband errors do not invent a projection phase")
    XCTAssertEqual(state.statusText, "listening")
    XCTAssertTrue(stopped, "stop parity must fire — no zombie Recording pill")
    XCTAssertEqual(state.activeText, "zdanie pierwsze")
    XCTAssertEqual(state.toast, "Dictation failed — transcript kept")
  }

  func testEngineErrorSidebandPreservesProjectedPhaseOnEmptyTake() {
    let state = OverlayState()
    state.handleError(message: "layer1_lane_degraded: Layer 1 lane fell back")
    XCTAssertEqual(state.mode, .listening)
    XCTAssertEqual(state.errorMessage, "layer1_lane_degraded: Layer 1 lane fell back")
  }

  @MainActor
  func testFormattedOverlayMinimumHeightSnapshotRenders() throws {
    let state = OverlayState()
    let longTranscript = Array(
      repeating:
        "Choose Insert to paste the text where you want it and press Return. The clipboard is untouched.",
      count: 20
    ).joined(separator: "\n")
    projectText(longTranscript, to: state, terminal: true)
    let size = CGSize(
      width: 617,
      height: DictationOverlayWindow.minSize.height
    )
    let hostingView = NSHostingView(
      rootView: DictationOverlayView(state: state)
        .environment(\.csTextScale, 0.8)
        .frame(width: size.width, height: size.height)
        .preferredColorScheme(.dark)
    )
    hostingView.frame = CGRect(origin: .zero, size: size)
    hostingView.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.03))
    guard let bitmap = hostingView.bitmapImageRepForCachingDisplay(in: hostingView.bounds) else {
      return XCTFail("could not allocate the formatted overlay bitmap")
    }
    hostingView.cacheDisplay(in: hostingView.bounds, to: bitmap)
    guard let png = bitmap.representation(using: .png, properties: [:]) else {
      XCTFail("could not render the formatted overlay")
      return
    }
    let dest = FileManager.default.temporaryDirectory
      .appendingPathComponent("codescribe-formatted-overlay-min-height.png")
    try png.write(to: dest)
    XCTAssertGreaterThan(png.count, 800)

    // Slim chrome: the bottom action layer is gone. Measure the empty center of
    // the footer — excluding its truthful engine label on the left and the
    // developer-power mark on the right. Bright glyphs in this corridor mean
    // the transcript escaped its clipped body. A small allowance covers
    // antialiased footer/border pixels; a real escape produces hundreds.
    var leakedBrightPixels = 0
    for x in 140..<500 {
      for y in 6..<28 {
        guard let color = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.deviceRGB) else {
          continue
        }
        if color.redComponent > 0.7 && color.greenComponent > 0.7
          && color.blueComponent > 0.7 && color.alphaComponent > 0.5
        {
          leakedBrightPixels += 1
        }
      }
    }
    XCTAssertLessThan(
      leakedBrightPixels, 20,
      "formatted transcript painted into the footer band"
    )
  }

  func testProjectionFixturesMirrorEveryCanvasField() {
    let state = OverlayState()
    let rows:
      [(
        phase: String, text: String, mode: String, paste: Bool, insert: Bool,
        copy: Bool, retranscribe: Bool, format: Bool, terminal: Bool
      )] = [
        ("listening", "  exact live\ntext  ", "dictation", false, false, true, false, true, false),
        ("finalizing", "final pass", "assistive", false, false, true, false, false, false),
        ("formatted", "final text", "dictation", true, true, true, true, true, true),
        ("no_speech", "", "dictation", false, false, false, true, false, true),
        ("error", "kept draft", "dictation", false, false, true, true, false, true),
      ]

    for (index, row) in rows.enumerated() {
      projectText(
        row.text,
        to: state,
        mode: row.mode,
        phase: row.phase,
        canPaste: row.paste,
        canInsert: row.insert,
        canCopy: row.copy,
        canRetranscribe: row.retranscribe,
        canFormat: row.format,
        terminal: row.terminal
      )

      XCTAssertEqual(state.mode.rawValue, row.phase)
      XCTAssertEqual(state.formattedText, row.text)
      XCTAssertEqual(state.activeText, row.text)
      XCTAssertEqual(state.transcriptMode, row.mode)
      XCTAssertEqual(state.revision, UInt64(index + 1))
      XCTAssertEqual(state.canPaste, row.paste)
      XCTAssertEqual(state.canInsert, row.insert)
      XCTAssertEqual(state.canCopy, row.copy)
      XCTAssertEqual(state.canRetranscribe, row.retranscribe)
      XCTAssertEqual(state.canFormat, row.format)
      XCTAssertEqual(state.terminal, row.terminal)
    }
  }

  func testCanvasSourceHasNoDeliveryOrPlacementChrome() throws {
    let macosDir = URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent()
      .deletingLastPathComponent()
    let overlayDir = macosDir.appendingPathComponent("Codescribe/Screens/Overlay")
    let overlaySource = try String(
      contentsOf: overlayDir.appendingPathComponent("DictationOverlayView.swift"),
      encoding: .utf8
    )
    let splitPath = overlayDir.appendingPathComponent("OverlaySplitPrimaryAction.swift").path

    XCTAssertFalse(FileManager.default.fileExists(atPath: splitPath))
    XCTAssertFalse(overlaySource.contains("Menu {"))
    XCTAssertFalse(overlaySource.contains("overlay-auto-paste"))
    XCTAssertFalse(overlaySource.contains("overlay-placement-menu"))
    XCTAssertFalse(overlaySource.contains("performPrimaryAction"))
    XCTAssertTrue(overlaySource.contains("OverlayIntentRail"))
    XCTAssertFalse(
      overlaySource.contains("CloseDot"),
      "the rail is the sole overlay control surface"
    )
    XCTAssertTrue(
      overlaySource.contains("chromeWaveform"),
      "waveform stays in the primary chrome"
    )
    XCTAssertFalse(
      overlaySource.contains("private var actionRow"),
      "do not restore the fat bottom Finish/Close row"
    )
  }

  @MainActor
  func testSlimOverlayListeningChromeRendersWithoutBottomActionMass() throws {
    let state = OverlayState.previewListening()
    let size = CGSize(
      width: DictationOverlayWindow.minSize.width,
      height: DictationOverlayWindow.minSize.height
    )
    let hostingView = NSHostingView(
      rootView: DictationOverlayView(state: state)
        .frame(width: size.width, height: size.height)
        .preferredColorScheme(.dark)
    )
    hostingView.frame = CGRect(origin: .zero, size: size)
    hostingView.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.03))
    guard let bitmap = hostingView.bitmapImageRepForCachingDisplay(in: hostingView.bounds) else {
      return XCTFail("could not allocate the slim listening overlay bitmap")
    }
    hostingView.cacheDisplay(in: hostingView.bounds, to: bitmap)
    guard let png = bitmap.representation(using: .png, properties: [:]) else {
      XCTFail("could not render the slim listening overlay")
      return
    }
    let dest = FileManager.default.temporaryDirectory
      .appendingPathComponent("codescribe-slim-listening-overlay-min-width.png")
    try png.write(to: dest)
    XCTAssertGreaterThan(png.count, 800)
    XCTAssertEqual(DictationOverlayWindow.minSize.width, 320)
    XCTAssertEqual(DictationOverlayWindow.minSize.height, 260)
  }

  /// Project one event for an explicitly named session, so a test can replay
  /// two Bus sessions whose sequences both restart at 1 — the production shape
  /// `projectText` cannot express because it pins a single session id.
  private func projectSessionText(
    _ text: String,
    sessionId: String,
    sequence: UInt64,
    to state: OverlayState,
    terminal: Bool = false
  ) {
    let sampleStart = (sequence - 1) * 16_000
    let sampleEnd = sequence * 16_000
    let receipt = CsProjectedAcousticReceipt(
      acousticSerialVersion: 1,
      acousticSerial: "\(sessionId)-acoustic-\(sequence)",
      sessionId: sessionId,
      captureEpoch: 1,
      sampleStart: sampleStart,
      sampleEnd: sampleEnd,
      durationMs: 1_000,
      energyIntegral: 1,
      meanRmsDbfs: -20,
      peakDbfs: -6,
      vadOpenSample: sampleStart,
      vadCloseSample: sampleEnd,
      evidenceCalibrationVersion: "test-v1",
      wordEvidenceReceipts: ["\(sessionId)-word-\(sequence)"],
      layerDecisionReceipts: ["\(sessionId)-layer-\(sequence)"],
      sealReceipt: terminal ? "\(sessionId)-seal-\(sequence)" : nil,
      manualEditReceipt: nil
    )
    state.applyTranscriptProjection(
      CsTranscriptProjectionEvent(
        schema: "codescribe.transcript_projection.v1",
        sequence: sequence,
        emittedAt: "2026-08-28T00:00:00Z",
        sessionId: sessionId,
        mode: "dictation",
        reducerRevision: sequence,
        reducerAction: terminal
          ? "record_ledger_terminal_seal"
          : "record_ledger_projection",
        occurrenceSessionId: sessionId,
        captureEpoch: 1,
        sampleStart: sampleStart,
        sampleEnd: sampleEnd,
        documentIndex: sequence - 1,
        label: terminal ? "terminal" : "live",
        renderedText: text,
        phase: terminal ? "formatted" : "listening",
        canPaste: terminal,
        canInsert: terminal,
        canCopy: !text.isEmpty,
        canRetranscribe: terminal,
        canFormat: !terminal,
        terminal: terminal,
        acousticReceipts: [receipt]
      )
    )
  }

  /// Regression, div0 2026-08-28: three takes in one app process, sessions
  /// `5567c17a` → `f15f4ad3`. Rust sealed both correctly, but the overlay kept
  /// painting the first take's text over the second and then raised
  /// "Recording ended before a sealed transcript was committed". Bus sequences
  /// restart at 1 per session, so a terminal-seal latch and a monotonic
  /// sequence guard held across sessions drop every projection of take 2.
  func testSecondSessionProjectionsAreNotLatchedByTheFirstSessionSeal() {
    let clock = OverlayStateTestClock()
    let state = OverlayState(nowProvider: { clock.now })

    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectSessionText(
      "Panie agenci masz brzydkie pięty",
      sessionId: "5567c17a",
      sequence: 42,
      to: state,
      terminal: true
    )
    XCTAssertEqual(state.formattedText, "Panie agenci masz brzydkie pięty")

    // Take 2 in the same process: a fresh session whose sequences start at 1.
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectSessionText(
      "Jestem naprawdę na skraju",
      sessionId: "f15f4ad3",
      sequence: 1,
      to: state
    )
    projectSessionText(
      "Jestem naprawdę na skraju wytrzymałości",
      sessionId: "f15f4ad3",
      sequence: 2,
      to: state,
      terminal: true
    )

    XCTAssertEqual(state.formattedText, "Jestem naprawdę na skraju wytrzymałości")
    XCTAssertEqual(state.mode, .formatted)
    state.finishControllerRecording()
    XCTAssertNil(state.errorMessage)
  }

  /// Events that reach the projection listener are already reducer-owned. Swift
  /// paints their arrival order without rebuilding sequence or seal policy.
  func testWithinOneSessionSwiftDoesNotRebuildReducerGuards() {
    let clock = OverlayStateTestClock()
    let state = OverlayState(nowProvider: { clock.now })
    state.handleRecordingPreparing()
    state.handleRecordingStarted()

    projectSessionText("pierwsza", sessionId: "same-session", sequence: 5, to: state)
    XCTAssertEqual(state.formattedText, "pierwsza")

    projectSessionText("spóźniona", sessionId: "same-session", sequence: 3, to: state)
    XCTAssertEqual(state.formattedText, "spóźniona")

    projectSessionText(
      "zapieczętowana",
      sessionId: "same-session",
      sequence: 6,
      to: state,
      terminal: true
    )
    XCTAssertEqual(state.formattedText, "zapieczętowana")

    projectSessionText("po pieczęci", sessionId: "same-session", sequence: 7, to: state)
    XCTAssertEqual(state.formattedText, "po pieczęci")
    XCTAssertEqual(state.mode, .listening)
    XCTAssertFalse(state.terminal)
  }

  func testRecordingLifecycleDoesNotClearTheLastProjection() {
    let state = OverlayState()

    state.handleRecordingStarted()
    projectSessionText(
      "tekst poprzedniego nagrania",
      sessionId: "previous-take",
      sequence: 9,
      to: state,
      terminal: true
    )
    state.finishControllerRecording()
    XCTAssertEqual(state.liveText, "tekst poprzedniego nagrania")

    state.handleRecordingStarted()

    XCTAssertEqual(state.mode, .formatted)
    XCTAssertEqual(state.liveText, "tekst poprzedniego nagrania")
    XCTAssertEqual(state.formattedText, "tekst poprzedniego nagrania")

    projectSessionText(
      "tekst nowego nagrania",
      sessionId: "new-take",
      sequence: 1,
      to: state
    )
    XCTAssertEqual(state.mode, .listening)
    XCTAssertEqual(state.formattedText, "tekst nowego nagrania")
  }
}
