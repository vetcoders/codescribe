import AppKit
import XCTest

@testable import Codescribe

@MainActor
final class SystemSleepWakeObserverTests: XCTestCase {
  func testWorkspaceSleepAndWakeCoalesceIntoRecorderLifecycleCallbacks() async {
    let center = NotificationCenter()
    let workspace = NSWorkspace.shared
    var deliveries = 0
    var nextDelivery = expectation(description: "sleep boundary delivered")
    let observer = SystemSleepWakeObserver(center: center, workspace: workspace) {
      deliveries += 1
      nextDelivery.fulfill()
    }
    observer.start()

    // A notification storm inside one run-loop turn becomes one bridge hop.
    center.post(name: NSWorkspace.willSleepNotification, object: workspace)
    center.post(name: NSWorkspace.willSleepNotification, object: workspace)
    await fulfillment(of: [nextDelivery], timeout: 1)
    XCTAssertEqual(deliveries, 1)

    nextDelivery = expectation(description: "wake boundary delivered")
    center.post(name: NSWorkspace.didWakeNotification, object: workspace)
    await fulfillment(of: [nextDelivery], timeout: 1)
    XCTAssertEqual(deliveries, 2)

    observer.invalidate()
  }
}
