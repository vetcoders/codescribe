import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class OverlayIntercomProjectionTests: XCTestCase {
  private func controller() -> OverlayIntercomWidgetController {
    OverlayIntercomWidgetController(
      state: OverlayState(), textScale: TextScaleController(key: UUID().uuidString))
  }

  private func identity(session: String = "first", name: String = "Roman") throws
    -> OverlayIntercomCardIdentity
  {
    let owner = try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": session,
        "lease_id": AgentPlaybackIdentity.leaseIdentifier(provider: "codex", session: session),
        "channel": "3", "name": name,
      ]))
    return .init(
      digit: "3",
      conversation: .init(
        id: owner.id, channel: "3", name: name, owner: owner, messages: []))
  }

  private func settle() async throws {
    // Let the coordinator's queued MainActor task and its explicit yield finish.
    try await Task.sleep(for: .milliseconds(30))
  }

  func testRepeatedHostingUpdatesDoNotRepeatNativePlacement() async throws {
    let controller = controller()
    let card = try identity()
    let host = NSHostingView(
      rootView: OverlayIntercomWindowProjection(
        chips: [], identities: [card], controller: controller))
    host.frame = CGRect(x: 0, y: 0, width: 40, height: 40)
    let window = NSWindow(
      contentRect: CGRect(x: -10000, y: -10000, width: 40, height: 40),
      styleMask: [.borderless], backing: .buffered, defer: false)
    window.contentView = host
    defer { window.contentView = nil }
    host.layoutSubtreeIfNeeded()
    try await settle()
    XCTAssertGreaterThan(controller.reconciliationRequests, 0, "The representable must execute")
    let initial = controller.reconciliationRequests
    for _ in 0..<5 {
      host.rootView = OverlayIntercomWindowProjection(
        chips: [], identities: [card], controller: controller)
      host.layoutSubtreeIfNeeded()
      try await settle()
    }
    XCTAssertEqual(controller.reconciliationRequests, initial)
  }

  func testIdenticalProjectionUpdatesReconcileOnlyOnce() async throws {
    let controller = controller()
    let coordinator = OverlayIntercomWindowProjection.Coordinator()
    defer { coordinator.cancel() }
    let first = try identity()
    for _ in 0..<20 {
      coordinator.submit(chips: [], identities: [first], scale: 1, controller: controller)
    }
    try await settle()
    XCTAssertEqual(controller.reconciliationRequests, 1)
    coordinator.submit(chips: [], identities: [first], scale: 1, controller: controller)
    try await settle()
    XCTAssertEqual(controller.reconciliationRequests, 1)
  }

  func testOwnerLabelAndScaleChangesRemainVisibleBehindTheSameDigit() async throws {
    let controller = controller()
    let coordinator = OverlayIntercomWindowProjection.Coordinator()
    defer { coordinator.cancel() }
    let original = try identity()
    let replacement = try identity(session: "second")
    let renamed = try identity(session: "second", name: "A much longer name")
    XCTAssertNotEqual(original, replacement)
    XCTAssertEqual(replacement, renamed, "Display names deliberately are not card identity")
    for (index, inputs) in [(original, CGFloat(1)), (replacement, 1), (renamed, 1), (renamed, 1.5)]
      .enumerated()
    {
      coordinator.submit(chips: [], identities: [inputs.0], scale: inputs.1, controller: controller)
      try await settle()
      XCTAssertEqual(controller.reconciliationRequests, index + 1)
    }
  }

  func testBurstTargetsOnlyTheLatestController() async throws {
    let first = controller()
    let last = controller()
    let coordinator = OverlayIntercomWindowProjection.Coordinator()
    defer { coordinator.cancel() }
    coordinator.submit(chips: [], identities: [], scale: 1, controller: first)
    coordinator.submit(chips: [], identities: [], scale: 1.5, controller: last)
    try await settle()
    XCTAssertEqual(first.reconciliationRequests, 0)
    XCTAssertEqual(last.reconciliationRequests, 1)
    coordinator.submit(chips: [], identities: [], scale: 1.5, controller: last)
    try await settle()
    XCTAssertEqual(last.reconciliationRequests, 1)
  }

  func testDismantleCancelsPendingWorkAndAllowsAFreshSubmission() async throws {
    let controller = controller()
    let coordinator = OverlayIntercomWindowProjection.Coordinator()
    coordinator.submit(chips: [], identities: [], scale: 1, controller: controller)
    OverlayIntercomWindowProjection.dismantleNSView(NSView(), coordinator: coordinator)
    try await settle()
    XCTAssertEqual(controller.reconciliationRequests, 0)
    coordinator.submit(chips: [], identities: [], scale: 1, controller: controller)
    try await settle()
    XCTAssertEqual(controller.reconciliationRequests, 1)
    coordinator.cancel()
  }

  func testPendingProjectionDoesNotRetainTheWindowOwner() async throws {
    let coordinator = OverlayIntercomWindowProjection.Coordinator()
    var owner: OverlayIntercomWidgetController? = controller()
    weak let weakOwner = owner
    coordinator.submit(chips: [], identities: [], scale: 1, controller: owner)
    owner = nil
    XCTAssertNil(weakOwner)
    try await settle()
    coordinator.cancel()
  }

  func testScreenVisibleFrameChangesStillReconcileWithIdenticalCards() async throws {
    let controller = controller()
    let coordinator = OverlayIntercomWindowProjection.Coordinator()
    defer { coordinator.cancel() }
    let card = try identity()
    for (index, height) in [CGFloat(900), 850, 850].enumerated() {
      coordinator.submit(
        chips: [], identities: [card], scale: 1, controller: controller,
        visibleFrames: [CGRect(x: 0, y: 0, width: 1440, height: height)])
      try await settle()
      XCTAssertEqual(controller.reconciliationRequests, min(index + 1, 2))
    }
  }

  func testChangedChipStateReachesExistingController() async throws {
    let controller = controller()
    let coordinator = OverlayIntercomWindowProjection.Coordinator()
    defer { coordinator.cancel() }
    for (index, open) in [false, true, true].enumerated() {
      let chip = OverlayChannelMapChip(
        digit: "3", name: "Roman", disconnected: false, microphoneOpen: open,
        unread: false, selectable: true, selected: false)
      coordinator.submit(chips: [chip], identities: [], scale: 1, controller: controller)
      try await settle()
      XCTAssertEqual(controller.reconciliationRequests, min(index + 1, 2))
    }
  }
}
