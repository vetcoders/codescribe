import AppKit
import XCTest

@testable import Codescribe

/// The presentation ledger is the forensic instrument for the overlay's
/// mini ⇄ midi ⇄ expanded morphs (Maciej, 2026-10-14: they sometimes jump).
/// These tests drive the real panel through animated morphs and read the
/// ledger the way a report would: one writer per generation, a one-way
/// trajectory, a landing on the chosen target, and no second hand on the
/// frame after the landing.
@MainActor
final class OverlayPresentationTraceTests: XCTestCase {
  private var trace: OverlayPresentationTrace { .shared }

  override func setUp() {
    super.setUp()
    trace.mirrorsToLog = false
    trace.reset()
  }

  override func tearDown() {
    trace.clock = { ProcessInfo.processInfo.systemUptime }
    trace.mirrorsToLog = true
    trace.diagnosticSink = nil
    super.tearDown()
  }

  func testDiagnosticSinkPersistsTheLedgerLine() throws {
    let url = FileManager.default.temporaryDirectory
      .appendingPathComponent("OverlayPresentationTraceTests-\(UUID().uuidString).log")
    defer { try? FileManager.default.removeItem(at: url) }
    let sink = DiagnosticLog(url: url)
    trace.diagnosticSink = sink
    trace.record(.modeRequested, from: .mini, mode: .midi, note: "user")
    let lines = try sink.lines()
    XCTAssertEqual(lines.count, 1)
    XCTAssertTrue(lines[0].hasSuffix(" overlay-presentation mode.requested mini→midi user"))
  }

  private func makePanel(_ name: String) throws -> (OverlayState, FloatingOverlayPanel) {
    let state = OverlayState()
    let panel = try XCTUnwrap(
      DictationOverlayWindow.make(
        state: state, textScale: TextScaleController(key: "OverlayPresentationTraceTests.\(name)"))
        as? FloatingOverlayPanel)
    return (state, panel)
  }

  /// Pumps the main run loop until the ledger holds the landing of `generation`.
  private func pump(untilCompleted generation: UInt64, deadline: TimeInterval = 2) -> Bool {
    let end = Date().addingTimeInterval(deadline)
    while Date() < end {
      if trace.entries.contains(where: {
        $0.kind == .frameTransitionCompleted && $0.generation == generation
      }) {
        return true
      }
      RunLoop.main.run(until: Date().addingTimeInterval(0.01))
    }
    return false
  }

  /// WindowServer visibility and the hosting layout settle on run-loop events.
  /// A painted-frame measurement starts only after those events, not a fixed delay.
  private func pump(untilReady panel: FloatingOverlayPanel, deadline: TimeInterval = 2) -> Bool {
    let end = Date().addingTimeInterval(deadline)
    while Date() < end {
      if panel.occlusionState.contains(.visible), panel.contentView?.needsLayout == false {
        return true
      }
      RunLoop.main.run(until: Date().addingTimeInterval(0.01))
    }
    return false
  }

  private func isOneWay(_ values: [CGFloat]) -> Bool {
    let deltas = zip(values, values.dropFirst()).map { $1 - $0 }.filter { abs($0) > 0.5 }
    return deltas.allSatisfy { $0 > 0 } || deltas.allSatisfy { $0 < 0 }
  }

  private func panelReadiness(_ panel: FloatingOverlayPanel, label: String) -> String {
    let visibleOcc = panel.occlusionState.contains(.visible)
    let needsLayout = panel.contentView?.needsLayout ?? false
    let needsDisplay = panel.contentView?.needsDisplay ?? false
    return "\(label){frame=\(NSStringFromRect(panel.frame)),isVisible=\(panel.isVisible),"
      + "occVisible=\(visibleOcc)(raw=\(panel.occlusionState.rawValue)),"
      + "winNum=\(panel.windowNumber),viewsNeedDisplay=\(panel.viewsNeedDisplay),"
      + "needsLayout=\(needsLayout),needsDisplay=\(needsDisplay),"
      + "style=\(panel.styleMask.rawValue),rec=\(trace.recorded)}"
  }

  private func formatTraceEntries(_ entries: [OverlayPresentationTrace.Entry]) -> String {
    let t0 = trace.entries.first?.at ?? entries.first?.at ?? 0
    return entries.map { entry in
      let ms = String(format: "%.2fms", (entry.at - t0) * 1000)
      return "\(ms):\(OverlayPresentationTrace.line(entry))"
    }.joined(separator: " | ")
  }

  func testAnimatedMorphsLedgerOneWriterPerGenerationAndLandOnTarget() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let (state, panel) = try makePanel("morph")
    let afterMake = panelReadiness(panel, label: "afterMake")
    panel.setFrameOrigin(NSPoint(x: 400, y: 400))
    let beforeOrderFront = panelReadiness(panel, label: "beforeOrderFront")
    BackgroundTestWindow.present(panel)
    defer { panel.orderOut(nil) }
    let afterOrderFront = panelReadiness(panel, label: "afterOrderFront")
    panel.setPresentationMode(.mini)
    // Synchronous display alone can leave both visibility and hosting layout
    // pending. Observe the real readiness events before counting painted frames.
    panel.contentView?.layoutSubtreeIfNeeded()
    panel.displayIfNeeded()
    let ready = pump(untilReady: panel)
    XCTAssertTrue(
      ready, "Initial mini never became ready: \(panelReadiness(panel, label: "timeout"))")
    guard ready else { return }
    XCTAssertEqual(panel.frame.size, DictationOverlayWindow.collapsedSize)
    let afterSetMini = panelReadiness(panel, label: "afterSetMini")
    let initialTrace = formatTraceEntries(trace.entries)
    let initialReadiness =
      "init=[\(afterMake); \(beforeOrderFront); \(afterOrderFront); \(afterSetMini); "
      + "reduceMotion=\(NSWorkspace.shared.accessibilityDisplayShouldReduceMotion); "
      + "appActive=\(NSApp.isActive); initTrace=(\(initialTrace))]"

    for mode in [OverlayPresentationMode.midi, .expanded, .mini] {
      let mark = trace.recorded
      let preMorph = panelReadiness(panel, label: "preMorph(\(mode))")
      panel.setPresentationMode(mode, animated: true)
      let postStart = panelReadiness(panel, label: "postStart(\(mode))")
      let began = try XCTUnwrap(
        trace.entries(after: mark).first { $0.kind == .frameTransitionBegan },
        "\(mode): the panel ledgers its target before animating")
      let generation = try XCTUnwrap(began.generation)
      let target = try XCTUnwrap(began.target)
      XCTAssertTrue(pump(untilCompleted: generation), "\(mode): the animator never landed")

      let after = trace.entries(after: mark)
      let completed = try XCTUnwrap(after.first { $0.kind == .frameTransitionCompleted })
      let postMorph = panelReadiness(panel, label: "postMorph(\(mode))")
      XCTAssertEqual(completed.generation, generation)
      XCTAssertEqual(completed.frame, target, "\(mode): the landing is the chosen target")
      XCTAssertNil(completed.note, "\(mode): no frame≠target note on a clean landing")
      XCTAssertEqual(panel.frame, target)

      let steps = after.filter { $0.kind == .windowResized && $0.generation == generation }
        .compactMap(\.frame)
      let declared = DictationOverlayWindow.presentationDuration(
        from: try XCTUnwrap(began.frame), to: target)
      let elapsed = completed.at - began.at
      let diag =
        "diag={beganFrame=\(began.frame.map(NSStringFromRect) ?? "nil"), "
        + "beganTarget=\(NSStringFromRect(target)), "
        + "completedFrame=\(completed.frame.map(NSStringFromRect) ?? "nil"), "
        + "actualFrame=\(NSStringFromRect(panel.frame)), "
        + "steps=\(steps.count), elapsed=\(String(format: "%.6f", elapsed))s, "
        + "declared=\(String(format: "%.6f", declared))s, "
        + "lastMorphDuration=\(String(format: "%.6f", panel.lastMorphDuration))s, "
        + "\(preMorph), \(postStart), \(postMorph), "
        + "afterMark=(\(formatTraceEntries(after))), \(initialReadiness)}"
      XCTAssertGreaterThanOrEqual(
        steps.count, 3, "\(mode): the animator paints intermediate frames -- \(diag)")
      XCTAssertTrue(
        isOneWay(steps.map(\.width)), "\(mode): width steps back: \(steps.map(\.width))")
      XCTAssertTrue(
        isOneWay(steps.map(\.height)), "\(mode): height steps back: \(steps.map(\.height))")
      XCTAssertEqual(
        elapsed, declared, accuracy: 0.25,
        "\(mode): the morph takes its declared duration -- \(diag)")

      // Nothing rewrites the frame once the morph has landed.
      RunLoop.main.run(until: Date().addingTimeInterval(0.1))
      let afterLanding = trace.entries(after: mark)
        .drop(while: { $0.kind != .frameTransitionCompleted }).dropFirst()
      let secondHands = afterLanding.filter { entry in
        switch entry.kind {
        case .placementApplied, .contentResize, .frameTransitionBegan: true
        case .windowResized, .windowMoved: entry.frame != target
        default: false
        }
      }
      XCTAssertTrue(
        secondHands.isEmpty,
        "\(mode): a second hand moved the frame after landing:\n"
          + secondHands.map(OverlayPresentationTrace.line).joined(separator: "\n"))
    }
    // The painted trajectory, for the report that reads this run. The test
    // host's stdout never reaches the xcodebuild log, so it is a file.
    let dump = URL(fileURLWithPath: NSTemporaryDirectory())
      .appendingPathComponent("overlay-presentation-ledger.txt")
    try? (initialReadiness + "\nmini→midi→expanded→mini\n" + trace.dump() + "\n").write(
      to: dump, atomically: true, encoding: .utf8)
    withExtendedLifetime(state) {}
  }

  /// Founder, 2026-10-15: a full-screen overlay folded in the time MIDI folds
  /// to mini. The morph keeps one speed: duration follows the longest edge travel.
  func testMorphDurationFollowsTheFarthestEdgeAtOneSpeed() {
    let midi = NSRect(x: 1590, y: 1040, width: 410, height: 46)
    let mini = NSRect(x: 1800, y: 1040, width: 200, height: 46)
    XCTAssertEqual(
      DictationOverlayWindow.presentationDuration(from: midi, to: mini),
      DictationOverlayWindow.presentationDuration, "A short step keeps the former pace")
    // Maciej, 2026-10-19: "nie tak wolne". A full-screen fold stops at the ceiling.
    let fullScreen = NSRect(x: 40, y: 40, width: 1960, height: 1046)
    let fold = DictationOverlayWindow.presentationDuration(from: fullScreen, to: mini)
    XCTAssertEqual(fold, DictationOverlayWindow.longestPresentationDuration, accuracy: 0.001)
    XCTAssertEqual(
      DictationOverlayWindow.presentationDuration(from: mini, to: fullScreen), fold,
      "Unfolding the same distance takes as long")
    let defaultPanel = NSRect(x: 1530, y: 760, width: 470, height: 326)
    XCTAssertEqual(
      DictationOverlayWindow.presentationDuration(from: defaultPanel, to: mini),
      DictationOverlayWindow.presentationDuration, "The default panel folds as before")
    let largePanel = NSRect(x: 1100, y: 500, width: 900, height: 586)
    let partial = DictationOverlayWindow.presentationDuration(from: largePanel, to: mini)
    XCTAssertEqual(partial, 700 / DictationOverlayWindow.presentationSpeed, accuracy: 0.001)
    XCTAssertLessThan(partial, fold, "A shorter fold is quicker, at the same speed")
    // The unroll from Maciej's ledger (13:40): 1149 pt, declared at 1.15 s before.
    let parked = NSRect(x: 1516, y: 1026, width: 200, height: 46)
    let expanded = NSRect(x: 367, y: 481, width: 1349, height: 591)
    XCTAssertEqual(
      DictationOverlayWindow.presentationDuration(from: parked, to: expanded),
      DictationOverlayWindow.longestPresentationDuration, accuracy: 0.001)
  }

  /// Maciej, 2026-10-19: the unroll into the expanded view laid the whole panel
  /// out again on every animator step. The content takes its landing size at
  /// once, held at the window's top-right corner, and the window reveals it.
  func testUnrollHoldsTheLandingLayoutAtTheTopRightCorner() throws {
    let (state, panel) = try makePanel("unroll")
    panel.setFrameOrigin(NSPoint(x: 400, y: 400))
    BackgroundTestWindow.present(panel)
    defer { panel.orderOut(nil) }
    panel.setPresentationMode(.mini)
    let root = try XCTUnwrap(panel.contentView as? OverlayContentContainer)
    XCTAssertEqual(root.contentFrame, root.bounds, "a resting panel fills its window")

    let mark = trace.recorded
    panel.setPresentationMode(.expanded, animated: true)
    let began = try XCTUnwrap(
      trace.entries(after: mark).first { $0.kind == .frameTransitionBegan })
    XCTAssertEqual(began.note, "unroll")
    let target = try XCTUnwrap(began.target)
    XCTAssertEqual(root.pinnedContentSize, target.size)
    XCTAssertEqual(root.contentFrame.size, target.size, "the landing size before the first step")

    RunLoop.main.run(until: Date().addingTimeInterval(panel.lastMorphDuration / 2))
    XCTAssertLessThan(panel.frame.width, target.width, "the unroll is under way")
    XCTAssertEqual(root.contentFrame.size, target.size, "a step does not lay the content out")
    XCTAssertEqual(root.contentFrame.maxX, root.bounds.maxX, accuracy: 0.5)
    XCTAssertEqual(root.contentFrame.maxY, root.bounds.maxY, accuracy: 0.5)

    XCTAssertTrue(pump(untilCompleted: try XCTUnwrap(began.generation)))
    XCTAssertEqual(panel.frame, target)
    XCTAssertNil(root.pinnedContentSize)
    XCTAssertEqual(root.contentFrame, root.bounds, "the landed panel fills its window again")
    XCTAssertEqual(root.layer?.masksToBounds, false)
    withExtendedLifetime(state) {}
  }

  /// The fold is the unroll's mirror: the departing layout keeps its size,
  /// held at the top-right corner, while the shrinking window covers it. No
  /// animator step lays the content out again, and releasing the hold lands
  /// on the settled frame in the same pass — no end-of-transition jump.
  func testFoldHoldsTheDepartingLayoutAtTheTopRightCorner() throws {
    let (state, panel) = try makePanel("fold")
    panel.setFrameOrigin(NSPoint(x: 400, y: 400))
    BackgroundTestWindow.present(panel)
    defer { panel.orderOut(nil) }
    panel.setPresentationMode(.expanded)
    let root = try XCTUnwrap(panel.contentView as? OverlayContentContainer)
    let departing = panel.frame
    XCTAssertEqual(root.contentFrame, root.bounds, "a resting panel fills its window")

    let mark = trace.recorded
    panel.setPresentationMode(.mini, animated: true)
    let began = try XCTUnwrap(
      trace.entries(after: mark).first { $0.kind == .frameTransitionBegan })
    XCTAssertEqual(began.note, "fold")
    XCTAssertEqual(root.pinnedContentSize, departing.size)
    XCTAssertEqual(root.contentFrame.size, departing.size)

    RunLoop.main.run(until: Date().addingTimeInterval(panel.lastMorphDuration / 2))
    XCTAssertLessThan(panel.frame.height, departing.height, "the fold is under way")
    XCTAssertGreaterThan(
      panel.frame.height, DictationOverlayWindow.collapsedHeight, "the fold has not landed yet")
    XCTAssertEqual(root.contentFrame.size, departing.size, "a step does not lay the content out")
    XCTAssertEqual(root.contentFrame.maxX, root.bounds.maxX, accuracy: 0.5)
    XCTAssertEqual(root.contentFrame.maxY, root.bounds.maxY, accuracy: 0.5)

    XCTAssertTrue(pump(untilCompleted: try XCTUnwrap(began.generation)))
    XCTAssertEqual(panel.frame.size, panel.miniSize)
    XCTAssertNil(root.pinnedContentSize)
    XCTAssertEqual(root.contentFrame, root.bounds, "the landed panel fills its window again")
    XCTAssertEqual(root.layer?.masksToBounds, false)

    // Reduce Motion takes the unanimated path: no hold is ever taken.
    panel.setPresentationMode(.expanded)
    panel.setPresentationMode(.mini)
    XCTAssertNil(root.pinnedContentSize)
    XCTAssertEqual(root.contentFrame, root.bounds)
    withExtendedLifetime(state) {}
  }

  /// A fold that catches an unroll keeps the size the unroll already holds:
  /// the layout on screen never changes size mid-flight, in either direction.
  /// The fold then covers that held layout until it lands on mini.
  func testFoldCatchingAnUnrollKeepsTheHeldLayout() throws {
    let (_, panel) = try makePanel("unroll-caught")
    panel.setFrameOrigin(NSPoint(x: 400, y: 400))
    BackgroundTestWindow.present(panel)
    defer { panel.orderOut(nil) }
    panel.setPresentationMode(.mini)
    let root = try XCTUnwrap(panel.contentView as? OverlayContentContainer)
    panel.setPresentationMode(.expanded, animated: true)
    let held = try XCTUnwrap(root.pinnedContentSize)
    RunLoop.main.run(until: Date().addingTimeInterval(panel.lastMorphDuration / 3))

    let mark = trace.recorded
    panel.setPresentationMode(.mini, animated: true)
    XCTAssertEqual(root.pinnedContentSize, held, "the held layout carries into the fold")
    XCTAssertEqual(root.contentFrame.size, held)
    XCTAssertEqual(root.layer?.masksToBounds, true)
    let began = try XCTUnwrap(
      trace.entries(after: mark).first { $0.kind == .frameTransitionBegan })
    XCTAssertEqual(began.note, "fold")

    RunLoop.main.run(until: Date().addingTimeInterval(panel.lastMorphDuration / 2))
    XCTAssertEqual(root.contentFrame.size, held, "a step does not lay the content out")
    XCTAssertEqual(root.contentFrame.maxX, root.bounds.maxX, accuracy: 0.5)
    XCTAssertEqual(root.contentFrame.maxY, root.bounds.maxY, accuracy: 0.5)

    XCTAssertTrue(pump(untilCompleted: try XCTUnwrap(began.generation)))
    XCTAssertEqual(panel.frame.size, panel.miniSize)
    XCTAssertNil(root.pinnedContentSize)
    XCTAssertEqual(root.contentFrame, root.bounds)
    XCTAssertEqual(root.layer?.masksToBounds, false)
  }

  /// A caller that settles an unroll before its first step lands it once; the
  /// deferred start then finds nothing left to animate.
  func testSettlingAnUnrollBeforeItsFirstStepLandsItOnce() throws {
    let (_, panel) = try makePanel("unroll-settled")
    panel.setFrameOrigin(NSPoint(x: 400, y: 400))
    BackgroundTestWindow.present(panel)
    defer { panel.orderOut(nil) }
    panel.setPresentationMode(.mini)
    let root = try XCTUnwrap(panel.contentView as? OverlayContentContainer)

    let mark = trace.recorded
    panel.setPresentationMode(.expanded, animated: true)
    let target = try XCTUnwrap(
      trace.entries(after: mark).first { $0.kind == .frameTransitionBegan }?.target)
    panel.settleFrameTransition()
    XCTAssertEqual(panel.frame, target)
    XCTAssertFalse(panel.isFrameTransitioning)
    XCTAssertNil(root.pinnedContentSize)

    RunLoop.main.run(until: Date().addingTimeInterval(0.2))
    XCTAssertEqual(panel.frame, target)
    XCTAssertEqual(
      trace.entries(after: mark).filter { $0.kind == .frameTransitionCompleted }.count, 1)
  }

  /// Founder, 2026-10-15: a fold caught by the pointer returns to the frame
  /// the panel had before the fold started, from wherever the morph had got to.
  func testFoldCaughtMidwayReturnsToThePreFoldFrame() throws {
    let (_, panel) = try makePanel("fold-caught")
    panel.setFrameOrigin(NSPoint(x: 400, y: 400))
    BackgroundTestWindow.present(panel)
    defer { panel.orderOut(nil) }
    panel.setPresentationMode(.expanded)
    let before = panel.frame

    panel.setPresentationMode(.mini, animated: true)
    let fold = panel.lastMorphDuration
    RunLoop.main.run(until: Date().addingTimeInterval(fold / 3))
    let caught = panel.frame
    XCTAssertLessThan(caught.height, before.height, "the fold is under way")
    XCTAssertGreaterThan(caught.height, DictationOverlayWindow.collapsedHeight)

    let mark = trace.recorded
    panel.setPresentationMode(.expanded, animated: true)
    XCTAssertLessThanOrEqual(panel.lastMorphDuration, fold, "the way back is shorter")
    let began = try XCTUnwrap(
      trace.entries(after: mark).first { $0.kind == .frameTransitionBegan })
    XCTAssertEqual(began.target, before, "the target is the pre-fold frame")
    XCTAssertTrue(pump(untilCompleted: try XCTUnwrap(began.generation)))
    XCTAssertEqual(panel.frame, before)
  }

  func testModeRequestPrecedesTheFrameAndCarriesItsCause() throws {
    let (state, panel) = try makePanel("cause")
    let other: OverlayPresentationMode = state.presentationMode == .midi ? .mini : .midi
    let mark = trace.recorded
    state.setPresentationMode(other)
    let kinds = trace.entries(after: mark).map(\.kind)
    XCTAssertEqual(kinds.first, .modeRequested)
    XCTAssertEqual(trace.entries(after: mark).first?.note, "user")
    let request = try XCTUnwrap(kinds.firstIndex(of: .modeRequested))
    let began = try XCTUnwrap(kinds.firstIndex(of: .frameTransitionBegan))
    XCTAssertLessThan(request, began, "the state asks before the window moves")
    XCTAssertEqual(panel.frame.size, DictationOverlayWindow.minimumSize(for: other))

    // A repeated request is ledgered as such and moves nothing.
    let again = trace.recorded
    state.setPresentationMode(other)
    let repeated = trace.entries(after: again)
    XCTAssertEqual(repeated.map(\.kind), [.modeRequested])
    XCTAssertEqual(repeated.first?.note, "user same")
  }

  func testDumpReadsAsRelativeMillisecondsOldestFirst() {
    var now: TimeInterval = 10
    trace.clock = { now }
    trace.record(.modeRequested, from: .mini, mode: .midi, note: "user")
    now += 0.2805
    trace.record(
      .frameTransitionCompleted, mode: .midi,
      frame: NSRect(x: 1, y: 2, width: 410, height: 46), generation: 7)
    let lines = trace.dump().split(separator: "\n").map(String.init)
    XCTAssertEqual(
      lines,
      [
        "     0 ms  mode.requested mini→midi user",
        "   281 ms  frame.completed midi gen=7 frame=1,2 410x46",
      ])
  }

  func testRingKeepsTheNewestEntriesAndCountsEveryRecord() {
    let capacity = OverlayPresentationTrace.capacity
    for i in 0..<(capacity + 5) { trace.record(.windowResized, note: "\(i)") }
    XCTAssertEqual(trace.entries.count, capacity)
    XCTAssertEqual(trace.recorded, capacity + 5)
    XCTAssertEqual(trace.entries.first?.note, "5")
    XCTAssertEqual(
      trace.entries(after: trace.recorded - 2).map(\.note), ["\(capacity + 3)", "\(capacity + 4)"])
    XCTAssertEqual(trace.entries(after: 0).count, capacity, "entries lost to the ring stay lost")
  }
}
