import Foundation
import XCTest

@testable import Codescribe

@MainActor
final class IdleRestartCoordinatorTests: XCTestCase {
  @MainActor
  private final class CaptureState {
    var busy = false
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
      reserveCapture: { !capture.busy }, terminate: { terminated += 1 })
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
      founderComposing: { composing.busy }, terminate: { terminated += 1 })
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
      }, reserveCapture: { !capture.busy }, terminate: { terminated = true })
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
      terminate: {
        XCTAssertTrue(closed)
        XCTAssertEqual(try? self.receipt(root, url)["status"] as? String, "accepted")
        terminated = true
      })
    await coordinator.handle(url)
    XCTAssertTrue(terminated)
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
      }, terminate: { XCTFail("Unexpected Quit") })
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
      }, terminate: { XCTFail("Unexpected Quit") })
    let url = try request(root)
    await coordinator.handle(url)
    XCTAssertEqual(reservations, 0)
    XCTAssertEqual(try receipt(root, url)["reason"] as? String, "idle_lease_unavailable")
  }
}
