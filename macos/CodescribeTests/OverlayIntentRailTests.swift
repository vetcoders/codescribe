import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
private final class OverlayIntentBoundaryEngine: DictationEngine {
  var cloudConfigured = false
  func cloudRetranscribeConfigured() -> Bool { cloudConfigured }
  var onTranscribeFile: (() -> Void)?
  var receivedTranscribePath: String?
  var onFormatter: (() -> Void)?
  var onCopyTagged: (() -> Void)?
  var copiedTaggedText: String?
  var formatterRequests:
    [(sessionId: String, sourceRevision: UInt64, level: FormattingPolicyOption?)] = []
  var formatterFailure: Error?
  var policy = OverlayPolicySnapshot(autoFormatLevel: .correction)
  var assistiveMode = false
  var startModes: [Bool] = []
  var onStart: (() -> Void)?

  func setListener(_ listener: CsTranscriptionListener) {}
  func startsInAssistiveMode() -> Bool { assistiveMode }
  func startRecording(assistive: Bool, language: CsLanguage?) async throws {
    startModes.append(assistive)
    onStart?()
  }
  func stopRecording() async throws -> String { "" }
  func commitUserRevision(
    sessionId: String, sourceRevision: UInt64, renderedText: String
  ) async throws -> CsUserRevisionResult {
    return CsUserRevisionResult(
      sessionId: sessionId,
      sourceRevision: sourceRevision,
      revision: sourceRevision + 1,
      renderedText: renderedText,
      provenanceReceipt: "user-edit-intent-boundary"
    )
  }
  func commitFormatterRevision(
    sessionId: String, sourceRevision: UInt64, level: FormattingPolicyOption?
  ) async throws -> CsUserRevisionResult {
    formatterRequests.append((sessionId, sourceRevision, level))
    onFormatter?()
    if let formatterFailure { throw formatterFailure }
    return CsUserRevisionResult(
      sessionId: sessionId,
      sourceRevision: sourceRevision,
      revision: sourceRevision + 1,
      renderedText: "formatted",
      provenanceReceipt: "formatter-intent-boundary"
    )
  }
  func isRecording() async -> Bool { false }
  func initModel() async throws {}
  func isModelLoaded() -> Bool { true }
  func currentOverlayPolicy() -> OverlayPolicySnapshot? { policy }
  func pasteText(text: String) async throws -> CsPasteResult { pasteResult() }
  func deferText(text: String) async throws -> CsPasteResult { pasteResult() }
  func copyTaggedTranscript(text: String) async throws {
    copiedTaggedText = text
    onCopyTagged?()
  }
  func pasteTargetAppName() async -> String? { nil }
  func sendAssistiveTranscript(text: String) async throws -> Bool { false }
  func lastSessionAudioPath() -> String? { "/tmp/overlay-intent-boundary.wav" }
  func sessionAudioPath(sessionId: String) -> String? { "/tmp/overlay-intent-boundary.wav" }
  func transcribeFile(path: String) async throws -> CsTranscription {
    receivedTranscribePath = path
    onTranscribeFile?()
    return CsTranscription(text: "retranscribed", language: "en")
  }

  private func pasteResult() -> CsPasteResult {
    CsPasteResult(
      outcome: .noop,
      targetAppName: nil,
      frontmostAppName: nil,
      deferredInsertShortcut: nil,
      deferredInsertFailure: nil
    )
  }
}

@MainActor
private final class OverlayHeaderControlFramesRecorder {
  var frames = OverlayHeaderControlFrames()
}

@MainActor
private struct OverlayHeaderControlFramesCapture: View {
  let state: OverlayState
  let recorder: OverlayHeaderControlFramesRecorder

  var body: some View {
    DictationOverlayView(state: state)
      .onPreferenceChange(OverlayHeaderControlFramesPreferenceKey.self) {
        recorder.frames = $0
      }
  }
}

@MainActor
final class OverlayIntentRailTests: XCTestCase {
  func testIdleRecordingControlStartsOnceWithCurrentTrayMode() async {
    for assistive in [false, true] {
      let engine = OverlayIntentBoundaryEngine()
      engine.assistiveMode = assistive
      let started = expectation(description: "controller start in mode \(assistive)")
      engine.onStart = { started.fulfill() }
      let state = OverlayState(micAccessProvider: { true })
      state.engine = engine

      let control = OverlayRecordingControls(
        canFinish: false, presentationMode: .expanded, compact: false, palette: .dark,
        onIntent: state.relayIntent, onPreviewToggle: {})
      XCTAssertEqual(control.recordingSymbol, "mic.fill")
      XCTAssertEqual(control.recordingIdentifier, "overlay-start-recording")
      XCTAssertEqual(control.recordingLabel, "Start dictation")
      control.activateRecordingControl()
      state.relayIntent(.startRecording)
      await fulfillment(of: [started], timeout: 2)
      XCTAssertEqual(engine.startModes, [assistive])
      XCTAssertTrue(state.recording)
    }
  }

  func testFinalizingRecordingControlCannotStart() {
    var intents: [OverlayIntent] = []
    let control = OverlayRecordingControls(
      canFinish: false, presentationMode: .expanded, compact: false, palette: .dark,
      onIntent: { intents.append($0) }, onPreviewToggle: {}, isFinalizing: true)
    XCTAssertEqual(control.recordingSymbol, "stop.fill")
    XCTAssertEqual(control.recordingIdentifier, "overlay-stop-recording")
    control.activateRecordingControl()
    XCTAssertTrue(intents.isEmpty)

    let finalizing = OverlayState.previewTranscribing()
    let engine = OverlayIntentBoundaryEngine()
    finalizing.engine = engine
    finalizing.relayIntent(.startRecording)
    XCTAssertTrue(engine.startModes.isEmpty)
    XCTAssertFalse(finalizing.recording)

    let awaitingProjection = OverlayState.previewListening()
    awaitingProjection.engine = engine
    awaitingProjection.handleRecordingPreparing()
    awaitingProjection.finishControllerRecording()
    awaitingProjection.relayIntent(.startRecording)
    XCTAssertTrue(engine.startModes.isEmpty)
    XCTAssertFalse(awaitingProjection.recording)
  }

  func testLongHistoryPopoverStaysWithinViewport() {
    let history = (1...200).map {
      TranscriptHistoryRecord(
        entry: CsHistoryEntry(
          path: "take-\($0).txt", timestampMs: Int64($0),
          preview: "Transcript \($0)", kind: .raw),
        characterCount: 1_000 + $0)
    }
    let host = NSHostingView(
      rootView: OverlayTranscriptHistory().historyList(history)
        .font(.system(size: 12, weight: .medium))
        .padding(10)
        .frame(maxWidth: 280)
        .fixedSize(horizontal: false, vertical: true))
    let size = host.fittingSize
    XCTAssertGreaterThan(size.height, 100)
    XCTAssertLessThanOrEqual(size.height, 380)
    XCTAssertLessThanOrEqual(size.width, 280)
  }

  func testEventFixturesRenderFrozenProjectionTable() {
    let rows:
      [(
        phase: String, text: String, paste: Bool, insert: Bool, copy: Bool,
        retranscribe: Bool, format: Bool, terminal: Bool, expected: [OverlayIntent]
      )] = [
        ("listening", "live", false, false, true, false, false, false, [.finish, .copy, .close]),
        ("listening", "", false, false, false, false, false, false, [.finish, .close]),
        ("finalizing", "draft", false, false, true, false, false, false, [.copy, .close]),
        (
          "formatted", "final", true, true, true, true, true, true,
          [.insertPaste, .copy, .retranscribe, .format, .close]
        ),
        ("no_speech", "", false, false, false, true, false, true, [.retranscribe, .close]),
        // Refused coverage: the ledger declined the seal, the words are real.
        // Every producer-authorized recovery is painted. Format reaches the
        // existing reducer path and any terminal refusal is shown by name.
        (
          "coverage_refused", "usable words", true, true, true, true, true, true,
          [.insertPaste, .copy, .retranscribe, .format, .close]
        ),
        // Empty typed refusal: nothing to act on, and the rail invents nothing.
        ("coverage_refused", "", false, false, false, false, false, true, [.close]),
        // Contract change (rc-w2-refusal-ui): `error` used to return `[.close]`
        // regardless of the producer's bits. A delivery failure keeps its words
        // and its audio, so that fixed list hid Copy/Insert/Retranscribe behind
        // the word "error" at exactly the moment they were the recovery.
        (
          "error", "draft", true, true, true, true, true, true,
          [.insertPaste, .copy, .retranscribe, .close]
        ),
        // Sink failure with retained audio, no destination left: no Insert.
        (
          "error", "draft", false, false, true, true, false, true,
          [.copy, .retranscribe, .close]
        ),
        // All-false error stays exactly as strict as before.
        ("error", "", false, false, false, false, false, true, [.close]),
      ]

    for row in rows {
      let state = projectedState(
        phase: row.phase,
        text: row.text,
        canPaste: row.paste,
        canInsert: row.insert,
        canCopy: row.copy,
        canRetranscribe: row.retranscribe,
        canFormat: row.format,
        terminal: row.terminal
      )

      XCTAssertEqual(
        OverlayIntentRail.projectedIntents(for: state),
        row.expected,
        "event fixture for \(row.phase) did not paint the frozen action table"
      )
    }
  }

  func testCloudChoiceFollowsEngineConfigurationWithoutStartingATranscription() {
    let state = OverlayState()
    let engine = OverlayIntentBoundaryEngine()
    var calls = 0
    engine.onTranscribeFile = { calls += 1 }
    state.engine = engine
    state.refreshRetranscriptionAvailability()
    XCTAssertFalse(state.cloudRetranscribeConfigured)
    engine.cloudConfigured = true
    state.refreshRetranscriptionAvailability()
    XCTAssertTrue(state.cloudRetranscribeConfigured)
    engine.cloudConfigured = false
    state.refreshRetranscriptionAvailability()
    XCTAssertFalse(state.cloudRetranscribeConfigured)
    XCTAssertEqual(calls, 0)
  }

  func testDispatchUsesProductionStateRouteAndLeavesProjectionUntouched() async {
    let state = projectedState(
      phase: "formatted",
      text: "final",
      canPaste: true,
      canInsert: true,
      canCopy: true,
      canRetranscribe: true,
      canFormat: true,
      terminal: true
    )
    let engine = OverlayIntentBoundaryEngine()
    state.engine = engine
    let requested = expectation(description: "format intent reached production boundary")
    engine.onFormatter = { requested.fulfill() }
    let rail = OverlayIntentRail(
      phase: state.statusText,
      intents: OverlayIntentRail.projectedIntents(for: state),
      palette: .dark,
      onIntent: state.relayIntent
    )

    rail.dispatch(.format)
    await fulfillment(of: [requested], timeout: 1)

    XCTAssertEqual(state.mode, .formatted)
    XCTAssertEqual(state.formattedText, "final")
    XCTAssertEqual(state.revision, 1)
    XCTAssertTrue(state.canPaste)
    XCTAssertTrue(state.canInsert)
    XCTAssertTrue(state.canCopy)
    XCTAssertTrue(state.canRetranscribe)
    XCTAssertTrue(state.canFormat)
    XCTAssertTrue(state.terminal)
    XCTAssertEqual(engine.formatterRequests.count, 1)
    XCTAssertEqual(engine.formatterRequests[0].sessionId, "intent-rail-fixture")
    XCTAssertEqual(engine.formatterRequests[0].sourceRevision, 1)
    XCTAssertNil(engine.formatterRequests[0].level)
    XCTAssertTrue(state.formatterCommitPending, "FFI acknowledgement is not projection")
    XCTAssertNil(state.formatterError)
  }

  func testRefusedSealStillOffersFormatterAndNamesReducerRefusal() async {
    let state = projectedState(
      phase: "coverage_refused", text: "usable words", canPaste: false,
      canInsert: false, canCopy: true, canRetranscribe: true, canFormat: true,
      terminal: true)
    let engine = OverlayIntentBoundaryEngine()
    engine.formatterFailure = NSError(domain: "NotTerminal", code: 1)
    state.engine = engine
    XCTAssertTrue(OverlayIntentRail.projectedIntents(for: state).contains(.format))
    let requested = expectation(description: "refused formatter reached reducer")
    engine.onFormatter = { requested.fulfill() }
    state.relayIntent(.format)
    await fulfillment(of: [requested], timeout: 1)
    await Task.yield()
    XCTAssertEqual(engine.formatterRequests.count, 1)
    XCTAssertEqual(state.mode, .coverageRefused)
    XCTAssertEqual(state.formattedText, "usable words")
    XCTAssertTrue(state.formatterError?.contains("NotTerminal") == true)
  }

  func testFormatMenuChoicesForwardOnceAndLeaveSettingsUnchanged() async {
    for level in [FormattingPolicyOption.correction, .smart, .max] {
      let state = projectedState(
        phase: "formatted", text: "final", canPaste: true, canInsert: true,
        canCopy: true, canRetranscribe: true, canFormat: true, terminal: true)
      let engine = OverlayIntentBoundaryEngine()
      state.engine = engine
      let settingsBefore = engine.policy
      let displayedLevelBefore = state.autoFormatLevel
      var interactions = 0
      let rail = OverlayIntentRail(
        phase: state.statusText, intents: OverlayIntentRail.projectedIntents(for: state),
        palette: .dark, formatLevel: state.autoFormatLevel, onIntent: state.relayIntent,
        onFormatOnce: { state.formatTranscript(at: $0) },
        onInteraction: { interactions += 1 })
      let formatted = expectation(description: "menu choice formats once")
      engine.onFormatter = { formatted.fulfill() }

      rail.formatOnce(level)
      await fulfillment(of: [formatted], timeout: 1)

      XCTAssertEqual(engine.formatterRequests.count, 1)
      XCTAssertEqual(engine.formatterRequests.first?.level, level)
      XCTAssertEqual(engine.formatterRequests.first?.sessionId, "intent-rail-fixture")
      XCTAssertEqual(engine.formatterRequests.first?.sourceRevision, 1)
      XCTAssertEqual(engine.policy, settingsBefore)
      XCTAssertEqual(state.autoFormatLevel, displayedLevelBefore)
      XCTAssertEqual(rail.formatLevel, displayedLevelBefore)
      XCTAssertEqual(interactions, 1)
    }
  }

  func testAaPrimaryActionUsesSettingsWithoutAnOverride() async {
    let state = projectedState(
      phase: "formatted", text: "final", canPaste: true, canInsert: true,
      canCopy: true, canRetranscribe: true, canFormat: true, terminal: true)
    let engine = OverlayIntentBoundaryEngine()
    engine.policy = OverlayPolicySnapshot(autoFormatLevel: .smart)
    state.engine = engine
    let settingsBefore = engine.policy
    let rail = OverlayIntentRail(
      phase: state.statusText, intents: OverlayIntentRail.projectedIntents(for: state),
      palette: .dark, formatLevel: .smart, onIntent: state.relayIntent,
      onFormatOnce: { _ in XCTFail("Primary action must not choose a one-shot level") })
    let formatted = expectation(description: "Aa uses Settings")
    engine.onFormatter = { formatted.fulfill() }

    rail.dispatch(.format)
    await fulfillment(of: [formatted], timeout: 1)

    XCTAssertEqual(engine.formatterRequests.count, 1)
    XCTAssertNil(engine.formatterRequests.first?.level)
    XCTAssertEqual(engine.policy, settingsBefore)
  }

  func testFloatingActionsKeepProjectedOrderWithCloseInHeader() {
    let intents: [OverlayIntent] = [
      .insertPaste, .copy, .retranscribe, .format, .close,
    ]
    let layout = OverlayDockLayout(projectedIntents: intents)

    XCTAssertEqual(layout.visibleIntents, intents.filter { $0 != .close })
    XCTAssertEqual(OverlayDockLayout(projectedIntents: []).visibleIntents, [])
    XCTAssertEqual(OverlayDockLayout.minimumCanvasWidth, 320)
  }

  func testUnsealedTakeKeepsWarningAndProjectedRailInSeparateOrderedSlots() {
    let state = projectedState(
      phase: "coverage_refused", text: "Words kept without a seal",
      canPaste: true, canInsert: true, canCopy: true, canRetranscribe: true,
      canFormat: true, terminal: true)
    state.setPresentationMode(.expanded)
    let slots = OverlayBottomChromeSlots(
      mode: state.mode, hasPresentationStatus: state.presentationStatus != nil,
      isCollapsed: state.isCollapsed, showsDiagnostics: true)

    XCTAssertEqual(state.mode, .coverageRefused)
    XCTAssertEqual(slots.ordered, [.rail, .coverageWarning])
    XCTAssertTrue(slots.showsCoverageWarning)
    XCTAssertEqual(
      OverlayIntentRail.projectedIntents(for: state),
      [.insertPaste, .copy, .retranscribe, .format, .close])
    XCTAssertNotNil(state.coverageRefusalNotice)
    XCTAssertFalse(state.coverageRefusalDetail.isEmpty)
  }

  func testSealedTakeKeepsRailWithoutReservingWarningSlot() {
    let state = projectedState(
      phase: "formatted", text: "Sealed words",
      canPaste: true, canInsert: true, canCopy: true, canRetranscribe: true,
      canFormat: true, terminal: true)
    state.setPresentationMode(.expanded)
    let slots = OverlayBottomChromeSlots(
      mode: state.mode, hasPresentationStatus: state.presentationStatus != nil,
      isCollapsed: state.isCollapsed)

    XCTAssertEqual(state.mode, .formatted)
    XCTAssertEqual(slots.ordered, [.rail])
    XCTAssertFalse(slots.showsCoverageWarning)
    XCTAssertEqual(
      OverlayIntentRail.projectedIntents(for: state),
      [.insertPaste, .copy, .retranscribe, .format, .close])
    XCTAssertEqual(
      OverlayBottomChromeSlots(
        mode: .coverageRefused, hasPresentationStatus: false, isCollapsed: true
      ).ordered, [])
  }

  func testDirtyRevisionReplacesDeliveryActionsWithCommitOrDiscard() {
    let state = projectedState(
      phase: "formatted",
      text: "ledger text",
      canPaste: true,
      canInsert: true,
      canCopy: true,
      canRetranscribe: true,
      canFormat: true,
      terminal: true
    )

    state.beginTranscriptEdit()
    state.updateRevisionDraft("local draft")

    XCTAssertEqual(
      OverlayIntentRail.projectedIntents(for: state),
      [.commitRevision, .discardRevision, .close]
    )
    XCTAssertEqual(state.formattedText, "ledger text")
    XCTAssertEqual(state.canvasText, "local draft")
  }

  func testRetranscribeMenuPicksThePassAndTheBareIntentStaysLocal() async {
    let state = projectedState(
      phase: "formatted",
      text: "final",
      canPaste: true,
      canInsert: true,
      canCopy: true,
      canRetranscribe: true,
      canFormat: true,
      terminal: true
    )
    let engine = OverlayIntentBoundaryEngine()
    state.engine = engine
    // Retranscribe is opt-in with a Local / Cloud pick. Its route must not
    // change the formatting level stored in Settings.
    let rail = OverlayIntentRail(
      phase: state.statusText,
      intents: OverlayIntentRail.projectedIntents(for: state),
      palette: .dark,
      onIntent: state.relayIntent,
      onRetranscribe: { state.retranscribe(pass: $0) }
    )
    XCTAssertTrue(rail.intents.contains(.retranscribe))

    let cloud = expectation(description: "cloud pass reached the engine")
    engine.onTranscribeFile = { cloud.fulfill() }
    rail.retranscribe(.cloud)
    await fulfillment(of: [cloud], timeout: 1)
    XCTAssertEqual(engine.receivedTranscribePath, "cloud:/tmp/overlay-intent-boundary.wav")

    let local = expectation(description: "bare intent runs the local HQ pass")
    engine.onTranscribeFile = { local.fulfill() }
    rail.dispatch(.retranscribe)
    await fulfillment(of: [local], timeout: 1)
    XCTAssertEqual(engine.receivedTranscribePath, "hq:/tmp/overlay-intent-boundary.wav")
    XCTAssertEqual(
      engine.policy.autoFormatLevel, .correction, "retranscribe never writes the formatting level")
  }

  func testEveryIntentHasVoiceOverCopyAndRailReportsProjectedPhase() {
    let intents: [OverlayIntent] = [
      .finish, .commitRevision, .discardRevision, .copy, .insertPaste, .retranscribe, .format,
      .recoverSuperseded, .discardSuperseded, .close,
    ]

    XCTAssertEqual(
      intents.map(\.accessibilityLabel),
      [
        "Finish recording",
        "Commit transcript revision",
        "Discard transcript draft",
        "Copy transcript",
        "Insert transcript",
        "Transcribe this take again",
        "Format transcript",
        "Copy previous take to clipboard",
        "Discard previous take",
        "Close overlay",
      ]
    )
    XCTAssertTrue(intents.allSatisfy { !$0.accessibilityHint.isEmpty })
    XCTAssertTrue(intents.allSatisfy { !$0.helpText.isEmpty })
    XCTAssertEqual(OverlayDockVisuals.hoverOpacity(isHovering: false), 0)
    XCTAssertGreaterThan(OverlayDockVisuals.hoverOpacity(isHovering: true), 0)
    XCTAssertEqual(OverlayIntentRail.accessibilityValue(for: "no speech"), "no speech")
  }

  // MARK: Retained-work recovery on the sole action surface

  /// Negative then positive: the two recovery commands appear only when work is
  /// actually retained. Error recovery follows usable text and capabilities;
  /// only an empty error with no capabilities has a close-only rail.
  func testRecoveryCommandsAppearOnlyWithRetainedWorkAndSurviveErrorMode() {
    let empty = projectedState(
      phase: "error", text: "", canPaste: false, canInsert: false,
      canCopy: false, canRetranscribe: false, canFormat: false, terminal: true)
    XCTAssertEqual(OverlayIntentRail.projectedIntents(for: empty), [.close])
    XCTAssertEqual(OverlayIntentRail.recoveryIntents(for: empty), [])
    XCTAssertFalse(empty.hasRecoverableSupersededWork)

    let clean = projectedState(
      phase: "error",
      text: "draft",
      canPaste: true,
      canInsert: true,
      canCopy: true,
      canRetranscribe: true,
      canFormat: true,
      terminal: true
    )
    XCTAssertEqual(OverlayIntentRail.recoveryIntents(for: clean), [])
    XCTAssertEqual(
      OverlayIntentRail.projectedIntents(for: clean), [.insertPaste, .copy, .retranscribe, .close])
    XCTAssertFalse(OverlayIntentRail.projectedIntents(for: clean).contains(.format))
    XCTAssertEqual(clean.mode, .error)
    XCTAssertEqual(clean.activeText, "draft")
    XCTAssertFalse(clean.hasRecoverableSupersededWork)

    let retained = stateWithOneRetainedEdit()
    XCTAssertTrue(retained.hasRecoverableSupersededWork)
    XCTAssertEqual(
      OverlayIntentRail.recoveryIntents(for: retained),
      [.recoverSuperseded, .discardSuperseded])

    // A live capture still shows at most the labelled affordance — never the
    // previous words, which stay off the canvas.
    retained.handleRecordingStarted()
    XCTAssertEqual(retained.canvasText, "")
    XCTAssertEqual(
      OverlayIntentRail.projectedIntents(for: retained),
      [.recoverSuperseded, .discardSuperseded, .finish, .close])

    // Retained work still projects recovery, but it does not open the tools.
    let actions = OverlayActionsPresentation()
    XCTAssertEqual(actions.phase, .idle)
    XCTAssertTrue(retained.hasRecoverableSupersededWork)
  }

  /// The rail's own dispatch route reaches the retention owner, and the
  /// retained bytes leave through the injected pasteboard rather than the
  /// reducer, a seal, a delivery or the canvas.
  func testPreviousTakeMenuDispatchesRecoveryAndDiscardThroughTheProductionRoute() throws {
    let url = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
      .deletingLastPathComponent()
      .appendingPathComponent("Codescribe/Screens/Overlay/OverlayIntentRail.swift")
    let source = try String(contentsOf: url, encoding: .utf8)
    XCTAssertTrue(source.contains("dispatch(.recoverSuperseded)"))
    XCTAssertTrue(source.contains("dispatch(.discardSuperseded)"))
    XCTAssertTrue(source.contains("role: .destructive"))
    XCTAssertTrue(source.contains("overlay-previous-take-menu"))
    let recovered = stateWithOneRetainedEdit()
    let engine = OverlayIntentBoundaryEngine()
    recovered.engine = engine
    var written: [String] = []
    recovered.recoveryClipboardWriter = { text in
      written.append(text)
      return true
    }
    let rail = OverlayIntentRail(
      phase: recovered.statusText,
      intents: OverlayIntentRail.projectedIntents(for: recovered),
      palette: .dark,
      onIntent: recovered.relayIntent
    )
    XCTAssertTrue(rail.intents.contains(.recoverSuperseded))
    XCTAssertTrue(rail.intents.contains(.discardSuperseded))

    rail.dispatch(.recoverSuperseded)

    XCTAssertEqual(written, ["Zdanie z moją poprawką."])
    XCTAssertFalse(recovered.hasRecoverableSupersededWork)
    XCTAssertNil(recovered.recoveryFailure)
    XCTAssertTrue(engine.formatterRequests.isEmpty, "recovery is not a reducer path")
    XCTAssertEqual(engine.policy.autoFormatLevel, .correction)
    XCTAssertFalse(recovered.isEditingTranscript, "recovery takes no focus")

    let discarded = stateWithOneRetainedEdit()
    let discardRail = OverlayIntentRail(
      phase: discarded.statusText,
      intents: OverlayIntentRail.projectedIntents(for: discarded),
      palette: .dark,
      onIntent: discarded.relayIntent)
    discardRail.dispatch(.discardSuperseded)
    XCTAssertFalse(discarded.hasRecoverableSupersededWork)

    // A refused write keeps the item, so the rail keeps offering both choices.
    let refused = stateWithOneRetainedEdit()
    refused.recoveryClipboardWriter = { _ in false }
    refused.relayIntent(.recoverSuperseded)
    XCTAssertTrue(refused.hasRecoverableSupersededWork)
    XCTAssertNotNil(refused.recoveryFailure)
    XCTAssertEqual(
      OverlayIntentRail.recoveryIntents(for: refused),
      [.recoverSuperseded, .discardSuperseded])
  }

  func testOverlayActionSymbolsHaveOneMeaningAcrossRailHeaderAndPlacement() {
    // All cases deliberately over-approximate co-visibility, so adding an
    // intent cannot silently evade the census. Close is a custom brand dot.
    let symbols =
      OverlayIntent.allCases.filter { $0 != .close }.map(\.systemImage)
      + [
        OverlayControlSymbols.history, OverlayControlSymbols.previousTake,
        OverlayControlSymbols.actions, OverlayControlSymbols.placement,
        OverlayControlSymbols.miniToMidi, OverlayControlSymbols.midiToTranscript,
        OverlayControlSymbols.returnToMini, "pin.fill",
        "arrow.up.and.down.and.arrow.left.and.right",
      ] + OverlayAnchor.allCases.map(\.systemImage)
    let collisions = Dictionary(grouping: symbols, by: { $0 }).filter { $0.value.count > 1 }
    XCTAssertTrue(collisions.isEmpty, "Duplicate overlay symbols: \(collisions.keys.sorted())")
    for symbol in symbols {
      XCTAssertNotNil(NSImage(systemSymbolName: symbol, accessibilityDescription: nil), symbol)
    }
  }

  func testActionsHandleMorphsToCloseOnlyWhileRailIsExpanded() {
    var actions = OverlayActionsPresentation()
    XCTAssertEqual(actions.controlSymbol, "ellipsis")
    XCTAssertEqual(actions.controlTitle, "More actions")

    actions.toggle()
    XCTAssertEqual(actions.phase, .open)
    XCTAssertEqual(actions.controlSymbol, "xmark")
    XCTAssertEqual(actions.controlTitle, "Close actions")

    actions.toggle()
    XCTAssertEqual(actions.phase, .idle)
    XCTAssertEqual(actions.controlSymbol, "ellipsis")
    XCTAssertEqual(actions.controlTitle, "More actions")
  }

  /// One formatted take with an uncommitted edit, superseded by a new capture.
  private func stateWithOneRetainedEdit() -> OverlayState {
    let state = projectedState(
      phase: "formatted",
      text: "Zdanie.",
      canPaste: true,
      canInsert: true,
      canCopy: true,
      canRetranscribe: true,
      canFormat: true,
      terminal: true
    )
    state.beginTranscriptEdit()
    state.updateRevisionDraft("Zdanie z moją poprawką.")
    state.endTranscriptEdit()
    state.handleRecordingPreparing()
    return state
  }

  func testProjectedRetranscribeIntentReachesInjectedEngineBoundary() async {
    let engine = OverlayIntentBoundaryEngine()
    let state = projectedState(
      phase: "formatted",
      text: "final",
      canPaste: true,
      canInsert: true,
      canCopy: true,
      canRetranscribe: true,
      canFormat: true,
      terminal: true
    )
    state.engine = engine
    let reached = expectation(description: "retranscribe reaches the engine boundary")
    engine.onTranscribeFile = { reached.fulfill() }

    state.relayIntent(.retranscribe)

    await fulfillment(of: [reached], timeout: 0.2)
    XCTAssertEqual(engine.receivedTranscribePath, "hq:/tmp/overlay-intent-boundary.wav")
    XCTAssertEqual(state.toast, "retranscribed — Back keeps the old text")
  }

  func testMissingEngineSurfacesCopyAndInsertFailuresOnCanvas() {
    let state = projectedState(
      phase: "formatted",
      text: "final",
      canPaste: true,
      canInsert: true,
      canCopy: true,
      canRetranscribe: false,
      canFormat: false,
      terminal: true
    )

    state.relayIntent(.copy)
    XCTAssertEqual(state.toast, "copy unavailable")
    XCTAssertEqual(state.errorMessage, "Copy needs the recording engine")

    state.relayIntent(.insertPaste)
    XCTAssertEqual(state.toast, "insert unavailable")
    XCTAssertEqual(state.errorMessage, "Insert needs the recording engine")
  }

  @MainActor
  func testDockRendersAtWindowFloorWithRoundedCanvasCorners() throws {
    let state = OverlayState.previewListening()
    let size = CGSize(
      width: OverlayDockLayout.minimumCanvasWidth,
      height: DictationOverlayWindow.minSize.height
    )

    let expandedRecorder = OverlayHeaderControlFramesRecorder()
    let hostingView = NSHostingView(
      rootView: OverlayHeaderControlFramesCapture(state: state, recorder: expandedRecorder)
        .frame(width: size.width, height: size.height)
        .preferredColorScheme(.dark)
    )
    hostingView.frame = CGRect(origin: .zero, size: size)
    hostingView.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.03))
    // Annex A2 removed the Auto Paste chip; the width it held went back to
    // the waveform, so at the window floor (no agent glyph, and no timer in a
    // windowless host) the full meter fits instead of the compact one.
    try assertHeaderControlsFit(
      expandedRecorder.frames,
      inside: size.width,
      context: "expanded listening header",
      compactMeter: true
    )

    let collapsedState = OverlayState.previewListening()
    collapsedState.handleRecordingPreparing()
    collapsedState.setPresentationMode(.midi)
    let collapsedRecorder = OverlayHeaderControlFramesRecorder()
    let collapsedHost = NSHostingView(
      rootView: OverlayHeaderControlFramesCapture(
        state: collapsedState, recorder: collapsedRecorder
      )
      .frame(width: 470, height: DictationOverlayWindow.collapsedHeight)
      .preferredColorScheme(.dark)
    )
    collapsedHost.frame = CGRect(
      x: 0, y: 0, width: 470, height: DictationOverlayWindow.collapsedHeight)
    collapsedHost.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.03))
    try assertHeaderControlsFit(
      collapsedRecorder.frames,
      inside: 470,
      context: "midi listening header",
      compactMeter: false
    )

    let bitmap = try XCTUnwrap(hostingView.bitmapImageRepForCachingDisplay(in: hostingView.bounds))
    hostingView.cacheDisplay(in: hostingView.bounds, to: bitmap)
    let png = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
    let destination = FileManager.default.temporaryDirectory
      .appendingPathComponent("codescribe-expanded-bottom-dock-min-width.png")
    try png.write(to: destination)

    XCTAssertGreaterThan(png.count, 800)
    XCTAssertEqual(size.width, DictationOverlayWindow.minSize.width)
    XCTAssertLessThan(bitmap.colorAt(x: 0, y: 0)?.alphaComponent ?? 0, 0.2)
    XCTAssertLessThan(
      bitmap.colorAt(x: bitmap.pixelsWide - 1, y: 0)?.alphaComponent ?? 0,
      0.2
    )
  }

  private func assertHeaderControlsFit(
    _ frames: OverlayHeaderControlFrames,
    inside width: CGFloat,
    context: String,
    compactMeter: Bool = true,
    file: StaticString = #filePath,
    line: UInt = #line
  ) throws {
    let stop = try XCTUnwrap(
      frames.stop, "\(context) has no Stop control geometry", file: file, line: line
    )
    let preview = try XCTUnwrap(
      frames.preview, "\(context) has no preview control geometry", file: file, line: line
    )
    let waveform = try XCTUnwrap(
      frames.waveform, "\(context) has no waveform geometry", file: file, line: line
    )

    XCTAssertGreaterThanOrEqual(
      stop.width, 22, "\(context) Stop hit target is too narrow", file: file, line: line
    )
    XCTAssertGreaterThanOrEqual(
      stop.height, 22, "\(context) Stop hit target is too short", file: file, line: line
    )
    XCTAssertGreaterThanOrEqual(
      preview.width, 22, "\(context) preview hit target is too narrow", file: file, line: line
    )
    XCTAssertGreaterThanOrEqual(
      preview.height, 22, "\(context) preview hit target is too short", file: file, line: line
    )
    // Founder, 2026-09-29: "ten stop jest olbrzymi". Stop is the chevron's
    // twin — one hairline circle, no word — so it can never outgrow the row.
    XCTAssertEqual(
      stop.width, preview.width, accuracy: 0.5,
      "\(context) Stop is wider than the chevron", file: file, line: line)
    XCTAssertEqual(
      stop.height, preview.height, accuracy: 0.5,
      "\(context) Stop is taller than the chevron", file: file, line: line)
    XCTAssertEqual(
      stop.width, OverlayRecordingControls.controlDiameter, accuracy: 0.5,
      "\(context) Stop left the shared control diameter", file: file, line: line)
    XCTAssertFalse(stop.intersects(preview), "\(context) controls overlap", file: file, line: line)
    XCTAssertFalse(
      stop.intersects(waveform), "\(context) Stop overlaps the meter", file: file, line: line
    )
    XCTAssertFalse(
      preview.intersects(waveform), "\(context) preview overlaps the meter", file: file, line: line
    )
    XCTAssertGreaterThanOrEqual(
      stop.minX, 0, "\(context) clips the Stop control", file: file, line: line
    )
    XCTAssertLessThanOrEqual(
      stop.maxX, width, "\(context) clips the Stop control", file: file, line: line
    )
    XCTAssertGreaterThanOrEqual(
      preview.minX, 0, "\(context) clips the preview control", file: file, line: line
    )
    XCTAssertLessThanOrEqual(
      preview.maxX, width, "\(context) clips the preview control", file: file, line: line
    )
    if compactMeter {
      XCTAssertLessThan(
        waveform.width, 100, "\(context) did not select the compact meter", file: file, line: line
      )
    } else {
      XCTAssertGreaterThanOrEqual(
        waveform.width, 100, "\(context) did not retain the full meter", file: file, line: line
      )
    }
  }

  @MainActor
  func testHeaderUsesFullMeterWhenWindowHasRoom() throws {
    let width = DictationOverlayWindow.minSize.width + 160
    let size = CGSize(width: width, height: DictationOverlayWindow.minSize.height)
    let recorder = OverlayHeaderControlFramesRecorder()
    let host = NSHostingView(
      rootView: OverlayHeaderControlFramesCapture(
        state: OverlayState.previewListening(), recorder: recorder
      )
      .frame(width: size.width, height: size.height)
      .preferredColorScheme(.dark)
    )
    host.frame = CGRect(origin: .zero, size: size)
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.03))

    try assertHeaderControlsFit(
      recorder.frames,
      inside: width,
      context: "expanded wide header",
      compactMeter: false
    )
  }

  /// Component-level control contract only; this does not start or stop capture.
  @MainActor
  func testPersistentRecordingControlsUseProjectedFinishAndExistingIntentRelay() {
    let listeningIntents = OverlayIntentRail.projectedIntents(
      phase: .listening, canPaste: false, canInsert: false, canCopy: false,
      canRetranscribe: false, canFormat: false
    )
    XCTAssertTrue(OverlayRecordingControls.showsStop(for: listeningIntents))
    XCTAssertEqual(OverlayRecordingControls.railIntents(from: listeningIntents), [.close])

    let finalizingIntents = OverlayIntentRail.projectedIntents(
      phase: .finalizing, canPaste: false, canInsert: false, canCopy: false,
      canRetranscribe: false, canFormat: false
    )
    XCTAssertFalse(OverlayRecordingControls.showsStop(for: finalizingIntents))
    XCTAssertEqual(OverlayRecordingControls.railIntents(from: finalizingIntents), [.close])

    var routedIntents: [OverlayIntent] = []
    var previewCollapsed = false
    let expanded = OverlayRecordingControls(
      canFinish: true,
      presentationMode: .expanded,
      compact: false,
      palette: .dark,
      onIntent: { routedIntents.append($0) },
      onPreviewToggle: { previewCollapsed.toggle() }
    )
    expanded.finishRecording()
    expanded.togglePreview()
    XCTAssertEqual(routedIntents, [.finish])
    XCTAssertTrue(previewCollapsed)
    XCTAssertEqual(expanded.previewAccessibilityLabel, "Collapse widget")
    XCTAssertEqual(expanded.previewSymbol, "arrow.left")

    let collapsed = OverlayRecordingControls(
      canFinish: true,
      presentationMode: .mini,
      compact: true,
      palette: .dark,
      onIntent: { routedIntents.append($0) },
      onPreviewToggle: { previewCollapsed.toggle() }
    )
    XCTAssertTrue(collapsed.showsStop)
    XCTAssertEqual(collapsed.previewAccessibilityLabel, "Expand to compact widget")
    XCTAssertEqual(collapsed.previewSymbol, "arrow.right", "click opens MIDI")

    let unavailable = OverlayRecordingControls(
      canFinish: false,
      presentationMode: .expanded,
      compact: true,
      palette: .dark,
      onIntent: { routedIntents.append($0) },
      onPreviewToggle: {}
    )
    unavailable.finishRecording()
    XCTAssertEqual(routedIntents, [.finish])
  }

  // MARK: Refusal recovery (rc-w2-refusal-ui) — UNRUN under W2

  /// The census claim, stated as a test rather than as prose: every case of
  /// `OverlayMode` is asked for its rail, and the two terminal outcomes that
  /// are not `formatted` must not collapse to a bare Close when the producer
  /// says recovery exists.
  func testEveryProjectionPhaseAnswersTheRailAndOnlySilenceProducesBareClose() {
    // A flat list, not a dictionary: every case is named exactly once, so a
    // seventh phase added to `OverlayMode` without a row here is a visible
    // omission rather than a silently missing key.
    let allBitsOn: [(mode: OverlayMode, expected: [OverlayIntent])] = [
      (.listening, [.finish, .copy, .close]),
      (.finalizing, [.copy, .close]),
      (.formatted, [.insertPaste, .copy, .retranscribe, .format, .close]),
      (.coverageRefused, [.insertPaste, .copy, .retranscribe, .format, .close]),
      (.noSpeech, [.retranscribe, .close]),
      (.error, [.insertPaste, .copy, .retranscribe, .close]),
    ]

    for (mode, expected) in allBitsOn {
      XCTAssertEqual(
        OverlayIntentRail.projectedIntents(
          phase: mode, canPaste: true, canInsert: true, canCopy: true,
          canRetranscribe: true, canFormat: true),
        expected,
        "\(mode) did not paint its full producer-authorized rail"
      )
      let silent = OverlayIntentRail.projectedIntents(
        phase: mode, canPaste: false, canInsert: false, canCopy: false,
        canRetranscribe: false, canFormat: false)
      XCTAssertEqual(
        silent.filter { $0 != .finish }, [.close],
        "\(mode) invented a command the producer did not authorize"
      )
    }
  }

  /// Refused coverage honors the producer bit; an error phase still withholds
  /// Format because the user cannot start a terminal formatter there.
  func testRefusedFormatUsesProducerPermission() {
    XCTAssertTrue(
      OverlayIntentRail.projectedIntents(
        phase: .coverageRefused, canPaste: false, canInsert: false, canCopy: false,
        canRetranscribe: false, canFormat: true
      ).contains(.format))
    for mode in [OverlayMode.error] {
      XCTAssertFalse(
        OverlayIntentRail.projectedIntents(
          phase: mode, canPaste: false, canInsert: false, canCopy: false,
          canRetranscribe: false, canFormat: true
        ).contains(.format),
        "\(mode) projected Format, whose relay refuses outside .formatted"
      )
    }
    XCTAssertTrue(
      OverlayIntentRail.projectedIntents(
        phase: .formatted, canPaste: false, canInsert: false, canCopy: false,
        canRetranscribe: false, canFormat: true
      ).contains(.format)
    )
  }

  /// A refused take reaching the rail through the real projection boundary,
  /// with its recovery commands dispatched into the production relay. Copy is
  /// the falsifier that matters: if the relay had been phase-gated, this would
  /// silently do nothing while the button was on screen.
  func testRefusedCoverageRecoveryReachesTheProductionRelay() async {
    let state = projectedState(
      phase: "coverage_refused",
      text: "usable but unsealed",
      canPaste: false,
      canInsert: false,
      canCopy: true,
      canRetranscribe: true,
      canFormat: false,
      terminal: true
    )
    let engine = OverlayIntentBoundaryEngine()
    state.engine = engine

    XCTAssertEqual(state.mode, .coverageRefused)
    XCTAssertEqual(state.statusText, "unverified coverage")
    XCTAssertEqual(
      OverlayIntentRail.projectedIntents(for: state), [.copy, .retranscribe, .close])

    let rail = OverlayIntentRail(
      phase: state.statusText,
      intents: OverlayIntentRail.projectedIntents(for: state),
      palette: .dark,
      onIntent: state.relayIntent,
      onRetranscribe: state.retranscribe
    )
    let copied = expectation(description: "copy intent reached the production relay")
    engine.onCopyTagged = { copied.fulfill() }
    rail.dispatch(.copy)
    await fulfillment(of: [copied], timeout: 1)

    XCTAssertEqual(engine.copiedTaggedText, "usable but unsealed")
    XCTAssertEqual(state.mode, .coverageRefused, "recovery must not relabel the phase")
    XCTAssertEqual(
      OverlayIntentRail.accessibilityValue(for: state.statusText), "unverified coverage")
  }

  func testErrorRecoveryCopyUsesProductionRouteWithoutFormatting() async {
    let state = projectedState(
      phase: "error", text: "usable error words", canPaste: false, canInsert: false,
      canCopy: true, canRetranscribe: true, canFormat: true, terminal: true)
    let engine = OverlayIntentBoundaryEngine()
    state.engine = engine
    let projection = state.latestTranscriptProjection
    let rail = OverlayIntentRail(
      phase: state.statusText,
      intents: OverlayIntentRail.projectedIntents(for: state),
      palette: .dark,
      onIntent: state.relayIntent
    )
    XCTAssertEqual(rail.intents, [.copy, .retranscribe, .close])
    let copied = expectation(description: "error recovery copy reached the production relay")
    engine.onCopyTagged = { copied.fulfill() }
    rail.dispatch(.copy)
    await fulfillment(of: [copied], timeout: 1)

    XCTAssertEqual(engine.copiedTaggedText, "usable error words")
    XCTAssertEqual(state.mode, .error)
    XCTAssertEqual(state.activeText, "usable error words")
    XCTAssertEqual(state.latestTranscriptProjection?.sequence, projection?.sequence)
    XCTAssertEqual(state.latestTranscriptProjection?.reducerRevision, projection?.reducerRevision)
    XCTAssertEqual(state.latestTranscriptProjection?.reducerAction, "intent_rail_fixture")
    XCTAssertTrue(
      engine.formatterRequests.isEmpty, "copy recovery must not create a revision or seal")
    XCTAssertEqual(engine.policy.autoFormatLevel, .correction)
  }

  /// Unacknowledged superseded work still LEADS the rail on a refused take.
  /// The refusal is precisely the case where the phase table is thin, and a
  /// thin table may not be the reason an unsaved edit becomes unreachable.
  func testRetainedWorkStillLeadsTheRailOnARefusedTake() {
    let state = projectedState(
      phase: "formatted",
      text: "previous take",
      canPaste: false,
      canInsert: false,
      canCopy: true,
      canRetranscribe: false,
      canFormat: false,
      terminal: true
    )
    state.beginTranscriptEdit()
    state.updateRevisionDraft("previous take, edited")
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    XCTAssertTrue(state.hasRecoverableSupersededWork)

    state.applyTranscriptProjection(
      transcriptProjection(
        sequence: 9,
        emittedAt: "2026-09-10T00:00:00Z",
        sessionId: "intent-rail-successor",
        renderedText: "refused words",
        phase: "coverage_refused",
        terminal: true,
        reducerAction: "session_ended",
        captureEpoch: 2,
        sampleStart: 16_000,
        sampleEnd: 32_000,
        documentIndex: 1,
        canCopy: true,
        canRetranscribe: true
      )
    )

    XCTAssertEqual(
      OverlayIntentRail.projectedIntents(for: state),
      [.recoverSuperseded, .discardSuperseded, .copy, .retranscribe, .close]
    )
  }

  private func projectedState(
    phase: String,
    text: String,
    canPaste: Bool,
    canInsert: Bool,
    canCopy: Bool,
    canRetranscribe: Bool,
    canFormat: Bool,
    terminal: Bool
  ) -> OverlayState {
    let state = OverlayState()
    state.applyTranscriptProjection(
      transcriptProjection(
        sequence: 1,
        emittedAt: "2026-09-04T00:00:00Z",
        sessionId: "intent-rail-fixture",
        renderedText: text,
        phase: phase,
        terminal: terminal,
        reducerAction: "intent_rail_fixture",
        label: phase,
        canPaste: canPaste,
        canInsert: canInsert,
        canCopy: canCopy,
        canRetranscribe: canRetranscribe,
        canFormat: canFormat
      )
    )
    return state
  }
}
