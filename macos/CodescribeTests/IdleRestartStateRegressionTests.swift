import AppKit
import XCTest

@testable import Codescribe

@MainActor
final class IdleRestartStateRegressionTests: XCTestCase {
  func testRetainedOwnershipHoldsAutoCollapseWithoutBlockingIdleRestart() {
    XCTAssertFalse(OverlayState().founderComposing)
    let clock = ManualAutoCollapseScheduler()
    let state = OverlayState(
      nowProvider: { clock.now }, autoSendEnabled: { false },
      micAccessProvider: { true }, autoCollapseScheduler: clock.scheduler)
    let engine = OverlayChromePolicyEngine()
    engine.expanded = true
    engine.autoFoldSeconds = 10
    state.engine = engine
    state.attach()
    state.setPresentationMode(.mini)
    state.setPresentationMode(.expanded)
    state.noteComposerEditorActive(true)
    clock.advance(by: 30)
    XCTAssertEqual(state.presentationMode, .expanded, "Keep retained ownership for auto-collapse")
    XCTAssertFalse(state.founderComposing, "Ownership alone is not the current keyboard recipient")
    state.noteComposerEditorActive(false)
    clock.advance(by: 10)
    XCTAssertEqual(state.presentationMode, .mini)
  }

  func testTypingDeadlineAndUnselectedDraftStayProtected() {
    var now: TimeInterval = 1000
    let state = OverlayState(nowProvider: { now }, autoSendEnabled: { false })
    let generation = state.captureGeneration
    state.noteComposerTypingActivity()
    now = 1014.999
    XCTAssertTrue(state.founderComposing)
    now = 1015
    XCTAssertFalse(state.founderComposing, "The exact typing deadline has elapsed")
    state.conversationDrafts = ["unselected": "Retain these unsent words", "other": " \n"]
    now = 10000
    state.noteComposerEditorActive(false)
    XCTAssertTrue(
      state.founderComposing, "Every nonempty draft remains protected without a timeout")
    XCTAssertEqual(state.conversationDrafts["unselected"], "Retain these unsent words")
    state.conversationDrafts["unselected"] = " \n"
    XCTAssertFalse(state.founderComposing)
    XCTAssertEqual(state.captureGeneration, generation)
    XCTAssertTrue(state.pendingTextMessages.isEmpty)
  }

  func testCoordinatorRefusesUnsentWordsThenAdmitsRetainedBlankOwnership() async throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    defer { try? FileManager.default.removeItem(at: root) }
    let state = OverlayState(autoSendEnabled: { false })
    state.noteComposerEditorActive(true)
    var reservations = 0
    var terminations = 0
    let coordinator = try IdleRestartCoordinator(
      root: root, pid: 123, nonce: "fixture",
      acquireLease: { try FileHandle(forReadingFrom: URL(fileURLWithPath: "/dev/null")) },
      reserveCapture: {
        reservations += 1
        return true
      },
      founderComposing: { state.founderComposing }, scheduleFinalization: { $0() },
      terminate: { terminations += 1 })
    func request() throws -> (URL, URL) {
      let id = UUID().uuidString
      let payload = IdleRestartCoordinator.Request(
        schema: "codescribe.restart-if-idle.request.v1",
        request: id, pid: 123, nonce: "fixture", created: Date().timeIntervalSince1970)
      try JSONEncoder().encode(payload).write(to: root.appendingPathComponent("\(id).request.json"))
      return (
        try XCTUnwrap(URL(string: "codescribe://restart-if-idle?request=\(id)")),
        root.appendingPathComponent("\(id).receipt.json")
      )
    }
    state.conversationDrafts["another-owner"] = "Unsent"
    let first = try request()
    await coordinator.handle(first.0)
    let refused = try XCTUnwrap(
      JSONSerialization.jsonObject(with: Data(contentsOf: first.1)) as? [String: Any])
    XCTAssertEqual(refused["reason"] as? String, "founder_composing")
    XCTAssertEqual(reservations, 0)
    XCTAssertEqual(terminations, 0)
    XCTAssertEqual(state.conversationDrafts["another-owner"], "Unsent")
    state.conversationDrafts["another-owner"] = ""
    let second = try request()
    await coordinator.handle(second.0)
    let accepted = try XCTUnwrap(
      JSONSerialization.jsonObject(with: Data(contentsOf: second.1)) as? [String: Any])
    XCTAssertEqual(accepted["status"] as? String, "accepted")
    XCTAssertEqual(reservations, 1)
    XCTAssertEqual(terminations, 1, "Only the fixture closure runs, never NSApp.terminate")
  }
}
