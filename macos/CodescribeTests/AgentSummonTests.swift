import AppKit
import XCTest

@testable import Codescribe

@MainActor
final class AgentSummonTests: XCTestCase {
  private final class SpyEngine: ChatEngineFixture {
    private(set) var streamCalls = 0
    private(set) var cancelCalls = 0

    func acceptReply(
      _: String, threadId _: String, attachmentPaths _: [String]
    ) async throws -> String {
      streamCalls += 1
      return "unexpected"
    }

    func cancelReply(threadId _: String) -> Bool {
      cancelCalls += 1
      return false
    }
  }

  func testRepeatedSummonPreservesThreadDraftAttachmentsAndIdleState() {
    var first = ChatThread(title: "First", meta: "now")
    first.messages = [ChatMessage(role: .you, timestamp: "now", text: "existing")]
    let second = ChatThread(title: "Second", meta: "now")
    let engine = SpyEngine()
    let store = AgentChatStore(engine: engine, threads: [first, second])
    store.select(second.id)
    store.draft = "unsent draft"
    store.addAttachments([URL(fileURLWithPath: "/tmp/staged-agent-summon.png")])

    let window = NSWindow()
    var presentedWindows: [ObjectIdentifier] = []
    let action = AgentSummonAction(store: store) {
      presentedWindows.append(ObjectIdentifier(window))
    }

    let threadIDs = store.threads.map(\.id)
    let messageCounts = store.threads.map { $0.messages.count }
    let attachmentIDs = store.pendingAttachments.map(\.id)
    action.perform()
    action.perform()

    XCTAssertEqual(presentedWindows.count, 2)
    XCTAssertEqual(Set(presentedWindows).count, 1, "the presenter must reuse one Agent window")
    XCTAssertEqual(store.composerFocusRequest, 2)
    XCTAssertEqual(store.threads.map(\.id), threadIDs)
    XCTAssertEqual(store.threads.map { $0.messages.count }, messageCounts)
    XCTAssertEqual(store.selectedThreadID, second.id)
    XCTAssertEqual(store.draft, "unsent draft")
    XCTAssertEqual(store.pendingAttachments.map(\.id), attachmentIDs)
    XCTAssertFalse(store.isThinking)
    XCTAssertFalse(store.isStreaming)
    XCTAssertEqual(engine.streamCalls, 0)
    XCTAssertEqual(engine.cancelCalls, 0)
  }

  func testForeignCallbackDeliversExactlyOneMainActorAction() async {
    let delivered = expectation(description: "show Agent action")
    delivered.expectedFulfillmentCount = 1
    let listener = AgentAppActionListener {
      delivered.fulfill()
    }

    listener.onShowAgent()

    await fulfillment(of: [delivered], timeout: 1.0)
  }

  func testMaxApprovalInvalidationDoesNotInvokeTheChatSummonAction() async {
    let delivered = expectation(description: "Max pending state refreshed")
    let listener = AgentAppActionListener(
      maxApprovalsChanged: { delivered.fulfill() },
      summonAgent: { XCTFail("Max must not select or focus the chat composer") }
    )
    listener.onMaxApprovalsChanged()
    await fulfillment(of: [delivered], timeout: 1.0)
    listener.invalidate()
  }

  func testAgentPinMapsToFloatingAndNormalWindowLevels() {
    XCTAssertEqual(
      AgentWindowLevelPolicy.level(isPinned: true).rawValue,
      NSWindow.Level.floating.rawValue
    )
    XCTAssertEqual(
      AgentWindowLevelPolicy.level(isPinned: false).rawValue,
      NSWindow.Level.normal.rawValue
    )
  }
}
