import Foundation
import XCTest
@testable import Codescribe

@MainActor
final class AgentThreadTitleTests: XCTestCase {
    private enum FakeError: Error {
        case failed
    }

    private struct StreamCall: Equatable {
        let text: String
        let threadID: String
        let attachmentPaths: [String]
    }

    private final class ControllableEngine: AgentChatEngine {
        private let lock = NSLock()
        private var storedTitleCalls: [String] = []
        private var storedStreamCalls: [StreamCall] = []
        private var titleContinuation: CheckedContinuation<String?, Error>?
        private var streamContinuation: CheckedContinuation<String, Error>?

        let unavailableDetail: String?

        init(unavailableDetail: String? = nil) {
            self.unavailableDetail = unavailableDetail
        }

        var titleCalls: [String] { lock.withLock { storedTitleCalls } }
        var streamCalls: [StreamCall] { lock.withLock { storedStreamCalls } }

        func isAvailable() -> Bool { unavailableDetail == nil }
        func availabilityDetail() -> String? { unavailableDetail }

        func generateThreadTitle(_ text: String) async throws -> String? {
            try await withCheckedThrowingContinuation { continuation in
                lock.withLock {
                    storedTitleCalls.append(text)
                    titleContinuation = continuation
                }
            }
        }

        func streamReply(
            _ text: String,
            threadId: String,
            attachmentPaths: [String],
            onDelta: @escaping @MainActor (String) -> Void,
            onReasoning: @escaping @MainActor (String) -> Void,
            onToolExecuting: @escaping @MainActor (String, String) -> Void,
            onToolResult: @escaping @MainActor (String, String, Bool, String) -> Void
        ) async throws -> String {
            try await withCheckedThrowingContinuation { continuation in
                lock.withLock {
                    storedStreamCalls.append(
                        StreamCall(text: text, threadID: threadId, attachmentPaths: attachmentPaths)
                    )
                    streamContinuation = continuation
                }
            }
        }

        func cancelReply(threadId: String) -> Bool {
            completeStream(.failure(CancellationError()))
            return true
        }

        func completeTitle(_ result: Result<String?, Error>) {
            let continuation = lock.withLock { () -> CheckedContinuation<String?, Error>? in
                defer { titleContinuation = nil }
                return titleContinuation
            }
            continuation?.resume(with: result)
        }

        func completeStream(_ result: Result<String, Error>) {
            let continuation = lock.withLock { () -> CheckedContinuation<String, Error>? in
                defer { streamContinuation = nil }
                return streamContinuation
            }
            continuation?.resume(with: result)
        }
    }

    private final class TitleThreadsProvider: ChatThreadsProviding {
        enum Event: Equatable {
            case list
            case generated(String)
            case renamed(String)
            case deleted
        }

        let backendID: String
        private(set) var events: [Event] = []
        private(set) var persistedTitle: String
        var threadExists = false
        var isCustom = false
        var forceGeneratedFailure = false

        init(backendID: String = "title-thread-1", title: String = "Heuristic slug") {
            self.backendID = backendID
            self.persistedTitle = title
        }

        func markFirstTurnPersisted() {
            threadExists = true
        }

        func listThreads() -> [ChatThread] {
            events.append(.list)
            guard threadExists else { return [] }
            var thread = ChatThread(title: persistedTitle, meta: "now")
            thread.backendId = backendID
            thread.messagesLoaded = true
            return [thread]
        }

        func searchThreads(query: String) -> [ChatThread] { listThreads() }
        func loadMessages(backendId: String) -> [ChatMessage] { [] }

        func deleteThread(backendId: String) -> Bool {
            events.append(.deleted)
            guard threadExists else { return false }
            threadExists = false
            return true
        }

        func setThreadFavorite(backendId: String, isFavorite: Bool) -> Bool { threadExists }

        func renameThread(backendId: String, title: String) -> Bool {
            events.append(.renamed(title))
            guard threadExists else { return false }
            persistedTitle = title
            isCustom = true
            return true
        }

        func setGeneratedTitle(backendId: String, title: String) -> Bool {
            events.append(.generated(title))
            guard threadExists, !isCustom, !forceGeneratedFailure else { return false }
            persistedTitle = title
            return true
        }

        func exportThreadMarkdown(backendId: String, assistantOnly: Bool) -> String? { nil }
        func generateThreadId() -> String { backendID }
    }

    func testFirstTextTurnLaunchesExactlyOneIndependentTitleRequest() async throws {
        let engine = ControllableEngine()
        let provider = TitleThreadsProvider()
        let store = makeStore(engine: engine, provider: provider)
        store.draft = "Plan the rename race"

        store.send()
        await waitUntil { engine.titleCalls.count == 1 && engine.streamCalls.count == 1 }

        XCTAssertEqual(engine.titleCalls, ["Plan the rename race"])
        XCTAssertEqual(
            engine.streamCalls,
            [StreamCall(text: "Plan the rename race", threadID: provider.backendID, attachmentPaths: [])]
        )

        engine.completeTitle(.success(nil))
        provider.markFirstTurnPersisted()
        engine.completeStream(.success("Assistant reply"))
        await waitUntil { store.activeComposerTurn == nil }

        XCTAssertEqual(engine.titleCalls.count, 1)
        XCTAssertEqual(store.currentThread?.messages.last?.text, "Assistant reply")
    }

    func testSubsequentAttachmentOnlyAndUnavailableTurnsDoNotLaunchTitle() async {
        do {
            let engine = ControllableEngine()
            let provider = TitleThreadsProvider()
            var thread = ChatThread(title: "Existing", meta: "now")
            thread.messages = [ChatMessage(role: .you, timestamp: "now", text: "first")]
            let store = makeStore(engine: engine, provider: provider, thread: thread)
            store.draft = "second"
            store.send()
            await waitUntil { engine.streamCalls.count == 1 }
            XCTAssertTrue(engine.titleCalls.isEmpty)
            provider.markFirstTurnPersisted()
            engine.completeStream(.success("done"))
            await waitUntil { store.activeComposerTurn == nil }
        }

        do {
            let engine = ControllableEngine()
            let provider = TitleThreadsProvider()
            let store = makeStore(engine: engine, provider: provider)
            store.addAttachments([URL(fileURLWithPath: "/tmp/title-attachment.png")])
            store.send()
            await waitUntil { engine.streamCalls.count == 1 }
            XCTAssertTrue(engine.titleCalls.isEmpty)
            XCTAssertEqual(engine.streamCalls.first?.attachmentPaths, ["/tmp/title-attachment.png"])
            provider.markFirstTurnPersisted()
            engine.completeStream(.success("saw image"))
            await waitUntil { store.activeComposerTurn == nil }
        }

        do {
            let engine = ControllableEngine(unavailableDetail: "Assistive lane unavailable")
            let provider = TitleThreadsProvider()
            let store = makeStore(engine: engine, provider: provider)
            store.draft = "first"
            store.send()
            await waitUntil { store.activeComposerTurn == nil }
            XCTAssertTrue(engine.titleCalls.isEmpty)
            XCTAssertTrue(engine.streamCalls.isEmpty)
            XCTAssertEqual(store.currentThread?.messages.last?.text, "Assistive lane unavailable")
        }
    }

    func testEarlyTitleUpdatesOriginalThreadWithoutChangingSelection() async throws {
        let engine = ControllableEngine()
        let provider = TitleThreadsProvider()
        let first = ChatThread(title: "Heuristic slug", meta: "now")
        let second = ChatThread(title: "Other thread", meta: "now")
        let store = AgentChatStore(engine: engine, threadsProvider: provider, threads: [first, second])
        store.draft = "first request"
        store.send()
        await waitUntil { engine.titleCalls.count == 1 && engine.streamCalls.count == 1 }
        store.select(second.id)

        engine.completeTitle(.success("Race-proof Swift titles"))
        await waitUntil { store.threads.first(where: { $0.id == first.id })?.title == "Race-proof Swift titles" }

        XCTAssertEqual(store.selectedThreadID, second.id)
        XCTAssertEqual(store.threads.first(where: { $0.id == first.id })?.title, "Race-proof Swift titles")

        provider.markFirstTurnPersisted()
        engine.completeStream(.success("done"))
        await waitUntil { store.activeComposerTurn == nil }
    }

    func testMissingFirstPersistRetriesExactlyOnceBeforeRefresh() async {
        let engine = ControllableEngine()
        let provider = TitleThreadsProvider()
        let store = makeStore(engine: engine, provider: provider)
        store.draft = "build title state"
        store.send()
        await waitUntil { engine.titleCalls.count == 1 && engine.streamCalls.count == 1 }

        engine.completeTitle(.success("Title state machine"))
        await waitUntil { provider.events.filter { $0 == .generated("Title state machine") }.count == 1 }
        XCTAssertEqual(store.currentThread?.title, "Title state machine")

        provider.markFirstTurnPersisted()
        engine.completeStream(.success("done"))
        await waitUntil { store.activeComposerTurn == nil }

        let generated = provider.events.filter { $0 == .generated("Title state machine") }
        XCTAssertEqual(generated.count, 2, "one immediate attempt plus one post-stream retry")
        XCTAssertEqual(provider.persistedTitle, "Title state machine")
        XCTAssertEqual(store.currentThread?.title, "Title state machine", "refresh must not flash the heuristic slug back")
        let lastGenerated = provider.events.lastIndex(of: .generated("Title state machine"))
        let refresh = provider.events.lastIndex(of: .list)
        XCTAssertNotNil(lastGenerated)
        XCTAssertNotNil(refresh)
        XCTAssertLessThan(lastGenerated!, refresh!, "queued title must flush before refresh")
    }

    func testLateTitlePersistsDirectlyWithoutRetry() async {
        let engine = ControllableEngine()
        let provider = TitleThreadsProvider()
        let store = makeStore(engine: engine, provider: provider)
        store.draft = "late title"
        store.send()
        await waitUntil { engine.titleCalls.count == 1 && engine.streamCalls.count == 1 }

        provider.markFirstTurnPersisted()
        engine.completeStream(.success("done"))
        await waitUntil { store.activeComposerTurn == nil }
        engine.completeTitle(.success("Late direct title"))
        await waitUntil { store.currentThread?.title == "Late direct title" }

        XCTAssertEqual(provider.events.filter { $0 == .generated("Late direct title") }.count, 1)
        XCTAssertEqual(provider.persistedTitle, "Late direct title")
    }

    func testManualRenameBeforeGenerationQueuesCustomAndDiscardsGenerated() async {
        let engine = ControllableEngine()
        let provider = TitleThreadsProvider()
        let store = makeStore(engine: engine, provider: provider)
        store.draft = "rename before generation"
        store.send()
        // Deliberately rename before the send task gets a scheduling turn. The
        // synchronous first-turn state must already own the missing-file race.
        let thread = store.currentThread!
        store.rename(thread, to: "My durable title")
        XCTAssertEqual(store.currentThread?.title, "My durable title")
        await waitUntil { engine.titleCalls.count == 1 && engine.streamCalls.count == 1 }
        engine.completeTitle(.success("Generated loser"))
        await Task.yield()

        provider.markFirstTurnPersisted()
        engine.completeStream(.success("done"))
        await waitUntil { store.activeComposerTurn == nil }

        XCTAssertEqual(provider.persistedTitle, "My durable title")
        XCTAssertEqual(store.currentThread?.title, "My durable title")
        XCTAssertTrue(provider.events.filter { $0 == .generated("Generated loser") }.isEmpty)
        XCTAssertEqual(provider.events.filter { $0 == .renamed("My durable title") }.count, 2)
    }

    func testManualRenameAfterEarlyGenerationDiscardsQueuedGeneratedRetry() async {
        let engine = ControllableEngine()
        let provider = TitleThreadsProvider()
        let store = makeStore(engine: engine, provider: provider)
        store.draft = "generation first"
        store.send()
        await waitUntil { engine.titleCalls.count == 1 && engine.streamCalls.count == 1 }

        engine.completeTitle(.success("Generated first"))
        await waitUntil { store.currentThread?.title == "Generated first" }
        store.rename(store.currentThread!, to: "Custom after generation")

        provider.markFirstTurnPersisted()
        engine.completeStream(.success("done"))
        await waitUntil { store.activeComposerTurn == nil }

        XCTAssertEqual(provider.events.filter { $0 == .generated("Generated first") }.count, 1)
        XCTAssertEqual(provider.persistedTitle, "Custom after generation")
        XCTAssertEqual(store.currentThread?.title, "Custom after generation")
        let customFlush = provider.events.lastIndex(of: .renamed("Custom after generation"))
        let refresh = provider.events.lastIndex(of: .list)
        XCTAssertLessThan(customFlush!, refresh!)
    }

    func testManualRenameAfterLateGeneratedPersistWins() async {
        let engine = ControllableEngine()
        let provider = TitleThreadsProvider()
        let store = makeStore(engine: engine, provider: provider)
        store.draft = "late generated then rename"
        store.send()
        await waitUntil { engine.titleCalls.count == 1 && engine.streamCalls.count == 1 }

        provider.markFirstTurnPersisted()
        engine.completeStream(.success("done"))
        await waitUntil { store.activeComposerTurn == nil }
        engine.completeTitle(.success("Generated persisted"))
        await waitUntil { store.currentThread?.title == "Generated persisted" }

        store.rename(store.currentThread!, to: "User final title")

        XCTAssertEqual(provider.persistedTitle, "User final title")
        XCTAssertEqual(store.currentThread?.title, "User final title")
    }

    func testDeleteBeforeTitleCompletionDiscardsLateResultAndNeverReselectsThread() async {
        let engine = ControllableEngine()
        let provider = TitleThreadsProvider()
        let store = makeStore(engine: engine, provider: provider)
        let deletedID = store.currentThread!.id
        store.draft = "delete this first turn"
        store.send()
        await waitUntil { engine.titleCalls.count == 1 && engine.streamCalls.count == 1 }

        store.delete(store.currentThread!)
        await waitUntil { store.activeComposerTurn == nil }
        let replacementID = store.selectedThreadID
        engine.completeTitle(.success("Resurrected title"))
        await Task.yield()

        XCTAssertFalse(store.threads.contains { $0.id == deletedID })
        XCTAssertNotEqual(replacementID, deletedID)
        XCTAssertEqual(store.selectedThreadID, replacementID)
        XCTAssertTrue(provider.events.filter { $0 == .generated("Resurrected title") }.isEmpty)
        XCTAssertFalse(store.threads.contains { $0.title == "Resurrected title" })
    }

    func testNilEmptyThrowAndPersistenceFailureLeaveFallbackAndAssistantUntouched() async {
        await assertGenerationFallback(.success(nil))
        await assertGenerationFallback(.success("   \n"))
        await assertGenerationFallback(.failure(FakeError.failed))

        let engine = ControllableEngine()
        let provider = TitleThreadsProvider()
        provider.forceGeneratedFailure = true
        let store = makeStore(engine: engine, provider: provider)
        store.draft = "persistence failure"
        store.send()
        await waitUntil { engine.titleCalls.count == 1 && engine.streamCalls.count == 1 }
        provider.markFirstTurnPersisted()
        engine.completeStream(.success("Assistant survives"))
        await waitUntil { store.activeComposerTurn == nil }
        engine.completeTitle(.success("Cannot persist"))
        await waitUntil { provider.events.contains(.generated("Cannot persist")) }

        XCTAssertEqual(store.currentThread?.title, "Heuristic slug")
        XCTAssertEqual(store.currentThread?.messages.last?.text, "Assistant survives")
        XCTAssertEqual(provider.events.filter { $0 == .generated("Cannot persist") }.count, 1)
    }

    private func assertGenerationFallback(_ outcome: Result<String?, Error>) async {
        let engine = ControllableEngine()
        let provider = TitleThreadsProvider()
        let store = makeStore(engine: engine, provider: provider)
        store.draft = "fallback title"
        store.send()
        await waitUntil { engine.titleCalls.count == 1 && engine.streamCalls.count == 1 }
        engine.completeTitle(outcome)
        provider.markFirstTurnPersisted()
        engine.completeStream(.success("Assistant survives"))
        await waitUntil { store.activeComposerTurn == nil }

        XCTAssertEqual(store.currentThread?.title, "Heuristic slug")
        XCTAssertEqual(store.currentThread?.messages.last?.text, "Assistant survives")
        XCTAssertTrue(provider.events.filter {
            if case .generated = $0 { return true }
            return false
        }.isEmpty)
    }

    private func makeStore(
        engine: ControllableEngine,
        provider: TitleThreadsProvider,
        thread: ChatThread = ChatThread(title: "Heuristic slug", meta: "now")
    ) -> AgentChatStore {
        AgentChatStore(engine: engine, threadsProvider: provider, threads: [thread])
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
        XCTFail("Timed out waiting for deterministic title state")
    }
}
