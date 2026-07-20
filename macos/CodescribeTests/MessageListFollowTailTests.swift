import XCTest
@testable import Codescribe

/// Pure scroll logic behind the chat message list: the follow-tail decision
/// (`followTailAfterScroll`, slack boundary) and the "↓ Latest" pill visibility
/// matrix (`showLatestPill`).
@MainActor
final class MessageListFollowTailTests: XCTestCase {
    // MARK: followTailAfterScroll boundaries (viewport 600, slack 40 → edge 640)

    func testFollowsExactlyAtSlackBoundary() {
        XCTAssertTrue(MessageList.followTailAfterScroll(
            contentBottom: 640, viewportHeight: 600, slack: 40
        ))
    }

    func testFollowsJustInsideSlack() {
        XCTAssertTrue(MessageList.followTailAfterScroll(
            contentBottom: 639, viewportHeight: 600, slack: 40
        ))
    }

    func testDetachesJustPastSlack() {
        XCTAssertFalse(MessageList.followTailAfterScroll(
            contentBottom: 641, viewportHeight: 600, slack: 40
        ))
    }

    func testShortContentAlwaysFollows() {
        // Content shorter than the viewport: bottom edge well inside → follow.
        XCTAssertTrue(MessageList.followTailAfterScroll(
            contentBottom: 200, viewportHeight: 600, slack: 40
        ))
    }

    func testDefaultSlackIs40() {
        // The contract slack (40pt) is the default argument — a caller passing
        // no slack gets the same boundary the view ships with.
        XCTAssertTrue(MessageList.followTailAfterScroll(
            contentBottom: 640, viewportHeight: 600
        ))
        XCTAssertFalse(MessageList.followTailAfterScroll(
            contentBottom: 640.5, viewportHeight: 600
        ))
    }

    // MARK: showLatestPill matrix (4 cases)

    func testPillHiddenWhileFollowingDuringStream() {
        XCTAssertFalse(MessageList.showLatestPill(followTail: true, isStreaming: true))
    }

    func testPillHiddenWhileFollowingIdle() {
        XCTAssertFalse(MessageList.showLatestPill(followTail: true, isStreaming: false))
    }

    func testPillShownWhenDetachedDuringStream() {
        XCTAssertTrue(MessageList.showLatestPill(followTail: false, isStreaming: true))
    }

    func testPillHiddenWhenDetachedOverSettledThread() {
        XCTAssertFalse(MessageList.showLatestPill(followTail: false, isStreaming: false))
    }

    // MARK: tailSignature (the auto-scroll trigger)

    func testSignatureChangesWhenStreamGrows() {
        var message = ChatMessage(role: .assistant, timestamp: "now", text: "partial")
        let before = MessageList.tailSignature([message])
        message.text += " more"
        XCTAssertNotEqual(before, MessageList.tailSignature([message]))
    }

    func testSignatureChangesWhenTurnLands() {
        let first = ChatMessage(role: .you, timestamp: "now", text: "hi")
        let before = MessageList.tailSignature([first])
        let reply = ChatMessage(role: .assistant, timestamp: "now", text: "")
        XCTAssertNotEqual(before, MessageList.tailSignature([first, reply]))
    }

    func testSignatureChangesWhenToolLineStateFlips() {
        var tool = ChatMessage(role: .tool, timestamp: "now", text: "")
        tool.toolLines = [ToolLine(verb: "tool", detail: "grep", state: .running)]
        let assistant = ChatMessage(role: .assistant, timestamp: "now", text: "")
        let before = MessageList.tailSignature([tool, assistant])
        tool.toolLines[0].state = .succeeded
        XCTAssertNotEqual(before, MessageList.tailSignature([tool, assistant]))
    }

    func testSignatureIgnoresRenderModeFlip() {
        // A raw↔rich toggle must NOT trigger an auto-scroll.
        var message = ChatMessage(role: .assistant, timestamp: "now", text: "done")
        let before = MessageList.tailSignature([message])
        message.renderMode = .rich
        XCTAssertEqual(before, MessageList.tailSignature([message]))
    }
}
