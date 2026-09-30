import XCTest

@testable import Codescribe

@MainActor
final class AppTerminationCoordinatorTests: XCTestCase {
  func testRepliesWhenCleanupFinishes() async {
    let replied = expectation(description: "Quit reply")
    var count = 0
    let coordinator = AppTerminationCoordinator()
    coordinator.begin(
      cleanup: {},
      reply: {
        count += 1
        replied.fulfill()
      }
    )

    await fulfillment(of: [replied], timeout: 1)
    XCTAssertEqual(count, 1)
  }

  func testDeadlineRepliesOnceEvenIfCleanupFinishesLater() async {
    let replied = expectation(description: "bounded Quit reply")
    let cleanupGate = AsyncStream<Void>.makeStream()
    var count = 0
    let coordinator = AppTerminationCoordinator(deadline: {})
    coordinator.begin(
      cleanup: {
        for await _ in cleanupGate.stream { break }
      },
      reply: {
        count += 1
        replied.fulfill()
      }
    )

    await fulfillment(of: [replied], timeout: 1)
    cleanupGate.continuation.yield(())
    cleanupGate.continuation.finish()
    await Task.yield()
    XCTAssertEqual(count, 1)
  }
}
