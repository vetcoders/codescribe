import AppKit
import XCTest

@testable import Codescribe

@MainActor
final class StreamScrollFreedomTests: XCTestCase {
  func testUserScrollDetachesAndStreamingAppendDoesNotRequestScroll() {
    var state = StreamScrollFollowState()

    XCTAssertEqual(state.handle(.userScrollBegan), .none)
    XCTAssertFalse(state.followingLive)
    XCTAssertTrue(state.showsJumpToCurrent)
    XCTAssertEqual(state.handle(.contentChanged), .none)
  }

  func testJumpToCurrentReattachesAndRequestsScroll() {
    var state = detachedState()

    XCTAssertEqual(state.handle(.jumpToCurrent), .scrollToLiveEdge)
    XCTAssertTrue(state.followingLive)
    XCTAssertFalse(state.showsJumpToCurrent)
  }

  func testReachingBottomManuallyReattaches() {
    var state = detachedState()

    XCTAssertEqual(state.handle(.userScrollEnded(isAtLiveEdge: true)), .none)
    XCTAssertTrue(state.followingLive)
    XCTAssertFalse(state.showsJumpToCurrent)
  }

  func testStaleLiveEdgeGeometryDuringGestureDoesNotReattach() {
    var state = detachedState()

    XCTAssertEqual(state.handle(.userViewportChanged(isAtLiveEdge: true)), .none)
    XCTAssertFalse(state.followingLive)
    XCTAssertTrue(state.showsJumpToCurrent)
  }

  func testEndingManualScrollAwayFromEdgeStaysDetached() {
    var state = detachedState()

    XCTAssertEqual(state.handle(.userScrollEnded(isAtLiveEdge: false)), .none)
    XCTAssertFalse(state.followingLive)
  }

  func testViewportAwayFromEdgeDoesNotDetachProgrammaticFollow() {
    var state = StreamScrollFollowState()

    XCTAssertEqual(state.handle(.userViewportChanged(isAtLiveEdge: false)), .none)
    XCTAssertTrue(state.followingLive)
  }

  func testStreamFinishNeverJumpsOrClearsDetachedState() {
    var state = detachedState()

    XCTAssertEqual(state.handle(.streamFinished), .none)
    XCTAssertFalse(state.followingLive)
    XCTAssertTrue(state.showsJumpToCurrent)
  }

  func testThreadSwitchResetsFollowStateConsciously() {
    var state = detachedState()

    XCTAssertEqual(state.handle(.threadChanged), .scrollToLiveEdge)
    XCTAssertTrue(state.followingLive)
    XCTAssertFalse(state.showsJumpToCurrent)
  }

  func testStreamingAppendFollowsOnlyAtLiveEdge() {
    var state = StreamScrollFollowState()

    XCTAssertEqual(state.handle(.contentChanged), .scrollToLiveEdge)
  }

  func testAppKitScrollEventsSynchronouslyOwnViewportUntilLiveScrollEnds() {
    let frame = NSRect(x: 0, y: 0, width: 420, height: 160)
    let window = NSWindow(
      contentRect: frame,
      styleMask: [.titled],
      backing: .buffered,
      defer: true
    )
    window.isReleasedWhenClosed = false
    let scrollView = NSScrollView(frame: frame)
    scrollView.hasVerticalScroller = true
    let documentView = NSView(frame: NSRect(x: 0, y: 0, width: 400, height: 1_200))
    scrollView.documentView = documentView
    window.contentView = scrollView

    var state = StreamScrollFollowState()
    let observer = ChatLiveScrollObserver { event in
      _ = state.handle(event)
    }
    let coordinator = observer.makeCoordinator()
    coordinator.attach(to: scrollView)
    defer {
      coordinator.detach()
      window.close()
    }

    NotificationCenter.default.post(
      name: NSScrollView.willStartLiveScrollNotification,
      object: scrollView
    )
    XCTAssertFalse(
      state.followingLive,
      "the AppKit begin event must reduce before this turn returns"
    )
    XCTAssertEqual(state.handle(.contentChanged), .none)

    scrollView.contentView.scroll(
      to: NSPoint(
        x: 0,
        y: documentView.bounds.maxY - scrollView.contentView.bounds.height
      )
    )
    scrollView.reflectScrolledClipView(scrollView.contentView)
    NotificationCenter.default.post(
      name: NSScrollView.didLiveScrollNotification,
      object: scrollView
    )
    XCTAssertFalse(
      state.followingLive,
      "intermediate live-scroll geometry cannot resume follow"
    )

    NotificationCenter.default.post(
      name: NSScrollView.didEndLiveScrollNotification,
      object: scrollView
    )
    XCTAssertTrue(
      state.followingLive,
      "the finished gesture resumes only at the actual bottom"
    )

    coordinator.detach()
    NotificationCenter.default.post(
      name: NSScrollView.willStartLiveScrollNotification,
      object: scrollView
    )
    XCTAssertTrue(
      state.followingLive,
      "dismantling removes the notification observer"
    )
  }

  private func detachedState() -> StreamScrollFollowState {
    var state = StreamScrollFollowState()
    _ = state.handle(.userScrollBegan)
    return state
  }
}
