import AppKit
import XCTest
@testable import Codescribe

@MainActor
final class AgentSummonTests: XCTestCase {
    private final class SpyEngine: AgentChatEngine {
        private(set) var streamCalls = 0
        private(set) var cancelCalls = 0

        func isAvailable() -> Bool { true }
        func availabilityDetail() -> String? { nil }
        func generateThreadTitle(_ text: String) async throws -> String? { nil }

        func streamReply(
            _ text: String,
            threadId: String,
            attachmentPaths: [String],
            onDelta: @escaping @MainActor (String) -> Void,
            onReasoning: @escaping @MainActor (String) -> Void,
            onToolExecuting: @escaping @MainActor (String, String) -> Void,
            onToolResult: @escaping @MainActor (String, String, Bool, String) -> Void
        ) async throws -> String {
            streamCalls += 1
            return "unexpected"
        }

        func cancelReply(threadId: String) -> Bool {
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
}
