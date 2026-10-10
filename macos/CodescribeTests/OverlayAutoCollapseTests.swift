import AppKit
import XCTest

@testable import Codescribe

/// Deterministic clock and scheduler for the automatic-collapse deadline.
/// Wakes run only when the test advances time; nothing sleeps.
@MainActor
final class ManualAutoCollapseScheduler {
  final class Wake {
    let due: TimeInterval
    let run: @MainActor () -> Void
    var cancelled = false
    init(due: TimeInterval, run: @escaping @MainActor () -> Void) {
      self.due = due
      self.run = run
    }
  }

  var now: TimeInterval = 1_000
  private(set) var wakes: [Wake] = []
  var outstanding: Int { wakes.filter { !$0.cancelled }.count }

  var scheduler: OverlayAutoCollapse.Scheduler {
    { [unowned self] delay, run in
      let wake = Wake(due: now + delay, run: run)
      wakes.append(wake)
      return { wake.cancelled = true }
    }
  }

  /// Advance the clock and run every due wake, including wakes scheduled by a
  /// wake that ran (an early wake sleeping its remainder).
  func advance(by seconds: TimeInterval) {
    now += seconds
    while let next = wakes.first(where: { !$0.cancelled && $0.due <= now }) {
      next.cancelled = true
      next.run()
    }
  }
}

@MainActor
final class OverlayAutoCollapseTests: XCTestCase {
  private func makeState(
    expandedByDefault: Bool = true
  ) -> (OverlayState, ManualAutoCollapseScheduler) {
    let clock = ManualAutoCollapseScheduler()
    let state = OverlayState(
      nowProvider: { [unowned clock] in clock.now },
      autoSendEnabled: { false },
      micAccessProvider: { true },
      autoCollapseScheduler: clock.scheduler)
    let engine = OverlayChromePolicyEngine()
    engine.expanded = expandedByDefault
    state.engine = engine
    state.attach()
    return (state, clock)
  }

  /// One automatic take expansion. The live capture holds the panel; its end
  /// arms the first idle interval.
  private func runTake(_ state: OverlayState) {
    state.handleRecordingPreparing()
    state.finishControllerRecording()
  }

  /// One automatic take that ends sealed: the terminal outcome arms the usual
  /// 5 s auto-hide next to the 10 s automatic return.
  private func runTerminalTake(_ state: OverlayState, sessionId: String = "collapse-take") {
    state.handleRecordingPreparing()
    state.handleRecordingStarted()
    let receipt = projectedAcousticReceipt(
      serial: "\(sessionId)-acoustic-1", sessionId: sessionId, sampleStart: 0,
      sampleEnd: 16_000, wordEvidence: ["\(sessionId)-word-1"],
      layerDecisions: ["\(sessionId)-layer-1"], sealReceipt: "\(sessionId)-seal-1")
    state.applyTranscriptProjection(
      transcriptProjection(
        sequence: 1, emittedAt: "2026-10-10T00:00:00Z", sessionId: sessionId,
        renderedText: "sealed words", phase: "formatted", terminal: true,
        reducerAction: "record_ledger_terminal_seal", sampleStart: 0, sampleEnd: 16_000,
        canPaste: true, canInsert: true, canRetranscribe: true, acousticReceipts: [receipt]))
    state.finishControllerRecording()
  }

  /// The 5 s hide comes due first. It must not take the full panel off screen
  /// before its 10 s return; once the panel is back in mini, the hide gets an
  /// ordinary 5 s countdown.
  func testTerminalHideWaitsForTheAutomaticReturnThenCountsItsUsualFiveSeconds() throws {
    let (state, clock) = makeState()
    var closes = 0
    state.onClose = { closes += 1 }
    state.setPresentationMode(.mini)
    runTerminalTake(state)
    XCTAssertEqual(state.presentationMode, .expanded)
    XCTAssertEqual(state.autoCollapseRestoreMode, .mini)
    XCTAssertEqual(
      try XCTUnwrap(state.autoHideDeadline), clock.now + OverlayState.autoHideDelaySeconds)

    clock.advance(by: OverlayState.autoHideDelaySeconds)
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closes, 0, "the full panel still owes its return")
    XCTAssertTrue(state.autoHideAwaitsAutoCollapse)
    XCTAssertEqual(state.presentationMode, .expanded)

    clock.advance(by: OverlayAutoCollapse.idleSeconds - OverlayState.autoHideDelaySeconds)
    XCTAssertEqual(state.presentationMode, .mini, "the 10 s return happened on screen")
    XCTAssertFalse(state.autoHideAwaitsAutoCollapse)
    XCTAssertEqual(
      try XCTUnwrap(state.autoHideDeadline), clock.now + OverlayState.autoHideDelaySeconds)

    clock.advance(by: OverlayState.autoHideDelaySeconds - 0.5)
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closes, 0)
    clock.advance(by: 0.5)
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closes, 1, "the compact panel leaves on its usual countdown")
  }

  /// Without an automatic expansion the 5 s hide is unchanged.
  func testTerminalHideOfACompactTakeStaysFiveSeconds() {
    let (state, clock) = makeState(expandedByDefault: false)
    var closes = 0
    state.onClose = { closes += 1 }
    state.setPresentationMode(.mini)
    runTerminalTake(state)
    XCTAssertNil(state.autoCollapseRestoreMode)
    clock.advance(by: OverlayState.autoHideDelaySeconds)
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closes, 1)
    XCTAssertFalse(state.autoHideAwaitsAutoCollapse)
  }

  /// A manual Transcription choice while the hide waits cancels the return; the
  /// manually chosen full panel stays full and leaves on the usual countdown.
  func testManualChoiceWhileTheHideWaitsKeepsTheFormAndRestoresTheCountdown() throws {
    let (state, clock) = makeState()
    var closes = 0
    state.onClose = { closes += 1 }
    state.setPresentationMode(.midi)
    runTerminalTake(state)
    clock.advance(by: OverlayState.autoHideDelaySeconds)
    state.fireAutoHideNowForTests()
    XCTAssertTrue(state.autoHideAwaitsAutoCollapse)

    state.setPresentationMode(.expanded)
    XCTAssertNil(state.autoCollapseRestoreMode)
    XCTAssertFalse(state.autoHideAwaitsAutoCollapse)
    XCTAssertEqual(
      try XCTUnwrap(state.autoHideDeadline), clock.now + OverlayState.autoHideDelaySeconds)
    clock.advance(by: OverlayState.autoHideDelaySeconds)
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closes, 1)
    XCTAssertEqual(state.presentationMode, .expanded, "a manual full panel is never collapsed")
  }

  /// Hover while the hide waits holds both timers; leaving re-arms both, and
  /// the hide again waits for the return instead of closing the full panel.
  func testHoverWhileTheHideWaitsRearmsBothAndKeepsTheOrder() {
    let (state, clock) = makeState()
    var closes = 0
    state.onClose = { closes += 1 }
    state.setPresentationMode(.mini)
    runTerminalTake(state)
    clock.advance(by: OverlayState.autoHideDelaySeconds)
    state.fireAutoHideNowForTests()
    XCTAssertTrue(state.autoHideAwaitsAutoCollapse)

    state.setPointerHovering(true)
    XCTAssertFalse(state.autoHideAwaitsAutoCollapse)
    XCTAssertNil(state.autoHideDeadline)
    clock.advance(by: 60)
    XCTAssertEqual(state.presentationMode, .expanded)

    state.setPointerHovering(false)
    clock.advance(by: OverlayState.autoHideDelaySeconds)
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closes, 0)
    XCTAssertTrue(state.autoHideAwaitsAutoCollapse)
    clock.advance(by: OverlayAutoCollapse.idleSeconds - OverlayState.autoHideDelaySeconds)
    XCTAssertEqual(state.presentationMode, .mini)
    clock.advance(by: OverlayState.autoHideDelaySeconds)
    state.fireAutoHideNowForTests()
    XCTAssertEqual(closes, 1)
  }

  /// A waiting hide belongs to its take. A successor capture drops it, so the
  /// predecessor's return cannot start a countdown against the live take.
  func testSuccessorCaptureDropsTheWaitingHide() {
    let (state, clock) = makeState()
    var closes = 0
    state.onClose = { closes += 1 }
    state.setPresentationMode(.mini)
    runTerminalTake(state, sessionId: "first-take")
    clock.advance(by: OverlayState.autoHideDelaySeconds)
    state.fireAutoHideNowForTests()
    XCTAssertTrue(state.autoHideAwaitsAutoCollapse)

    state.handleRecordingPreparing()
    XCTAssertFalse(state.autoHideAwaitsAutoCollapse)
    clock.advance(by: 60)
    XCTAssertEqual(state.presentationMode, .expanded, "the live capture holds the panel")
    XCTAssertNil(state.autoHideDeadline)
    XCTAssertEqual(closes, 0)
  }

  func testAutomaticTakeExpansionReturnsToMiniAfterTenIdleSeconds() {
    let (state, clock) = makeState()
    state.setPresentationMode(.mini)
    runTake(state)
    XCTAssertEqual(state.presentationMode, .expanded)
    XCTAssertEqual(state.autoCollapseRestoreMode, .mini)
    clock.advance(by: 9.5)
    XCTAssertEqual(state.presentationMode, .expanded)
    clock.advance(by: 0.5)
    XCTAssertEqual(state.presentationMode, .mini)
    XCTAssertNil(state.autoCollapseRestoreMode)
    XCTAssertEqual(clock.outstanding, 0)
  }

  func testAutomaticTakeExpansionReturnsToMidi() {
    let (state, clock) = makeState()
    state.setPresentationMode(.midi)
    runTake(state)
    XCTAssertEqual(state.presentationMode, .expanded)
    XCTAssertEqual(state.autoCollapseRestoreMode, .midi)
    clock.advance(by: 10)
    XCTAssertEqual(state.presentationMode, .midi)
  }

  func testLiveCaptureHoldsTheExpandedPanelWithoutATimer() {
    let (state, clock) = makeState()
    state.setPresentationMode(.mini)
    state.handleRecordingPreparing()
    clock.advance(by: 60)
    XCTAssertEqual(state.presentationMode, .expanded)
    XCTAssertEqual(clock.outstanding, 0)
    state.finishControllerRecording()
    XCTAssertEqual(clock.outstanding, 1)
    clock.advance(by: 10)
    XCTAssertEqual(state.presentationMode, .mini)
  }

  func testActivityMovesTheDeadlineWithOneOutstandingWake() {
    let (state, clock) = makeState()
    state.setPresentationMode(.midi)
    runTake(state)
    clock.advance(by: 6)
    state.userDraggedOverlay()
    state.noteComposerTypingActivity()
    XCTAssertEqual(clock.outstanding, 1)
    clock.advance(by: 6)
    XCTAssertEqual(state.presentationMode, .expanded, "the deadline moved with the activity")
    XCTAssertEqual(clock.outstanding, 1, "an early wake sleeps only its remainder")
    clock.advance(by: 3.5)
    XCTAssertEqual(state.presentationMode, .expanded)
    clock.advance(by: 0.5)
    XCTAssertEqual(state.presentationMode, .midi)
  }

  func testAgentCaptureKeepsTranscriptionExpandedThroughSilenceThenReturnsAfterStop() {
    for channel in ["0", "3"] {
      let (state, clock) = makeState()
      state.setPresentationMode(.mini)
      var roster = CsChannelRosterState(
        channel: channel, audience: channel == "0" ? "all" : "astra", provider: "codex",
        providerSessionId: "agent-capture", open: true, loud: false,
        autosealDeadlineUnixMs: nil, followerAlive: true)
      state.applyChannelRoster([roster])
      XCTAssertFalse(state.recording, "agent capture must not change dictation lifecycle")
      XCTAssertTrue(state.audioCaptureActive)
      XCTAssertEqual(state.presentationMode, .expanded)
      XCTAssertNil(state.autoCollapseDeadline)
      clock.advance(by: 60)
      XCTAssertEqual(state.presentationMode, .expanded, "silence is still an active take")
      state.applyChannelRoster([roster])
      XCTAssertNil(state.autoCollapseDeadline, "unchanged roster cannot arm an idle timer")

      roster.open = false
      state.applyChannelRoster([roster])
      XCTAssertFalse(state.audioCaptureActive)
      XCTAssertEqual(clock.outstanding, 1, "Stop starts the ordinary return interval")
      clock.advance(by: OverlayAutoCollapse.idleSeconds - 0.5)
      XCTAssertEqual(state.presentationMode, .expanded)
      clock.advance(by: 0.5)
      XCTAssertEqual(state.presentationMode, .mini)
    }
  }

  func testAgentCaptureCancelsPredecessorWakeUntilEveryChannelCloses() throws {
    let (state, clock) = makeState()
    state.setPresentationMode(.mini)
    runTake(state)
    let predecessor = try XCTUnwrap(clock.wakes.last)
    var first = CsChannelRosterState(
      channel: "2", audience: "lena", provider: "codex", providerSessionId: "lena-session",
      open: true, loud: false, autosealDeadlineUnixMs: nil, followerAlive: true)
    var second = CsChannelRosterState(
      channel: "3", audience: "astra", provider: "codex", providerSessionId: "astra-session",
      open: true, loud: false, autosealDeadlineUnixMs: nil, followerAlive: true)
    state.applyChannelRoster([first, second])
    XCTAssertTrue(predecessor.cancelled)
    predecessor.run()
    clock.advance(by: 60)
    XCTAssertEqual(state.presentationMode, .expanded)
    first.open = false
    state.applyChannelRoster([first, second])
    clock.advance(by: 60)
    XCTAssertEqual(state.presentationMode, .expanded, "remaining recipient still owns capture")
    second.open = false
    state.applyChannelRoster([first, second])
    clock.advance(by: OverlayAutoCollapse.idleSeconds)
    XCTAssertEqual(state.presentationMode, .mini)
  }

  func testSuccessorExpansionKeepsTheFirstMemoAndRefusesThePredecessorWake() throws {
    let (state, clock) = makeState()
    state.setPresentationMode(.mini)
    runTake(state)
    let predecessor = try XCTUnwrap(clock.wakes.last)
    clock.advance(by: 8)
    runTake(state)
    XCTAssertEqual(state.autoCollapseRestoreMode, .mini, "an expanded take never becomes the memo")
    XCTAssertTrue(predecessor.cancelled)
    predecessor.run()
    clock.advance(by: 9)
    XCTAssertEqual(state.presentationMode, .expanded, "the predecessor's wake cannot act")
    clock.advance(by: 1)
    XCTAssertEqual(state.presentationMode, .mini)
  }

  func testManualExpandedChoiceStaysExpanded() {
    let (state, clock) = makeState()
    state.setPresentationMode(.mini)
    runTake(state)
    state.setPresentationMode(.expanded)
    XCTAssertNil(state.autoCollapseRestoreMode)
    XCTAssertEqual(clock.outstanding, 0)
    clock.advance(by: 120)
    XCTAssertEqual(state.presentationMode, .expanded)
  }

  func testManualCompactChoiceSupersedesTheRestore() throws {
    let (state, clock) = makeState()
    state.setPresentationMode(.mini)
    runTake(state)
    let wake = try XCTUnwrap(clock.wakes.last)
    state.setPresentationMode(.midi)
    wake.run()
    clock.advance(by: 120)
    XCTAssertEqual(state.presentationMode, .midi)
    XCTAssertNil(state.autoCollapseRestoreMode)
  }

  func testUserExpansionThroughTheSurfaceIsNotUndone() {
    let (state, clock) = makeState()
    state.setPresentationMode(.mini)
    state.showTranscription()
    XCTAssertEqual(state.presentationMode, .expanded)
    XCTAssertNil(state.autoCollapseRestoreMode)
    clock.advance(by: 120)
    XCTAssertEqual(state.presentationMode, .expanded)
  }

  func testHoverHoldsAndItsEndArmsAFreshInterval() {
    let (state, clock) = makeState()
    state.setPresentationMode(.mini)
    runTake(state)
    clock.advance(by: 5)
    state.setPointerHovering(true)
    XCTAssertEqual(clock.outstanding, 0, "a held panel keeps no timer")
    clock.advance(by: 60)
    XCTAssertEqual(state.presentationMode, .expanded)
    state.setPointerHovering(false)
    clock.advance(by: 9.5)
    XCTAssertEqual(state.presentationMode, .expanded)
    clock.advance(by: 0.5)
    XCTAssertEqual(state.presentationMode, .mini)
  }

  func testComposerEditorHoldsUntilFocusLeaves() {
    let (state, clock) = makeState()
    state.setPresentationMode(.midi)
    runTake(state)
    state.noteComposerEditorActive(true)
    clock.advance(by: 30)
    XCTAssertEqual(state.presentationMode, .expanded)
    state.noteComposerEditorActive(false)
    clock.advance(by: 10)
    XCTAssertEqual(state.presentationMode, .midi)
  }

  func testWindowHoldAtTheDeadlineWaitsForTheInteractionToEnd() {
    let (state, clock) = makeState()
    var held = false
    state.autoCollapseExternalHold = { held }
    state.setPresentationMode(.mini)
    runTake(state)
    held = true  // e.g. a menu opened without any state-side signal
    clock.advance(by: 10)
    XCTAssertEqual(state.presentationMode, .expanded)
    XCTAssertEqual(clock.outstanding, 0)
    held = false
    state.noteAutoCollapseActivity()  // the menu closed
    clock.advance(by: 9)
    XCTAssertEqual(state.presentationMode, .expanded)
    clock.advance(by: 1)
    XCTAssertEqual(state.presentationMode, .mini)
  }

  func testSuspendedDeadlineCannotActAndMemoSurvives() throws {
    let (state, clock) = makeState()
    state.setPresentationMode(.mini)
    runTake(state)
    let wake = try XCTUnwrap(clock.wakes.last)
    state.suspendAutoCollapse()
    wake.run()
    clock.advance(by: 120)
    XCTAssertEqual(state.presentationMode, .expanded)
    XCTAssertEqual(state.autoCollapseRestoreMode, .mini)
    XCTAssertNil(state.autoCollapseDeadline)
  }

  func testTakeWithoutTranscriptByDefaultArmsNothing() {
    let (state, clock) = makeState(expandedByDefault: false)
    state.setPresentationMode(.mini)
    runTake(state)
    XCTAssertEqual(state.presentationMode, .midi)
    XCTAssertNil(state.autoCollapseRestoreMode)
    XCTAssertEqual(clock.outstanding, 0)
  }

  func testHiddenPanelHoldsAndHideRefusesTheShownPanelsWake() throws {
    let clock = ManualAutoCollapseScheduler()
    let state = OverlayState(
      nowProvider: { [unowned clock] in clock.now },
      autoSendEnabled: { false },
      micAccessProvider: { true },
      autoCollapseScheduler: clock.scheduler)
    let controller = OverlayController(
      state: state, engine: OverlayChromePolicyEngine(),
      overlayEnabledProvider: { true }, assistiveStatusProvider: { false },
      panelFactory: { _, _ in NSPanel() },
      orderPanelFront: { _ in }, orderPanelOut: { _ in })
    // The lifecycle callbacks drive AppModel; this test owns only presentation.
    state.onRecordingPreparing = nil
    state.onRecordingStopped = nil
    state.setPresentationMode(.mini)
    runTake(state)
    XCTAssertEqual(state.autoCollapseRestoreMode, .mini)
    XCTAssertEqual(clock.outstanding, 0, "a panel that was never shown holds the deadline")
    controller.show()
    XCTAssertEqual(clock.outstanding, 1)
    let wake = try XCTUnwrap(clock.wakes.last)
    controller.dismiss()
    XCTAssertTrue(wake.cancelled)
    wake.run()
    clock.advance(by: 120)
    XCTAssertEqual(state.presentationMode, .expanded)
    controller.show()
    clock.advance(by: 10)
    XCTAssertEqual(state.presentationMode, .mini)
  }
}
