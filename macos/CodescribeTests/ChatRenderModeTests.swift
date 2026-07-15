import XCTest
@testable import Codescribe

/// Per-bubble raw↔rich rendering policy: the pure `nextRenderMode` toggle, the
/// raw default (operator decision C2b — stream and settled turn identical), and
/// the store mutation path that owns the state (never the view).
@MainActor
final class ChatRenderModeTests: XCTestCase {
    func testNextRenderModeTogglesRawToRich() {
        XCTAssertEqual(MessageRenderMode.nextRenderMode(after: .raw), .rich)
    }

    func testNextRenderModeTogglesRichBackToRaw() {
        XCTAssertEqual(MessageRenderMode.nextRenderMode(after: .rich), .raw)
    }

    func testDefaultRenderModeIsRaw() {
        // C2b: raw mono is the default; rich is opt-in. Do not flip this.
        let message = ChatMessage(role: .assistant, timestamp: "now", text: "hi")
        XCTAssertEqual(message.renderMode, .raw)
    }

    func testStoreToggleFlipsOnlyTheTargetMessage() {
        var thread = ChatThread(title: "t", meta: "now")
        let first = ChatMessage(role: .assistant, timestamp: "now", text: "one")
        let second = ChatMessage(role: .assistant, timestamp: "now", text: "two")
        thread.messages = [first, second]
        let store = AgentChatStore(threads: [thread])

        store.toggleRenderMode(messageID: second.id, in: thread.id)

        XCTAssertEqual(store.threads[0].messages[0].renderMode, .raw)
        XCTAssertEqual(store.threads[0].messages[1].renderMode, .rich)

        store.toggleRenderMode(messageID: second.id, in: thread.id)
        XCTAssertEqual(store.threads[0].messages[1].renderMode, .raw)
    }

    func testStoreToggleUnknownMessageIsNoOp() {
        var thread = ChatThread(title: "t", meta: "now")
        thread.messages = [ChatMessage(role: .assistant, timestamp: "now", text: "one")]
        let store = AgentChatStore(threads: [thread])

        store.toggleRenderMode(messageID: UUID(), in: thread.id)

        XCTAssertEqual(store.threads[0].messages[0].renderMode, .raw)
    }
}
