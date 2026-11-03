import AppKit
import XCTest

@testable import Codescribe

@MainActor
final class OverlayIntercomDetachGestureTests: XCTestCase {
  func testShortPullCancelsWithoutDetachingAndResetsFeedback() throws {
    let window = makeWindow()
    defer { window.close() }
    let view = OverlayIntercomDragView(frame: CGRect(x: 0, y: 0, width: 100, height: 100))
    window.contentView = view
    view.minimumDragDistance = 24
    var attempts = 0
    var pulls: [CGSize] = []
    view.onDrag = { attempts += 1; return false }
    view.onPull = { pulls.append($0) }
    view.mouseDown(with: try event(.leftMouseDown, x: 20, y: 20, window: window))
    view.mouseDragged(with: try event(.leftMouseDragged, x: 32, y: 20, window: window))
    XCTAssertEqual(attempts, 0, "An ordinary small drag must not detach")
    XCTAssertEqual(pulls.last, CGSize(width: 12, height: 0))
    view.mouseUp(with: try event(.leftMouseUp, x: 32, y: 20, window: window))
    XCTAssertEqual(attempts, 0)
    XCTAssertEqual(pulls.last, .zero, "Releasing before the latch returns the grip")
  }

  func testDeliberatePullCrossesLatchOnceAndRefusedOwnerCannotRetryWithinGesture() throws {
    let window = makeWindow()
    defer { window.close() }
    let view = OverlayIntercomDragView(frame: CGRect(x: 0, y: 0, width: 100, height: 100))
    window.contentView = view
    view.minimumDragDistance = 24
    var attempts = 0
    view.onDrag = { attempts += 1; return false }
    view.mouseDown(with: try event(.leftMouseDown, x: 20, y: 20, window: window))
    view.mouseDragged(with: try event(.leftMouseDragged, x: 38, y: 38, window: window))
    view.mouseDragged(with: try event(.leftMouseDragged, x: 50, y: 50, window: window))
    XCTAssertEqual(attempts, 1)
    view.mouseUp(with: try event(.leftMouseUp, x: 50, y: 50, window: window))
    view.mouseDown(with: try event(.leftMouseDown, x: 20, y: 20, window: window))
    view.mouseDragged(with: try event(.leftMouseDragged, x: 45, y: 20, window: window))
    XCTAssertEqual(attempts, 2, "A fresh deliberate gesture may retry")
    view.mouseUp(with: try event(.leftMouseUp, x: 45, y: 20, window: window))
  }

  private func makeWindow() -> NSWindow {
    let window = NSWindow(contentRect: CGRect(x: -10000, y: -10000, width: 100, height: 100),
      styleMask: .borderless, backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    return window
  }

  private func event(_ type: NSEvent.EventType, x: CGFloat, y: CGFloat, window: NSWindow) throws -> NSEvent {
    try XCTUnwrap(NSEvent.mouseEvent(with: type, location: CGPoint(x: x, y: y),
      modifierFlags: [], timestamp: 0, windowNumber: window.windowNumber,
      context: nil, eventNumber: 1, clickCount: 1, pressure: 1))
  }
}
