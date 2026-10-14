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
    super.tearDown()
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

  private func isOneWay(_ values: [CGFloat]) -> Bool {
    let deltas = zip(values, values.dropFirst()).map { $1 - $0 }.filter { abs($0) > 0.5 }
    return deltas.allSatisfy { $0 > 0 } || deltas.allSatisfy { $0 < 0 }
  }

  func testAnimatedMorphsLedgerOneWriterPerGenerationAndLandOnTarget() throws {
    let (state, panel) = try makePanel("morph")
    panel.setFrameOrigin(NSPoint(x: 400, y: 400))
    panel.orderFrontRegardless()
    defer { panel.orderOut(nil) }
    panel.setPresentationMode(.mini)

    for mode in [OverlayPresentationMode.midi, .expanded, .mini] {
      let mark = trace.recorded
      panel.setPresentationMode(mode, animated: true)
      let began = try XCTUnwrap(
        trace.entries(after: mark).first { $0.kind == .frameTransitionBegan },
        "\(mode): the panel ledgers its target before animating")
      let generation = try XCTUnwrap(began.generation)
      let target = try XCTUnwrap(began.target)
      XCTAssertTrue(pump(untilCompleted: generation), "\(mode): the animator never landed")

      let after = trace.entries(after: mark)
      let completed = try XCTUnwrap(after.first { $0.kind == .frameTransitionCompleted })
      XCTAssertEqual(completed.generation, generation)
      XCTAssertEqual(completed.frame, target, "\(mode): the landing is the chosen target")
      XCTAssertNil(completed.note, "\(mode): no frame≠target note on a clean landing")
      XCTAssertEqual(panel.frame, target)

      let steps = after.filter { $0.kind == .windowResized && $0.generation == generation }
        .compactMap(\.frame)
      XCTAssertGreaterThanOrEqual(
        steps.count, 3, "\(mode): the animator paints intermediate frames")
      XCTAssertTrue(
        isOneWay(steps.map(\.width)), "\(mode): width steps back: \(steps.map(\.width))")
      XCTAssertTrue(
        isOneWay(steps.map(\.height)), "\(mode): height steps back: \(steps.map(\.height))")
      XCTAssertEqual(
        completed.at - began.at, DictationOverlayWindow.presentationDuration, accuracy: 0.25,
        "\(mode): the morph takes its declared duration")

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
    try? ("mini→midi→expanded→mini\n" + trace.dump() + "\n").write(
      to: dump, atomically: true, encoding: .utf8)
    withExtendedLifetime(state) {}
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
