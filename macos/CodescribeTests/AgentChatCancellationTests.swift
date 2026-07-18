import Foundation
import XCTest
@testable import Codescribe

@MainActor
final class AgentChatCancellationTests: XCTestCase {
    private enum CancellationEvent: Equatable {
        case swiftTaskCancelled
        case rustCancel(String)
    }

    private final class LockedState: @unchecked Sendable {
        private let lock = NSLock()
        private var storedEvents: [CancellationEvent] = []
        private var storedContinuation: CheckedContinuation<String, Error>?
        private var storedCallCount = 0

        var events: [CancellationEvent] {
            lock.withLock { storedEvents }
        }

        var callCount: Int {
            lock.withLock { storedCallCount }
        }

        func nextCall() -> Int {
            lock.withLock {
                storedCallCount += 1
                return storedCallCount
            }
        }

        func suspend(with continuation: CheckedContinuation<String, Error>) {
            lock.withLock { storedContinuation = continuation }
        }

        func record(_ event: CancellationEvent) {
            lock.withLock { storedEvents.append(event) }
        }

        func cancelSuspendedCall() {
            let continuation = lock.withLock { () -> CheckedContinuation<String, Error>? in
                defer { storedContinuation = nil }
                return storedContinuation
            }
            continuation?.resume(throwing: CancellationError())
        }
    }

    private final class SpyEngine: AgentChatEngine {
        let firstStreamStarted: XCTestExpectation
        let emitPartialAndTool: Bool
        let state = LockedState()

        init(firstStreamStarted: XCTestExpectation, emitPartialAndTool: Bool = false) {
            self.firstStreamStarted = firstStreamStarted
            self.emitPartialAndTool = emitPartialAndTool
        }

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
            let call = state.nextCall()
            if call > 1 {
                await onDelta("Recovered")
                return "Recovered"
            }

            if emitPartialAndTool {
                await onDelta("Partial answer")
                await onToolExecuting("slow-side-effect", "call-1")
            }

            return try await withTaskCancellationHandler {
                try await withCheckedThrowingContinuation { continuation in
                    state.suspend(with: continuation)
                    firstStreamStarted.fulfill()
                }
            } onCancel: {
                self.state.record(.swiftTaskCancelled)
            }
        }

        func cancelReply(threadId: String) -> Bool {
            state.record(.rustCancel(threadId))
            state.cancelSuspendedCall()
            return true
        }
    }

    private final class VoiceCancelSpy: VoiceTurnCancelling {
        private(set) var threadIDs: [String] = []
        var acknowledges = true

        func cancelVoiceTurn(threadId: String) -> Bool {
            threadIDs.append(threadId)
            return acknowledges
        }
    }

    func testThinkingStopCancelsSwiftBeforeExactRustThreadAndIsIdempotent() async throws {
        let started = expectation(description: "first composer stream started")
        let engine = SpyEngine(firstStreamStarted: started)
        let store = makeStore(engine: engine, backendID: "backend-thread-42")
        store.draft = "start thinking"

        store.send()
        await fulfillment(of: [started], timeout: 1)

        XCTAssertEqual(store.activeComposerTurn?.phase, .thinking)
        XCTAssertEqual(store.activeComposerTurn?.backendThreadID, "backend-thread-42")

        store.stopActiveTurn()
        XCTAssertEqual(store.activeComposerTurn?.phase, .cancelling)
        store.stopActiveTurn()

        await waitUntil { store.activeComposerTurn == nil }

        XCTAssertEqual(
            engine.state.events,
            [.swiftTaskCancelled, .rustCancel("backend-thread-42")]
        )
        let assistant = try XCTUnwrap(store.currentThread?.messages.last { $0.role == .assistant })
        XCTAssertEqual(assistant.text, "Stopped")
        XCTAssertTrue(assistant.wasStopped)
        XCTAssertFalse(assistant.isThinking)
        XCTAssertFalse(assistant.isStreaming)
        XCTAssertFalse(store.isThinking)
        XCTAssertFalse(store.isStreaming)
    }

    func testStreamingStopPreservesPartialCancelsToolAndNextSendRecovers() async throws {
        let started = expectation(description: "stream emitted partial text and slow tool")
        let engine = SpyEngine(firstStreamStarted: started, emitPartialAndTool: true)
        let store = makeStore(engine: engine, backendID: "backend-recovery")
        store.draft = "stream this"

        store.send()
        await fulfillment(of: [started], timeout: 1)

        XCTAssertEqual(store.activeComposerTurn?.phase, .streaming)
        XCTAssertTrue(store.isStreaming)

        store.stopActiveTurn()
        await waitUntil { store.activeComposerTurn == nil }

        let stoppedAssistant = try XCTUnwrap(store.currentThread?.messages.last { $0.role == .assistant })
        XCTAssertEqual(stoppedAssistant.text, "Partial answer")
        XCTAssertTrue(stoppedAssistant.wasStopped)
        XCTAssertFalse(stoppedAssistant.isThinking)
        XCTAssertFalse(stoppedAssistant.isStreaming)

        let tool = try XCTUnwrap(store.currentThread?.messages.last { $0.role == .tool })
        let line = try XCTUnwrap(tool.toolLines.first)
        XCTAssertEqual(line.state, .cancelled)
        XCTAssertEqual(line.verb, "stopped")
        XCTAssertTrue(tool.toolTitle.contains("stopped"))

        store.draft = "send again"
        store.send()
        await waitUntil { engine.state.callCount == 2 && store.activeComposerTurn == nil }

        let recovered = try XCTUnwrap(store.currentThread?.messages.last { $0.role == .assistant })
        XCTAssertEqual(recovered.text, "Recovered")
        XCTAssertFalse(recovered.wasStopped)
        XCTAssertFalse(recovered.isThinking)
        XCTAssertFalse(recovered.isStreaming)
    }

    func testComposerActionProjectsThinkingStreamingAndCancelling() {
        XCTAssertEqual(
            ComposerActionVisualState.resolve(canSend: false, activePhase: .thinking),
            .stop
        )
        XCTAssertEqual(
            ComposerActionVisualState.resolve(canSend: false, activePhase: .streaming),
            .stop
        )
        XCTAssertEqual(
            ComposerActionVisualState.resolve(canSend: true, activePhase: .cancelling),
            .stopping
        )
        XCTAssertFalse(ComposerActionVisualState.stopping.isEnabled)
        XCTAssertEqual(ComposerActionVisualState.stop.accessibilityLabel, "Stop response")
        XCTAssertEqual(ComposerActionAccessibility.identifier, "agent-composer-primary-action")
    }

    func testVoiceStopRoutesOnlyToVoiceAdapterPreservesPartialAndRecovers() throws {
        // Construct directly rather than registering an XCTest expectation: the
        // stronger routing assertion is that the composer engine records no
        // events/calls, and registered-but-unwaited expectations fail XCTest.
        let composerStarted = XCTestExpectation(description: "unused composer start")
        let engine = SpyEngine(firstStreamStarted: composerStarted)
        let voice = VoiceCancelSpy()
        let store = makeStore(
            engine: engine,
            backendID: "voice-thread-42",
            voiceTurnCanceller: voice
        )

        store.ingestVoiceTurn(threadId: "voice-thread-42", userText: "voice request")
        store.ingestVoiceDelta("Partial voice answer")
        store.ingestVoiceToolExecuting(name: "slow-side-effect", id: "voice-call-1")

        XCTAssertEqual(store.selectedComposerTurnPhase, .streaming)
        store.stopActiveTurn()
        store.stopActiveTurn()

        XCTAssertEqual(voice.threadIDs, ["voice-thread-42"])
        XCTAssertTrue(engine.state.events.isEmpty, "voice Stop must not call composer cancellation")
        XCTAssertEqual(store.voiceTurnPhase, .cancelling)

        // A queued delta after Stop cannot repaint the cancelling bubble.
        store.ingestVoiceDelta(" late")
        store.ingestVoiceCancelled(threadId: "voice-thread-42")
        store.ingestVoiceDone() // a duplicate successful terminal is ignored

        let stopped = try XCTUnwrap(store.currentThread?.messages.last { $0.role == .assistant })
        XCTAssertEqual(stopped.text, "Partial voice answer")
        XCTAssertTrue(stopped.wasStopped)
        XCTAssertFalse(stopped.isThinking)
        XCTAssertFalse(stopped.isStreaming)
        let stoppedTool = try XCTUnwrap(store.currentThread?.messages.last { $0.role == .tool })
        XCTAssertEqual(stoppedTool.toolLines.first?.state, .cancelled)
        XCTAssertEqual(stoppedTool.toolLines.first?.verb, "stopped")
        XCTAssertNil(store.voiceTurnPhase)

        store.ingestVoiceTurn(threadId: "voice-thread-42", userText: "try again")
        store.ingestVoiceDelta("Recovered voice turn")
        store.ingestVoiceDone()

        let recovered = try XCTUnwrap(store.currentThread?.messages.last { $0.role == .assistant })
        XCTAssertEqual(recovered.text, "Recovered voice turn")
        XCTAssertFalse(recovered.wasStopped)
        XCTAssertNil(store.voiceTurnPhase)
    }

    private func makeStore(
        engine: AgentChatEngine,
        backendID: String,
        voiceTurnCanceller: VoiceTurnCancelling? = nil
    ) -> AgentChatStore {
        var thread = ChatThread(title: "Cancellation", meta: "now")
        thread.backendId = backendID
        thread.messagesLoaded = true
        return AgentChatStore(
            engine: engine,
            threads: [thread],
            voiceTurnCanceller: voiceTurnCanceller
        )
    }

    private func waitUntil(
        timeout: Duration = .seconds(1),
        _ condition: @escaping @MainActor () -> Bool
    ) async {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: timeout)
        while clock.now < deadline {
            if condition() { return }
            try? await Task.sleep(for: .milliseconds(5))
        }
        XCTFail("Timed out waiting for cancellation state")
    }
}
