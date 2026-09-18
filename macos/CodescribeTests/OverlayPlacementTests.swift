import XCTest

@testable import Codescribe

/// Pure-geometry contract for the overlay's anchored placement: six anchors
/// over a visible frame, margin respected, and the free-motion clamp keeping a
/// restored origin fully on-screen after display changes.
final class OverlayPlacementTests: XCTestCase {
  // A visible frame with a non-zero origin, as on a secondary display —
  // anchor math must respect minX/minY, not assume (0,0).
  private let visible = NSRect(x: 100, y: 50, width: 1600, height: 900)
  private let size = NSSize(width: 320, height: 600)
  private let m = OverlayPlacement.margin

  func testTopAnchorsSitUnderTheVisibleTopEdge() {
    for anchor in [OverlayAnchor.topLeft, .topCenter, .topRight] {
      let origin = OverlayPlacement.origin(for: anchor, size: size, in: visible)
      XCTAssertEqual(origin.y, visible.maxY - size.height - m, "\(anchor)")
    }
  }

  func testBottomAnchorsSitOnTheVisibleBottomEdge() {
    for anchor in [OverlayAnchor.bottomLeft, .bottomCenter, .bottomRight] {
      let origin = OverlayPlacement.origin(for: anchor, size: size, in: visible)
      XCTAssertEqual(origin.y, visible.minY + m, "\(anchor)")
    }
  }

  func testHorizontalLanesLeftCenterRight() {
    let left = OverlayPlacement.origin(for: .topLeft, size: size, in: visible)
    let center = OverlayPlacement.origin(for: .topCenter, size: size, in: visible)
    let right = OverlayPlacement.origin(for: .topRight, size: size, in: visible)
    XCTAssertEqual(left.x, visible.minX + m)
    XCTAssertEqual(center.x, visible.midX - size.width / 2)
    XCTAssertEqual(right.x, visible.maxX - size.width - m)
  }

  func testDefaultAnchorIsTopRightUnderTheTray() {
    XCTAssertEqual(OverlayPlacement.defaultAnchor, .topRight)
  }

  func testEveryAnchorKeepsThePanelFullyInsideTheVisibleFrame() {
    for anchor in OverlayAnchor.allCases {
      let origin = OverlayPlacement.origin(for: anchor, size: size, in: visible)
      let frame = NSRect(origin: origin, size: size)
      XCTAssertTrue(visible.contains(frame), "\(anchor): \(frame) escapes \(visible)")
    }
  }

  func testClampPullsAnOffscreenFreeMotionOriginBackInside() {
    let offscreen = NSPoint(x: visible.maxX + 500, y: visible.minY - 500)
    let clamped = OverlayPlacement.clampOrigin(offscreen, size: size, in: visible)
    XCTAssertTrue(visible.contains(NSRect(origin: clamped, size: size)))
  }

  func testClampIsIdentityForAnOriginAlreadyInside() {
    let inside = NSPoint(x: visible.midX, y: visible.minY + 20)
    let clamped = OverlayPlacement.clampOrigin(inside, size: size, in: visible)
    XCTAssertEqual(clamped, inside)
  }

  @MainActor
  func testAnchorSelectionMovesTheRealPanelAndFreeMotionPersistsADrag() throws {
    let oldAnchor = OverlayPlacement.anchor
    let oldFreeMotion = OverlayPlacement.freeMotion
    let oldOrigin = OverlayPlacement.restoredOrigin(size: .zero, on: nil)
    defer {
      OverlayPlacement.anchor = oldAnchor
      OverlayPlacement.freeMotion = oldFreeMotion
      if let oldOrigin {
        OverlayPlacement.persistOrigin(oldOrigin)
      } else {
        OverlayPlacement.clearPersistedOrigin()
      }
    }

    let state = OverlayState.previewListening()
    let panel = try XCTUnwrap(
      DictationOverlayWindow.make(
        state: state,
        textScale: TextScaleController(key: "OverlayPlacementTests.textScale")
      ) as? FloatingOverlayPanel
    )
    defer { panel.invalidatePresence() }
    let controller = OverlayController(
      state: state,
      engine: nil,
      overlayEnabledProvider: { true },
      assistiveStatusProvider: { false },
      panelFactory: { _, _ in panel },
      orderPanelFront: { _ in },
      orderPanelOut: { _ in }
    )
    controller.show()

    let screen = try XCTUnwrap(NSScreen.main)
    state.selectPlacementAnchor(.bottomLeft)
    XCTAssertFalse(state.freeMotion)
    XCTAssertEqual(
      panel.frame.origin,
      OverlayPlacement.origin(for: .bottomLeft, size: panel.frame.size, on: screen)
    )

    state.selectFreeMotion()
    let dragged = OverlayPlacement.clampOrigin(
      NSPoint(x: screen.visibleFrame.midX, y: screen.visibleFrame.midY),
      size: panel.frame.size,
      in: screen.visibleFrame
    )
    panel.setFrameOrigin(dragged)
    panel.onUserMove?()

    XCTAssertTrue(state.freeMotion)
    let restored = try XCTUnwrap(
      OverlayPlacement.restoredOrigin(size: panel.frame.size, on: screen)
    )
    XCTAssertEqual(restored.x, dragged.x, accuracy: 1)
    XCTAssertEqual(restored.y, dragged.y, accuracy: 1)
  }
}
