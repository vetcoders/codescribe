import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

// Executed by `make test-swift`. The marker-rebase assertions below run for
// real; see CodescribeTests/README.md.

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

  /// Admit Rust-owned transcript truth through the same projection boundary as
  /// production. Tests may choose rendered documents; they do not replay the
  /// demolished Swift preview/final/patch reducer.
  private func projectText(
    _ text: String,
    to state: OverlayState,
    terminal: Bool = false,
    includesWordEvidence: Bool = true
  ) {
    nextProjectionSequence += 1
    let sequence = nextProjectionSequence
    let sampleStart = (sequence - 1) * 16_000
    let sampleEnd = sequence * 16_000
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
        mode: "dictation",
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
    XCTAssertTrue(
      String(state.listeningCanvas.characters).contains(
        "analyze the repo for duplicate dispatch"
      )
    )
  }

  func testProjectionWithoutAcousticEvidenceCannotClaimTheCanvas() {
    let state = OverlayState()
    state.handleRecordingPreparing()
    state.handleRecordingStarted()

    projectText("unproven shadow", to: state, includesWordEvidence: false)

    XCTAssertFalse(state.canCopy)
    XCTAssertTrue(state.activeText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
    XCTAssertEqual(state.mode, .listening)
  }

  func testTerminalSealRejectsEveryLaterMachineProjection() {
    let state = OverlayState()
    var successfulSignals = 0
    state.onSuccessfulDictation = { successfulSignals += 1 }
    state.handleRecordingPreparing()
    state.handleRecordingStarted()

    projectText("sealed document", to: state, terminal: true)
    projectText("late competing document", to: state)

    XCTAssertEqual(state.activeText, "sealed document")
    XCTAssertEqual(state.formattedText, "sealed document")
    XCTAssertEqual(state.mode, .formatted)
    XCTAssertEqual(successfulSignals, 1)
  }

  func testTranscriptEditIsOptInUntilClick() {
    let state = OverlayState()
    state.mode = .formatted
    state.formattedText = "hello"
    XCTAssertFalse(state.isEditingTranscript)
    state.beginTranscriptEdit()
    XCTAssertTrue(state.isEditingTranscript)
    state.endTranscriptEdit()
    XCTAssertFalse(state.isEditingTranscript)
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

  func testApprovedOverlayActionPresentationIsLiteral() {
    XCTAssertEqual(OverlayActionPresentation.sendTitle, "To Agent")
    XCTAssertEqual(OverlayActionPresentation.sendHelp, "Send transcript to the agent")
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

  func testNoMeasuredLevelRemainsExplicitAndDoesNotClaimAudioEvidence() {
    let state = OverlayState()
    state.handleRecordingPreparing()
    state.handleRecordingStarted()

    XCTAssertNil(state.levelMeter.gain)
    XCTAssertFalse(state.hasMeasuredAudioLevel)
    XCTAssertEqual(state.statusText, "recording · level pending")
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
    XCTAssertEqual(state.statusText, "recording · level pending")
  }

  func testControllerStopWithoutTerminalProjectionCannotLeaveZombieRecordingUI() {
    let state = OverlayState()
    var stoppedCallbacks = 0
    state.onRecordingStopped = { stoppedCallbacks += 1 }

    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    state.applyAudioLevel(0.7)
    state.applyVad(true)
    state.handleRecordingFinalising()
    state.finishControllerRecording()

    XCTAssertEqual(state.mode, .error)
    XCTAssertEqual(state.statusText, "failed")
    XCTAssertEqual(state.tagText, "ERROR")
    XCTAssertEqual(
      state.errorMessage,
      "Recording ended before a sealed transcript was committed"
    )
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

  func testControllerStopUsesExplicitNoSpeechOutcomeWhenEngineProvidedIt() {
    let state = OverlayState()
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    state.applyNoSpeech(reason: "no_speech_detected")

    state.finishControllerRecording()

    XCTAssertEqual(state.mode, .noSpeech)
    XCTAssertEqual(state.statusText, "no speech")
    XCTAssertEqual(state.noSpeechNotice, OverlayState.defaultNoSpeechNotice)
  }

  /// Product honesty: badge must not say "live preview · raw" while the body
  /// is only the empty-canvas placeholder ("listening…"). Claim live preview
  /// only after an admitted projection puts text on the canvas.
  func testMetaTextClaimsLivePreviewOnlyWhenCanvasHasText() {
    let state = OverlayState()
    state.handleRecordingPreparing()
    state.handleRecordingStarted()

    XCTAssertEqual(state.metaText, "live preview · waiting")
    XCTAssertTrue(state.liveText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
    XCTAssertTrue(
      state.footerRight.contains("waiting"),
      "empty canvas footer must not claim vad-gated preview: \(state.footerRight)"
    )

    projectText("apple partial", to: state)
    XCTAssertEqual(state.metaText, "live preview · raw")
    XCTAssertEqual(state.liveText, "apple partial")

    projectText("apple partial sealed", to: state)
    // Canvas still reflects the latest admitted Rust projection.
    XCTAssertEqual(state.metaText, "live preview · raw")
    XCTAssertEqual(state.liveText, "apple partial sealed")
  }

  func testSessionFinalisedStartsFinalPassUntilControllerStops() {
    let state = OverlayState()
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectText("captured text", to: state)

    state.handleRecordingFinalising()
    XCTAssertEqual(state.statusText, "transcribing")

    state.applySessionFinalised()
    XCTAssertEqual(state.mode, .listening)
    XCTAssertEqual(state.statusText, "final pass")

    projectText("captured text", to: state, terminal: true)
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

  func testManualEditProvenanceIsConsumedOnceAndRearmsOnlyOnAnotherEdit() {
    let state = makeFinalizedState(clock: OverlayStateTestClock())
    state.userEditedTranscript("first human correction")

    XCTAssertEqual(
      state.consumeManualEditProvenanceForQuality(isEdited: true),
      "manual_human"
    )
    XCTAssertNil(state.consumeManualEditProvenanceForQuality(isEdited: true))

    state.userEditedTranscript("second human correction")
    XCTAssertEqual(
      state.consumeManualEditProvenanceForQuality(isEdited: true),
      "manual_human"
    )
    XCTAssertNil(state.consumeManualEditProvenanceForQuality(isEdited: false))

  }

  func testCanonicalProjectionClearsManualEditProvenance() {
    let state = OverlayState()
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectText("streaming projection", to: state)
    state.mode = .formatted
    state.userEditedTranscript("manual text before authoritative projection")

    projectText("authoritative product seal", to: state, terminal: true)

    XCTAssertNil(
      state.consumeManualEditProvenanceForQuality(isEdited: true),
      "an admitted Rust projection is machine provenance"
    )
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
    projectText("untouched final", to: state, terminal: true)
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
    projectText("alpha beta", to: state)

    state.applyContextMarker(position: 5, marker: "{selection_1}")
    XCTAssertEqual(state.liveText, "alpha {selection_1} beta")

    projectText("alpha beta", to: state, terminal: true)
    state.finishControllerRecording()
    XCTAssertEqual(state.formattedText, "alpha beta")
    XCTAssertEqual(state.activeText, "alpha {selection_1} beta")
  }

  func testContextMarkerInsideWordStaysUnpaddedForLosslessTitles() {
    let state = OverlayState()
    state.applyIndicatorMode(.assistive)
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectText("bardzo mnie drażni", to: state)

    // Position 9 splits "mnie" between "mn" and "ie": no padding, so the
    // split stays lossless for downstream title derivation.
    state.applyContextMarker(position: 9, marker: "{selection_1}")
    XCTAssertEqual(state.liveText, "bardzo mn{selection_1}ie drażni")
  }

  func testContextMarkersAtSamePositionKeepCaptureOrder() {
    let state = OverlayState()
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    projectText("alpha", to: state)

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
    projectText("original final", to: state, terminal: true)
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
    state.applyNoSpeech(reason: "no_speech_detected")
    projectText("", to: state, terminal: true)
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
    state.mode = .formatted
    state.formattedText = "review take"
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
    XCTAssertEqual(state.mode, .error)
    XCTAssertTrue(state.errorMessage?.contains("Speech Recognition") == true)
    XCTAssertFalse(state.toast?.contains("speech_auth") == true)
    XCTAssertNil(state.recoverySettingsSection)
  }

  func testAdmissionErrorRoutesRecoveryToAudioSettingsAndResetClearsIt() {
    let state = OverlayState()
    state.handleError(
      message:
        "admission_calibration_missing: no acoustic calibration measured yet — Run Calibrate microphone in Settings › Audio."
    )

    XCTAssertEqual(state.mode, .error)
    XCTAssertEqual(state.recoverySettingsSection, .audio)

    state.handleRecordingPreparing()
    XCTAssertNil(state.recoverySettingsSection)
  }

  /// Born from the 2026-08-12 operator report: a routine
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

  /// Born from the PR #73 review (2026-08-13): after the bridge-side warning
  /// split, everything reaching `on_error` is a user-terminal failure — yet a
  /// failure arriving with a non-empty draft was still labelled
  /// "Engine warning" and returned early, leaving the overlay in a zombie
  /// live-capture UI (no stop parity, tray stuck on Recording, engine possibly
  /// still holding the mic). A terminal failure with a draft must END the
  /// session like a stop — finalized, stop callback fired, honest toast —
  /// while the transcript stays on the normal terminal surface.
  func testTerminalFailureWithDraftEndsSessionButKeepsTranscript() {
    let state = OverlayState()
    var stopped = false
    state.onRecordingStopped = { stopped = true }
    state.handleRecordingStarted()
    projectText("zdanie pierwsze", to: state)

    state.handleError(message: "transcription_failed: engine gave up mid-take")

    XCTAssertEqual(state.mode, .formatted, "kept draft lands on the normal terminal surface")
    XCTAssertEqual(state.statusText, "done", "the failed session must actually end")
    XCTAssertTrue(stopped, "stop parity must fire — no zombie Recording pill")
    XCTAssertEqual(state.activeText, "zdanie pierwsze")
    XCTAssertEqual(state.toast, "Dictation failed — transcript kept")
  }

  /// The other half of the rule: with no transcript to protect, the warning must
  /// still be visible. Staying silent here would hide a genuine dead-on-arrival
  /// session behind a toast the user may never notice.
  func testEngineWarningOnEmptyTakeStaysTerminal() {
    let state = OverlayState()
    state.handleError(message: "layer1_lane_degraded: Layer 1 lane fell back")
    XCTAssertEqual(state.mode, .error)
  }

  func testCanvasRunsSplitLexiconWordAndKeepPreview() {
    let runs = OverlayCanvas.runs(
      segments: [(utteranceId: 1, text: "Reports i Edyta")],
      highlights: [
        OverlayCanvas.lexiconHighlight(
          utteranceId: 1, start: 0, replacement: "Reports", before: "RIPOS")!
      ],
      preview: "dalej"
    )
    XCTAssertEqual(
      runs,
      [
        .highlight(
          OverlayCanvas.lexiconHighlight(
            utteranceId: 1, start: 0, replacement: "Reports", before: "RIPOS")!),
        .text(" i Edyta dalej"),
      ])
  }

  func testHighlightScreenshotRendersLexiconAndGap() throws {
    let highlight = OverlayCanvas.lexiconHighlight(
      utteranceId: 1, start: 0, replacement: "Reports", before: "RIPOS")!
    let gap = OverlayCanvas.speechGap(utteranceId: 2)
    let view = OverlayHighlightScreenshot(
      runs: OverlayCanvas.runs(
        segments: [(utteranceId: 1, text: "Reports")],
        highlights: [highlight, gap],
        preview: ""
      ),
      highlights: [highlight, gap]
    )
    .frame(width: 520, height: 220)
    let renderer = ImageRenderer(content: view)
    renderer.scale = 2
    guard let image = renderer.nsImage else {
      XCTFail("ImageRenderer produced no nsImage")
      return
    }
    let dest = FileManager.default.temporaryDirectory
      .appendingPathComponent("w13-6b-highlights.png")
    let reportDest = URL(fileURLWithPath: NSHomeDirectory())
      .appendingPathComponent(
        ".vibecrafted/artifacts/vetcoders/codescribe/2026_0813/reports/implement/w13-6b-highlights.png"
      )
    guard let tiff = image.tiffRepresentation,
      let rep = NSBitmapImageRep(data: tiff),
      let png = rep.representation(using: .png, properties: [:])
    else {
      XCTFail("could not encode highlight screenshot")
      return
    }
    try png.write(to: dest)
    try? FileManager.default.createDirectory(
      at: reportDest.deletingLastPathComponent(), withIntermediateDirectories: true)
    try? png.write(to: reportDest)
    XCTAssertGreaterThan(png.count, 800)
    XCTAssertTrue(FileManager.default.fileExists(atPath: dest.path))
  }

  @MainActor
  func testFormattedOverlayMinimumHeightSnapshotRenders() throws {
    let state = OverlayState.previewListening()
    state.formattedText = Array(
      repeating:
        "Choose Insert to paste the text where you want it and press Return. The clipboard is untouched.",
      count: 20
    ).joined(separator: "\n")
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
    state.mode = .formatted
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
    // the native TextEditor escaped its clipped body. The previous 40..<580
    // range counted both legitimate footer labels and failed while the rendered
    // screenshot was visually correct.
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
      leakedBrightPixels, 5,
      "formatted transcript painted into the footer band"
    )
  }

  func testSlimChromePrimaryActionAndFooterHonesty() {
    let listening = OverlayState()
    listening.handleRecordingPreparing()
    XCTAssertEqual(listening.primaryActionKind, .finish)
    XCTAssertEqual(listening.primaryActionTitle, OverlayActionPresentation.finishTitle)
    XCTAssertTrue(listening.showsFooterHonesty)
    XCTAssertEqual(listening.footerHonestyText, "waiting for audio")

    listening.handleRecordingStarted()
    XCTAssertEqual(listening.footerHonestyText, "waiting for words")

    projectText("hello", to: listening)
    XCTAssertFalse(listening.showsFooterHonesty)

    let formatted = OverlayState.previewFormatted()
    XCTAssertEqual(formatted.primaryActionKind, .insert)
    XCTAssertFalse(formatted.showsFooterHonesty)

    let silent = OverlayState.previewNoSpeech()
    XCTAssertNil(silent.primaryActionKind)
  }

  /// macOS `Menu` + `primaryAction` treats the whole control as the primary, so
  /// the painted chevron never opens. The slim chrome must be a true split:
  /// a Button for Finish/Insert and a separate Menu on the chevron.
  func testCompactPrimaryActionIsSplitControlNotMenuPrimaryAction() throws {
    let macosDir = URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent()
      .deletingLastPathComponent()
    let overlayDir = macosDir.appendingPathComponent("Codescribe/Screens/Overlay")
    let overlaySource = try String(
      contentsOf: overlayDir.appendingPathComponent("DictationOverlayView.swift"),
      encoding: .utf8
    )
    let splitSource = try String(
      contentsOf: overlayDir.appendingPathComponent("OverlaySplitPrimaryAction.swift"),
      encoding: .utf8
    )

    XCTAssertFalse(
      overlaySource.contains("primaryAction:") || splitSource.contains("primaryAction:"),
      "Menu.primaryAction swallows the chevron on macOS; the split control must not use it"
    )
    XCTAssertFalse(
      overlaySource.contains("NSComboButton") || splitSource.contains("NSComboButton"),
      "HStack split is the sanctioned control; NSComboButton is the fallback not taken"
    )
    XCTAssertTrue(
      splitSource.contains("accessibilityIdentifier(\"overlay-primary-action\")"),
      "primary Finish/Insert button must keep overlay-primary-action"
    )
    XCTAssertTrue(
      splitSource.contains("accessibilityIdentifier(\"overlay-primary-action-menu\")"),
      "chevron menu must be a separate hit target"
    )
    XCTAssertTrue(
      splitSource.contains("performPrimaryAction(kind)"),
      "primary button must run the existing Finish/Insert action"
    )
    XCTAssertTrue(
      splitSource.contains("secondaryActionButtons(for: kind)"),
      "chevron menu must keep the secondary commands"
    )
    XCTAssertTrue(
      overlaySource.contains("func performPrimaryAction(_ kind: OverlayPrimaryActionKind)"),
      "seal_to_delivery hop stays one function body in DictationOverlayView.swift"
    )
    XCTAssertTrue(
      overlaySource.contains("CloseDot"),
      "CloseDot stays the always-visible dismiss control"
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
    let size = CGSize(width: 470, height: DictationOverlayWindow.minSize.height)
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
      .appendingPathComponent("codescribe-slim-listening-overlay.png")
    try png.write(to: dest)
    XCTAssertGreaterThan(png.count, 800)
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

  /// The guards still do their job inside one session: a stale, out-of-order
  /// projection and any projection after that session's terminal seal are
  /// refused. Without this, the session-scoping above would be a hole.
  func testWithinOneSessionTheSealAndSequenceGuardsStillRefuse() {
    let clock = OverlayStateTestClock()
    let state = OverlayState(nowProvider: { clock.now })
    state.handleRecordingPreparing()
    state.handleRecordingStarted()

    projectSessionText("pierwsza", sessionId: "same-session", sequence: 5, to: state)
    XCTAssertEqual(state.formattedText, "pierwsza")

    // Out of order inside the same session — refused.
    projectSessionText("spóźniona", sessionId: "same-session", sequence: 3, to: state)
    XCTAssertEqual(state.formattedText, "pierwsza")

    projectSessionText(
      "zapieczętowana",
      sessionId: "same-session",
      sequence: 6,
      to: state,
      terminal: true
    )
    XCTAssertEqual(state.formattedText, "zapieczętowana")

    // After the terminal seal of the same session — refused.
    projectSessionText("po pieczęci", sessionId: "same-session", sequence: 7, to: state)
    XCTAssertEqual(state.formattedText, "zapieczętowana")
  }

  /// A controller may enter the started phase directly, without a preceding
  /// preparing callback. The new take must clear the remembered canonical
  /// projection itself; clearing only `formattedText` still lets `liveText`
  /// paint the previous take through `latestTranscriptProjection`.
  func testRecordingStartClearsThePreviousTakeProjection() {
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

    XCTAssertEqual(state.mode, .listening)
    XCTAssertTrue(state.liveText.isEmpty)
    XCTAssertTrue(state.formattedText.isEmpty)
  }
}
