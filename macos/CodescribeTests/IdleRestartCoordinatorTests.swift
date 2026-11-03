import Darwin
import Foundation
import XCTest

@testable import Codescribe

@MainActor
final class IdleRestartCoordinatorTests: XCTestCase {
  @MainActor
  private final class CaptureState {
    var busy = false
  }

  @MainActor
  private final class FinalizationQueue {
    var actions: [IdleRestartCoordinator.Finalization] = []

    func runNext() {
      precondition(!actions.isEmpty)
      actions.removeFirst()()
    }
  }

  private func fixture() throws -> URL {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    addTeardownBlock { try? FileManager.default.removeItem(at: root) }
    return root
  }

  private func request(_ root: URL, nonce: String = "generation", age: Double = 0) throws -> URL {
    let id = UUID().uuidString
    let request = IdleRestartCoordinator.Request(
      schema: "codescribe.restart-if-idle.request.v1", request: id, pid: 123,
      nonce: nonce, created: Date().timeIntervalSince1970 - age)
    try JSONEncoder().encode(request).write(to: root.appendingPathComponent("\(id).request.json"))
    return try XCTUnwrap(URL(string: "codescribe://restart-if-idle?request=\(id)"))
  }

  private func receipt(_ root: URL, _ url: URL) throws -> [String: Any] {
    let id = try XCTUnwrap(
      URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems?.first?.value)
    let bytes = try Data(contentsOf: root.appendingPathComponent("\(id).receipt.json"))
    return try XCTUnwrap(JSONSerialization.jsonObject(with: bytes) as? [String: Any])
  }

  func testBusyCaptureRefusesWithoutTerminationAndCanRetry() async throws {
    let root = try fixture()
    let capture = CaptureState()
    capture.busy = true
    var terminated = 0
    let coordinator = try IdleRestartCoordinator(
      root: root, pid: 123, nonce: "generation",
      acquireLease: { try FileHandle(forReadingFrom: URL(fileURLWithPath: "/dev/null")) },
      reserveCapture: { !capture.busy }, scheduleFinalization: { $0() },
      terminate: { terminated += 1 })
    let first = try request(root)
    await coordinator.handle(first)
    XCTAssertEqual(try receipt(root, first)["status"] as? String, "refused")
    XCTAssertEqual(terminated, 0)
    capture.busy = false
    let next = try request(root)
    await coordinator.handle(next)
    XCTAssertEqual(try receipt(root, next)["status"] as? String, "accepted")
    XCTAssertEqual(terminated, 1)
  }

  /// Maciej, 2026-10-23: a restart-if-idle quit the app under a long message
  /// he was typing. Writing to an agent refuses before the lease is taken,
  /// and words typed while the lease was being acquired refuse as well.
  func testFounderComposingRefusesBeforeAndAfterTheLease() async throws {
    let root = try fixture()
    let composing = CaptureState()
    composing.busy = true
    var reservations = 0
    var terminated = 0
    let coordinator = try IdleRestartCoordinator(
      root: root, pid: 123, nonce: "generation",
      acquireLease: { try FileHandle(forReadingFrom: URL(fileURLWithPath: "/dev/null")) },
      reserveCapture: {
        reservations += 1
        return true
      },
      founderComposing: { composing.busy }, scheduleFinalization: { $0() },
      terminate: { terminated += 1 })
    let typing = try request(root)
    await coordinator.handle(typing)
    XCTAssertEqual(try receipt(root, typing)["reason"] as? String, "founder_composing")
    XCTAssertEqual(reservations, 0, "a draft in progress never reaches capture reservation")
    XCTAssertEqual(terminated, 0)
    composing.busy = false
    let idle = try request(root)
    await coordinator.handle(idle)
    XCTAssertEqual(try receipt(root, idle)["status"] as? String, "accepted")
    XCTAssertEqual(terminated, 1)
  }

  /// Copilot on #184: typing that starts while the lease is being acquired
  /// must refuse before the reservation, which closes capture admission for
  /// good. A refusal after it would leave dictation disabled with no restart.
  func testTypingDuringTheLeaseWaitRefusesBeforeReservingCapture() async throws {
    let root = try fixture()
    let entered = expectation(description: "lease wait entered")
    var continuation: CheckedContinuation<Void, Never>?
    let composing = CaptureState()
    var reservations = 0
    var terminated = false
    let coordinator = try IdleRestartCoordinator(
      root: root, pid: 123, nonce: "generation",
      acquireLease: {
        await withCheckedContinuation {
          continuation = $0
          entered.fulfill()
        }
        return try FileHandle(forReadingFrom: URL(fileURLWithPath: "/dev/null"))
      },
      reserveCapture: {
        reservations += 1
        return true
      },
      founderComposing: { composing.busy }, scheduleFinalization: { $0() },
      terminate: { terminated = true })
    let url = try request(root)
    let pending = Task { await coordinator.handle(url) }
    await fulfillment(of: [entered], timeout: 1)
    composing.busy = true
    continuation?.resume()
    await pending.value
    XCTAssertEqual(try receipt(root, url)["reason"] as? String, "founder_composing")
    XCTAssertEqual(reservations, 0, "typing during the lease wait never reaches the reservation")
    XCTAssertFalse(terminated)
  }

  func testCaptureBeginningDuringLeaseCheckStillRefuses() async throws {
    let root = try fixture()
    let entered = expectation(description: "lease check entered")
    var continuation: CheckedContinuation<Void, Never>?
    let capture = CaptureState()
    var terminated = false
    let coordinator = try IdleRestartCoordinator(
      root: root, pid: 123, nonce: "generation",
      acquireLease: {
        await withCheckedContinuation {
          continuation = $0
          entered.fulfill()
        }
        return try FileHandle(forReadingFrom: URL(fileURLWithPath: "/dev/null"))
      }, reserveCapture: { !capture.busy }, scheduleFinalization: { $0() },
      terminate: { terminated = true })
    let url = try request(root)
    let pending = Task { await coordinator.handle(url) }
    await fulfillment(of: [entered], timeout: 1)
    capture.busy = true
    continuation?.resume()
    await pending.value
    XCTAssertFalse(terminated)
    XCTAssertEqual(try receipt(root, url)["reason"] as? String, "capture_busy_or_not_ready")
  }

  func testAcceptedRequestClosesAdmissionBeforeReceiptAndTermination() async throws {
    let root = try fixture()
    var closed = false
    var terminated = false
    let url = try request(root)
    let coordinator = try IdleRestartCoordinator(
      root: root, pid: 123, nonce: "generation",
      acquireLease: { try FileHandle(forReadingFrom: URL(fileURLWithPath: "/dev/null")) },
      reserveCapture: {
        closed = true
        return true
      },
      scheduleFinalization: { $0() },
      terminate: {
        XCTAssertTrue(closed)
        XCTAssertEqual(try? self.receipt(root, url)["status"] as? String, "accepted")
        terminated = true
      })
    await coordinator.handle(url)
    XCTAssertTrue(terminated)
  }

  func testReservationAndTerminationDoNotYieldToComposingActivity() async throws {
    let root = try fixture()
    let composing = CaptureState()
    let queue = FinalizationQueue()
    var pendingComposition: Task<Void, Never>?
    var terminated = false
    let coordinator = try IdleRestartCoordinator(
      root: root, pid: 123, nonce: "generation",
      acquireLease: { try FileHandle(forReadingFrom: URL(fileURLWithPath: "/dev/null")) },
      reserveCapture: {
        pendingComposition = Task { @MainActor in composing.busy = true }
        return true
      },
      founderComposing: { composing.busy },
      scheduleFinalization: { queue.actions.append($0) },
      terminate: {
        XCTAssertFalse(composing.busy, "reservation must not suspend before termination")
        terminated = true
      })
    let url = try request(root)
    await coordinator.handle(url)
    XCTAssertFalse(terminated)
    queue.runNext()
    XCTAssertTrue(terminated)
    XCTAssertEqual(try receipt(root, url)["status"] as? String, "accepted")
    await pendingComposition?.value
    XCTAssertTrue(composing.busy)
  }

  func testWrongGenerationAndExpiredRequestCannotReserveCapture() async throws {
    let root = try fixture()
    var reservations = 0
    let coordinator = try IdleRestartCoordinator(
      root: root, pid: 123, nonce: "generation",
      acquireLease: { try FileHandle(forReadingFrom: URL(fileURLWithPath: "/dev/null")) },
      reserveCapture: {
        reservations += 1
        return true
      }, scheduleFinalization: { $0() }, terminate: { XCTFail("Unexpected Quit") })
    await coordinator.handle(try request(root, nonce: "previous-process"))
    let expired = try request(root, age: 31)
    await coordinator.handle(expired)
    XCTAssertEqual(reservations, 0)
    XCTAssertEqual(try receipt(root, expired)["reason"] as? String, "expired_request")
  }

  func testUnavailableLeaseDoesNotReserveOrTerminate() async throws {
    let root = try fixture()
    var reservations = 0
    let coordinator = try IdleRestartCoordinator(
      root: root, pid: 123, nonce: "generation",
      acquireLease: { throw CocoaError(.fileReadUnknown) },
      reserveCapture: {
        reservations += 1
        return true
      }, scheduleFinalization: { $0() }, terminate: { XCTFail("Unexpected Quit") })
    let url = try request(root)
    await coordinator.handle(url)
    XCTAssertEqual(reservations, 0)
    XCTAssertEqual(try receipt(root, url)["reason"] as? String, "idle_lease_unavailable")
  }

  func testTypingAfterQueueingRefusesAndClosesLeaseBeforeAnyReservation() async throws {
    let root = try fixture()
    let composing = CaptureState()
    let queue = FinalizationQueue()
    var leaseFD: Int32 = -1
    var reservations = 0
    var terminated = 0
    let coordinator = try IdleRestartCoordinator(
      root: root, pid: 123, nonce: "generation",
      acquireLease: {
        let handle = try FileHandle(forReadingFrom: URL(fileURLWithPath: "/dev/null"))
        leaseFD = handle.fileDescriptor
        return handle
      },
      reserveCapture: {
        reservations += 1
        return true
      },
      founderComposing: { composing.busy },
      scheduleFinalization: { queue.actions.append($0) },
      terminate: { terminated += 1 })
    let first = try request(root)
    await coordinator.handle(first)
    XCTAssertEqual(queue.actions.count, 1)
    XCTAssertGreaterThanOrEqual(fcntl(leaseFD, F_GETFD), 0, "lease survives async handle return")
    XCTAssertEqual(try receipt(root, first)["status"] as? String, "checking")
    let concurrent = try request(root)
    await coordinator.handle(concurrent)
    XCTAssertEqual(try receipt(root, concurrent)["reason"] as? String, "restart_in_progress")
    XCTAssertEqual(queue.actions.count, 1)
    composing.busy = true
    queue.runNext()
    XCTAssertEqual(try receipt(root, first)["reason"] as? String, "founder_composing")
    XCTAssertEqual(fcntl(leaseFD, F_GETFD), -1)
    XCTAssertEqual(reservations, 0)
    XCTAssertEqual(terminated, 0)

    composing.busy = false
    let retry = try request(root)
    await coordinator.handle(retry)
    XCTAssertEqual(queue.actions.count, 1, "refusal releases handling for another request")
    queue.runNext()
    XCTAssertEqual(try receipt(root, retry)["status"] as? String, "accepted")
    XCTAssertEqual(reservations, 1)
    XCTAssertEqual(terminated, 1)
  }

  func testFinalizationChecksReservesAndTerminatesWithinTheSameCallback() async throws {
    let root = try fixture()
    let queue = FinalizationQueue()
    var events: [String] = []
    var leaseFD: Int32 = -1
    let url = try request(root)
    let coordinator = try IdleRestartCoordinator(
      root: root, pid: 123, nonce: "generation",
      acquireLease: {
        let handle = try FileHandle(forReadingFrom: URL(fileURLWithPath: "/dev/null"))
        leaseFD = handle.fileDescriptor
        return handle
      },
      reserveCapture: {
        events.append("reserve")
        return true
      },
      founderComposing: {
        events.append("composing")
        return false
      },
      scheduleFinalization: { queue.actions.append($0) },
      terminate: {
        XCTAssertEqual(try? self.receipt(root, url)["status"] as? String, "accepted")
        XCTAssertGreaterThanOrEqual(fcntl(leaseFD, F_GETFD), 0)
        events.append("terminate")
      })
    await coordinator.handle(url)
    XCTAssertEqual(events, ["composing", "composing"])
    XCTAssertEqual(queue.actions.count, 1)
    events.removeAll()
    queue.runNext()
    XCTAssertEqual(events, ["composing", "reserve", "terminate"])
    XCTAssertTrue(queue.actions.isEmpty, "successful reservation introduces no second dispatch")
    XCTAssertGreaterThanOrEqual(fcntl(leaseFD, F_GETFD), 0, "accepted lease stays held")
  }

  func testQueuedReservationRefusalOrThrowClosesLeaseAndAllowsRetry() async throws {
    for throwsOnReserve in [false, true] {
      let root = try fixture()
      let queue = FinalizationQueue()
      let refusal = CaptureState()
      refusal.busy = true
      var leaseFD: Int32 = -1
      var terminated = false
      let coordinator = try IdleRestartCoordinator(
        root: root, pid: 123, nonce: "generation",
        acquireLease: {
          let handle = try FileHandle(forReadingFrom: URL(fileURLWithPath: "/dev/null"))
          leaseFD = handle.fileDescriptor
          return handle
        },
        reserveCapture: {
          if refusal.busy && throwsOnReserve { throw CocoaError(.fileReadUnknown) }
          return !refusal.busy
        },
        scheduleFinalization: { queue.actions.append($0) },
        terminate: { terminated = true })
      let first = try request(root)
      await coordinator.handle(first)
      queue.runNext()
      XCTAssertFalse(terminated)
      XCTAssertEqual(fcntl(leaseFD, F_GETFD), -1)
      XCTAssertEqual(
        try receipt(root, first)["reason"] as? String,
        throwsOnReserve ? "idle_lease_unavailable" : "capture_busy_or_not_ready")
      refusal.busy = false
      let retry = try request(root)
      await coordinator.handle(retry)
      XCTAssertEqual(queue.actions.count, 1)
      queue.runNext()
      XCTAssertTrue(terminated)
      XCTAssertEqual(try receipt(root, retry)["status"] as? String, "accepted")
    }
  }

  func testAcceptedReceiptWriteFailureStillTerminatesAndRetainsLease() async throws {
    let root = try fixture()
    let queue = FinalizationQueue()
    let url = try request(root)
    let id = try XCTUnwrap(
      URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems?.first?.value)
    let receiptURL = root.appendingPathComponent("\(id).receipt.json")
    var terminated = false
    var leaseFD: Int32 = -1
    let coordinator = try IdleRestartCoordinator(
      root: root, pid: 123, nonce: "generation",
      acquireLease: {
        let handle = try FileHandle(forReadingFrom: URL(fileURLWithPath: "/dev/null"))
        leaseFD = handle.fileDescriptor
        return handle
      },
      reserveCapture: {
        // Initial checking succeeded. Make only the later accepted write fail
        // after admission has closed: a file cannot replace this directory.
        try FileManager.default.removeItem(at: receiptURL)
        try FileManager.default.createDirectory(at: receiptURL, withIntermediateDirectories: false)
        return true
      },
      scheduleFinalization: { queue.actions.append($0) },
      terminate: { terminated = true })
    await coordinator.handle(url)
    XCTAssertEqual(try receipt(root, url)["status"] as? String, "checking")
    queue.runNext()
    XCTAssertTrue(terminated, "receipt storage failure cannot undo accepted shutdown")
    XCTAssertTrue(queue.actions.isEmpty)
    XCTAssertGreaterThanOrEqual(fcntl(leaseFD, F_GETFD), 0, "accepted lease stays held")
    var isDirectory: ObjCBool = false
    XCTAssertTrue(
      FileManager.default.fileExists(atPath: receiptURL.path, isDirectory: &isDirectory))
    XCTAssertTrue(isDirectory.boolValue, "accepted receipt could not replace the directory")
  }
}
