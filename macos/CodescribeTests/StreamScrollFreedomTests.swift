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

        XCTAssertEqual(state.handle(.userViewportChanged(isAtLiveEdge: true)), .none)
        XCTAssertTrue(state.followingLive)
        XCTAssertFalse(state.showsJumpToCurrent)
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

    private func detachedState() -> StreamScrollFollowState {
        var state = StreamScrollFollowState()
        _ = state.handle(.userScrollBegan)
        return state
    }
}
