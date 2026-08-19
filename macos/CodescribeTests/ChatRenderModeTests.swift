import XCTest

@testable import Codescribe

/// Per-bubble raw↔rich rendering policy: the pure `nextRenderMode` toggle, the
/// rich settled-message default, and
/// the store mutation path that owns the state (never the view).
@MainActor
final class ChatRenderModeTests: XCTestCase {
  func testNextRenderModeTogglesRawToRich() {
    XCTAssertEqual(MessageRenderMode.nextRenderMode(after: .raw), .rich)
  }

  func testNextRenderModeTogglesRichBackToRaw() {
    XCTAssertEqual(MessageRenderMode.nextRenderMode(after: .rich), .raw)
  }

  func testDefaultRenderModeIsRich() {
    let message = ChatMessage(role: .assistant, timestamp: "now", text: "hi")
    XCTAssertEqual(message.renderMode, .rich)
  }

  func testStoreToggleFlipsOnlyTheTargetMessage() {
    var thread = ChatThread(title: "t", meta: "now")
    let first = ChatMessage(role: .assistant, timestamp: "now", text: "one")
    let second = ChatMessage(role: .assistant, timestamp: "now", text: "two")
    thread.messages = [first, second]
    let store = AgentChatStore(threads: [thread])

    store.toggleRenderMode(messageID: second.id, in: thread.id)

    XCTAssertEqual(store.threads[0].messages[0].renderMode, .rich)
    XCTAssertEqual(store.threads[0].messages[1].renderMode, .raw)

    store.toggleRenderMode(messageID: second.id, in: thread.id)
    XCTAssertEqual(store.threads[0].messages[1].renderMode, .rich)
  }

  func testStoreToggleUnknownMessageIsNoOp() {
    var thread = ChatThread(title: "t", meta: "now")
    thread.messages = [ChatMessage(role: .assistant, timestamp: "now", text: "one")]
    let store = AgentChatStore(threads: [thread])

    store.toggleRenderMode(messageID: UUID(), in: thread.id)

    XCTAssertEqual(store.threads[0].messages[0].renderMode, .rich)
  }
}
