import XCTest

@testable import Codescribe

/// Falsifier for the 2026-09-06 12:06:09 hang of build 773 (pid 95323).
///
/// Evidence (`logs/codescribe-run-2026_0906_crash.log`, 1 ms `sample`,
/// `codescribe.log` 10:06:09Z): a 174 s listening take (14 projection
/// revisions) was refused at stop (`terminal transcript refused: seal coverage
/// incomplete`). The controller emitted `transcription_failed`, which reaches
/// the overlay as `handleError(message:)`. With a non-empty projection that
/// path keeps `.listening`, aborts the capture (`onRecordingStopped`), and
/// shows the "Dictation failed — transcript kept" toast. The main thread never
/// returned from
/// `NSHostingView.layout → preferencesDidChange → FocusBridge.invalidateKeyViewLoop
/// → updateDefaultKeyViewLoop → FocusNavigationSequence.next()`
/// (98 % CPU for ≥ 6 min, 31 hang reports). A 22 s refused take at 09:54 did
/// not hang the app.
///
/// Run 6 (2026-09-06 12:36) replayed the WRONG transition — a Rust status card
/// (`applyPresentationStatus`) swapping the transcript out — and returned in
/// 0.22 s for both lengths. These tests replay the production transition on
/// the real panel and hosting hierarchy: a revision stream ticking the
/// timeline, then `handleError`, then the toast clearing. The witness is the
/// layout pass returning, not a name in a view tree. The status-card case is
/// kept as the control that is already known to return.
final class OverlayRefusalLayoutHangTests: XCTestCase {
  private let sentence =
    "Zajrzyj z powrotem, zobacz, dowiedz się co to jest sesja, skąd ona się bierze, bo jest losowa, i sprawdź czy są dostosowane do naszego nowego kontraktu. "
  private let refusal =
    "transcription_failed: terminal transcript refused: seal coverage incomplete (1291264/2473472 samples covered; max gap 416768 > threshold 12000)"

  @MainActor
  func testProductionRefusalAfterLongRevisionStreamReturnsFromLayout() throws {
    // 14 growing revisions ≈ the 12:03–12:06 take that hung build 773.
    try assertProductionRefusalLayoutReturns(revisions: 14, label: "long")
  }

  @MainActor
  func testProductionRefusalAfterShortTakeReturnsFromLayout() throws {
    // One revision ≈ the 22 s take (09:54) that the app survived.
    try assertProductionRefusalLayoutReturns(revisions: 1, label: "short")
  }

  @MainActor
  func testStatusCardRefusalControlReturnsFromLayout() throws {
    try assertStatusCardLayoutReturns(revisions: 14, label: "control")
  }

  @MainActor
  func testToastClearObservationThrowsWhenDeadlineExpiresWithToastPresent() throws {
    let state = OverlayState()
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    state.applyTranscriptProjection(listeningProjection(sentence, sequence: 1))
    state.handleError(message: refusal)
    let toast = try XCTUnwrap(state.toast)

    // The real expiry task cannot run during this synchronous MainActor beat.
    // An already-expired observation deadline must fail, never count as removal.
    XCTAssertThrowsError(try waitForToastClear(state, timeout: 0)) { error in
      XCTAssertEqual(error as? ToastClearObservationError, .toastStillPresent(toast))
    }
    XCTAssertEqual(state.toast, toast, "observation must not clear the toast itself")
  }

  // MARK: Production transition (handleError → abort → toast)

  @MainActor
  private func assertProductionRefusalLayoutReturns(revisions: Int, label: String) throws {
    let harness = try mountListeningPanel(revisions: revisions, label: label)
    defer { harness.tearDown() }
    let state = harness.state
    let root = harness.root

    // AppModel.markStopped mirrors: the stopped beat re-enters the state
    // through `finishControllerRecording` (idempotent once finalized).
    var stoppedBeats = 0
    state.onRecordingStopped = { [weak state] in
      stoppedBeats += 1
      state?.finishControllerRecording()
    }

    state.handleError(message: refusal)
    XCTAssertEqual(state.mode, .listening, "a refusal with a draft keeps the transcript on screen")
    XCTAssertEqual(state.toast, "Dictation failed — transcript kept")
    XCTAssertEqual(stoppedBeats, 1)

    let refusalElapsed = measureLayout(root)
    print(
      "W5_T18_REFUSAL_LAYOUT transition=production label=\(label) chars=\(harness.chars) elapsed_s=\(refusalElapsed)"
    )
    XCTAssertLessThan(
      refusalElapsed, 2.0,
      "\(label): production refusal layout pass took \(refusalElapsed)s — key-view loop rebuild is not bounded"
    )

    // Second preference change of the same beat: the toast clears after 2.6 s.
    // Its production timer already ran during measureLayout. Keep the former
    // 3 s ceiling, but stop waiting as soon as removal is actually observed.
    try waitForToastClear(state)
    XCTAssertNil(state.toast, "toast must have cleared before measuring its removal")
    let toastElapsed = measureLayout(root)
    print(
      "W5_T18_REFUSAL_LAYOUT transition=toast-clear label=\(label) chars=\(harness.chars) elapsed_s=\(toastElapsed)"
    )
    XCTAssertLessThan(
      toastElapsed, 2.0, "\(label): toast removal layout pass took \(toastElapsed)s")
  }

  // MARK: Control (Rust status card swaps the transcript out)

  @MainActor
  private func assertStatusCardLayoutReturns(revisions: Int, label: String) throws {
    let harness = try mountListeningPanel(revisions: revisions, label: label)
    defer { harness.tearDown() }
    harness.state.applyPresentationStatus(refusedTerminalSeal())
    XCTAssertEqual(harness.state.mode, .error)
    let elapsed = measureLayout(harness.root)
    print(
      "W5_T18_REFUSAL_LAYOUT transition=status-card label=\(label) chars=\(harness.chars) elapsed_s=\(elapsed)"
    )
    XCTAssertLessThan(elapsed, 2.0, "\(label): status-card layout pass took \(elapsed)s")
  }

  // MARK: Harness

  private enum ToastClearObservationError: Error, Equatable {
    case toastStillPresent(String)
  }

  @MainActor
  private func waitForToastClear(_ state: OverlayState, timeout: TimeInterval = 3.0) throws {
    let deadline = ProcessInfo.processInfo.systemUptime + timeout
    while let toast = state.toast {
      let remaining = deadline - ProcessInfo.processInfo.systemUptime
      guard remaining > 0 else {
        throw ToastClearObservationError.toastStillPresent(toast)
      }
      // Service AppKit, SwiftUI and the real MainActor expiry task between
      // observations. No blocking sleep, fake expiry or busy polling.
      RunLoop.main.run(until: Date().addingTimeInterval(min(0.01, remaining)))
    }
  }

  private struct Harness {
    let state: OverlayState
    let panel: FloatingOverlayPanel
    let root: NSView
    let chars: Int

    @MainActor func tearDown() {
      panel.orderOut(nil)
      panel.invalidatePresence()
    }
  }

  /// Real panel + hosting hierarchy in `.listening`, fed `revisions` growing
  /// Rust-owned projections with the run loop ticking between them (timeline
  /// timer, NSTextView updates, waveform) — the shape of the 174 s take.
  @MainActor
  private func mountListeningPanel(revisions: Int, label: String) throws -> Harness {
    let state = OverlayState()
    state.handleRecordingPreparing()
    state.handleRecordingStarted()

    let panel = try XCTUnwrap(
      DictationOverlayWindow.make(
        state: state,
        textScale: TextScaleController(key: "OverlayRefusalLayoutHangTests.\(label).textScale")
      ) as? FloatingOverlayPanel
    )
    panel.setContentSize(NSSize(width: 470, height: 560))
    panel.orderFrontRegardless()
    let root = try XCTUnwrap(panel.contentView)
    root.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.2))

    var text = ""
    for revision in 1...max(revisions, 1) {
      text = String(repeating: sentence, count: revision)
      state.applyTranscriptProjection(listeningProjection(text, sequence: UInt64(revision)))
      root.layoutSubtreeIfNeeded()
      RunLoop.main.run(until: Date().addingTimeInterval(0.15))
    }
    XCTAssertEqual(state.mode, .listening)
    XCTAssertEqual(state.activeText, text)
    root.layoutSubtreeIfNeeded()
    return Harness(state: state, panel: panel, root: root, chars: text.count)
  }

  @MainActor
  private func measureLayout(_ root: NSView) -> TimeInterval {
    let started = Date()
    root.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.3))
    root.layoutSubtreeIfNeeded()
    return Date().timeIntervalSince(started)
  }

  /// Rust-owned projection admitted through the production boundary, shaped
  /// like the 12:03–12:06 take: one growing live document, not terminal.
  private func listeningProjection(_ text: String, sequence: UInt64) -> CsTranscriptProjectionEvent
  {
    let sampleEnd = UInt64(7_680 + 595_000 * sequence)
    let receipt = CsProjectedAcousticReceipt(
      acousticSerialVersion: 1,
      acousticSerial: "hang-acoustic-\(sequence)",
      sessionId: "hang-session",
      captureEpoch: 1,
      sampleStart: 7_680,
      sampleEnd: sampleEnd,
      durationMs: UInt64(sampleEnd / 48),
      energyIntegral: 1,
      meanRmsDbfs: -20,
      peakDbfs: -6,
      vadOpenSample: 7_680,
      vadCloseSample: sampleEnd,
      evidenceCalibrationVersion: "cal2-macbook-pro-microphone-1",
      wordEvidenceReceipts: ["hang-word-evidence-\(sequence)"],
      layerDecisionReceipts: ["hang-layer-decision-\(sequence)"],
      sealReceipt: nil,
      manualEditReceipt: nil,
      presentationReceipt: nil
    )
    return CsTranscriptProjectionEvent(
      schema: "codescribe.transcript_projection.v1",
      sequence: sequence,
      emittedAt: "2026-09-06T10:04:06Z",
      sessionId: "hang-session",
      mode: "dictation",
      reducerRevision: 3 + sequence,
      reducerAction: "record_ledger_projection",
      occurrenceSessionId: "hang-session",
      captureEpoch: 1,
      sampleStart: 7_680,
      sampleEnd: sampleEnd,
      documentIndex: 0,
      label: "live",
      renderedText: text,
      deliveryText: nil,
      phase: "listening",
      canPaste: false,
      canInsert: false,
      canCopy: true,
      canRetranscribe: false,
      canFormat: false,
      canSendToAgent: false,
      terminal: false,
      lifecycleTerminal: false,
      delivery: .unattempted,
      acousticReceipts: [receipt],
      sealCoverage: nil,
      consultationPresentations: []
    )
  }

  private func refusedTerminalSeal() -> CsPresentationStatusEvent {
    CsPresentationStatusEvent(
      schema: "codescribe.presentation-status.v1",
      emittedAt: "2026-09-06T10:06:09Z",
      sessionId: "hang-session",
      kind: "terminal_refused",
      code: "terminal_seal_coverage_incomplete",
      statusLabel: "transcript refused",
      headline: "Transcript refused",
      message:
        "terminal transcript refused: seal coverage incomplete (1291264/2473472 samples covered; max gap 416768 > threshold 12000)",
      isError: true,
      terminal: true,
      calibrationVersion: nil
    )
  }

  // MARK: New-take presentation on the real panel

  /// Founder witness, Dragon `3bae96614`: the next take opened on the previous
  /// take's words. The state contract is asserted in OverlayStateTests; this is
  /// the real hosting-hierarchy witness that clearing a long canvas repaints and
  /// that the layout pass still returns — the failure mode this file exists for.
  @MainActor
  func testNewTakeAfterARefusedTakeRepaintsFromEmptyAndLayoutReturns() throws {
    let harness = try mountListeningPanel(revisions: 14, label: "new-take")
    defer { harness.tearDown() }
    harness.state.handleError(message: refusal)
    XCTAssertEqual(harness.state.mode, .listening)
    XCTAssertEqual(harness.state.activeText.count, harness.chars)

    harness.state.handleRecordingPreparing()
    harness.root.layoutSubtreeIfNeeded()
    XCTAssertEqual(harness.state.canvasText, "", "the new take opens on its own screen")
    XCTAssertEqual(harness.state.activeText, "")

    let elapsed = measureLayout(harness.root)
    print(
      "W2_OVERLAY_NEW_TAKE_LAYOUT chars=\(harness.chars) elapsed_s=\(elapsed)"
    )
    XCTAssertLessThan(
      elapsed, 2.0,
      "clearing a \(harness.chars)-char canvas took \(elapsed)s — the repaint is not bounded"
    )
    XCTAssertEqual(
      harness.state.pendingSupersededTake?.renderedText.count, harness.chars,
      "the refused take's words are retired, not destroyed")
  }

  /// `hideForAgentHandoff` used to re-read `self.panel` in its fade completion.
  /// The panel object is cached and reused, so a take that started inside the
  /// 0.18 s fade had its own overlay ordered out by its predecessor's handoff.
  /// The fade is injected here so the completion fires at an exact point in the
  /// lifecycle instead of racing the animation.
  @MainActor
  func testHandoffFadeCompletionCannotCloseTheSuccessorsOverlay() throws {
    let state = OverlayState()
    let panel = try XCTUnwrap(
      DictationOverlayWindow.make(
        state: state,
        textScale: TextScaleController(key: "OverlayRefusalLayoutHangTests.handoff.textScale")
      ) as? FloatingOverlayPanel
    )
    defer {
      panel.orderOut(nil)
      panel.invalidatePresence()
    }

    // Storing the completion here is not just test convenience: it is the
    // witness for the injected seam's escaping lifetime. `OverlayController`
    // declares that completion `@escaping` precisely because both consumers keep
    // it past the call — AppKit's real `completionHandler` does, and so does
    // this fake. If that attribute were dropped, this line is the one that
    // could no longer hold the callback.
    var pendingFade: (@MainActor @Sendable () -> Void)?
    var outs = 0
    let controller = OverlayController(
      state: state,
      engine: nil,
      overlayEnabledProvider: { true },
      assistiveStatusProvider: { false },
      panelFactory: { _, _ in panel },
      orderPanelFront: { $0.orderFrontRegardless() },
      orderPanelOut: { shown in
        shown.orderOut(nil)
        outs += 1
      },
      // Mimic the real fade's visible effect so the alpha assertions below
      // describe a window that was actually dimmed, not one that never moved.
      // Deliberately NOT synchronous: calling `completed()` here would delete
      // the delay the successor assertions depend on.
      runHandoffFade: { faded, completed in
        faded.alphaValue = 0
        pendingFade = completed
      }
    )
    // The controller's production hooks reach `AppModel.shared`; these are the
    // panel calls they make, without building the real chat/license stack.
    state.onRecordingPreparing = { [unowned controller] in controller.showForRecording() }
    state.onRecordingStarted = { [unowned controller] in controller.showForRecording() }

    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    XCTAssertTrue(panel.isVisible)
    let handedOffGeneration = state.captureGeneration

    controller.hideForAgentHandoff()
    let staleFade = try XCTUnwrap(pendingFade, "the handoff must run a fade")
    XCTAssertEqual(outs, 0, "nothing is ordered out until the fade completes")
    XCTAssertEqual(panel.alphaValue, 0, "the window is mid-fade")

    // Take 2 opens while the predecessor's fade is still in flight.
    state.finishControllerRecording()
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    XCTAssertGreaterThan(state.captureGeneration, handedOffGeneration)
    XCTAssertTrue(panel.isVisible)
    XCTAssertEqual(panel.alphaValue, 1, "showing the successor undims the reused window")

    staleFade()
    XCTAssertEqual(outs, 0, "the predecessor's handoff cannot close the successor")
    XCTAssertTrue(panel.isVisible, "the successor keeps its window")
    XCTAssertEqual(panel.alphaValue, 1, "and that window is not left invisible")

    // The successor's own handoff still completes normally.
    controller.hideForAgentHandoff()
    let ownFade = try XCTUnwrap(pendingFade)
    ownFade()
    XCTAssertEqual(outs, 1, "a handoff still hides the take it actually fed")
    XCTAssertEqual(panel.alphaValue, 1)
    withExtendedLifetime(controller) {}
  }
}
