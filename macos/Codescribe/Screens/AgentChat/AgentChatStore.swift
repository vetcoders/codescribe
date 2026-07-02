import OSLog
import SwiftUI

/// Diagnostic breadcrumbs for the attachment staging path. Filter with:
///   log show --predicate 'subsystem == "com.vetcoders.codescribe"' --info
private let attachLog = Logger(
    subsystem: Bundle.main.bundleIdentifier ?? "com.vetcoders.codescribe",
    category: "attachments"
)

// MARK: - Runtime contract (read before extending this screen)
//
// This screen is backed by the real codescribe UniFFI bridge when constructed
// from AppModel: `RealChatEngine` streams assistant deltas / tool events and
// `RealThreadsEngine` reads persisted ThreadStore entries. The #Preview still
// uses local mock data. Known remaining gaps: attachments are not wired, restored
// structured tool/reasoning payloads are flattened by the thread adapter, and
// composer shortcuts are still simplified.

// MARK: - Engine seam (W2-01 injects the real adapter)

/// Thin, UI-only seam over the agent primitives the screen actually uses.
/// W2-01 supplies an adapter that forwards to the real `VistaEngine`
/// (mapping `assistive` → `VistaAiMode.assistive`). Kept free of bridge types
/// so the view-model + #Preview compile and render standalone.
protocol AgentChatEngine: AnyObject {
    /// True when the assistive provider can be built (keys present).
    func isAvailable() -> Bool
    /// Streams a real assistant reply. Callbacks fire on the main actor as tokens
    /// arrive; returns the final assembled text.
    ///
    /// `attachmentPaths` are absolute filesystem paths to images the composer
    /// attached (empty for a text-only turn). Kept as plain paths — not bridge
    /// types — so the view-model + #Preview stay standalone; the real adapter
    /// maps them to the bridge `CsAttachment` at the edge.
    func streamReply(
        _ text: String,
        threadId: String,
        attachmentPaths: [String],
        onDelta: @escaping @MainActor (String) -> Void,
        onReasoning: @escaping @MainActor (String) -> Void,
        onTool: @escaping @MainActor (_ name: String, _ isError: Bool) -> Void
    ) async throws -> String
}

// MARK: - Models

enum ChatRole {
    case you
    case tool
    case assistant
}

struct ToolLine: Identifiable, Hashable {
    let id = UUID()
    let verb: String     // "grep", "read" — rendered olive
    let detail: String   // "events/bus.ts · ui/store.ts"
}

struct ChatMessage: Identifiable {
    let id = UUID()
    var role: ChatRole
    var timestamp: String
    /// Body text. May contain `backtick` code spans for assistant/you turns.
    var text: String

    /// Files attached to a sent user turn (empty otherwise). Rendered as chips in
    /// the You bubble. In-memory for the current session only — the persisted
    /// thread does not carry attachment metadata (see RealThreadsEngine), so a
    /// restored turn comes back with this empty.
    var attachments: [MessageAttachment] = []

    // Tool-activity turn
    var toolTitle: String = ""        // "What I checked · 2 tools"
    var toolLines: [ToolLine] = []

    // Assistant turn
    var reasonedSeconds: Double? = nil
    var isThinking: Bool = false      // pre-reply "thinking…" state
    var isStreaming: Bool = false     // word-reveal in progress (shows caret)
}

/// An image the user staged in the composer but has not sent yet. Referenced by
/// file URL (NSOpenPanel / clipboard-saved temp file); the send path forwards the
/// path to the bridge, which loads + validates the bytes.
struct PendingAttachment: Identifiable, Hashable {
    let id = UUID()
    let url: URL
    var name: String { url.lastPathComponent }
}

/// An attachment carried by a *sent* chat message, surfaced as a chip in the You
/// bubble. `url` points at the source file for an optional inline thumbnail; it
/// is nil for restored turns (the persisted thread has no source path), in which
/// case the chip shows the filename only.
struct MessageAttachment: Identifiable, Hashable {
    let id = UUID()
    let name: String
    let url: URL?
}

struct ChatThread: Identifiable {
    let id = UUID()
    var title: String
    var meta: String        // mono subtitle, e.g. "active · restored" / "today · 18:40"
    var isRestored: Bool = false
    var isFavorite: Bool = false
    var backendId: String? = nil      // codescribe ThreadStore id (nil = local-only, not yet persisted)
    var messagesLoaded: Bool = false  // lazy-load guard for persisted threads
    var messages: [ChatMessage] = []
}

// MARK: - Threads provider (read-only access to persisted codescribe threads)

/// Backs the thread rail / drawer with real persisted threads from the
/// codescribe ThreadStore (via `CodescribeThreads`). Kept separate from
/// `AgentChatEngine` so the #Preview mock stays standalone.
protocol ChatThreadsProviding: AnyObject {
    func listThreads() -> [ChatThread]
    func searchThreads(query: String) -> [ChatThread]
    func loadMessages(backendId: String) -> [ChatMessage]
    func deleteThread(backendId: String) -> Bool
    func setThreadFavorite(backendId: String, isFavorite: Bool) -> Bool
    /// Rename a persisted thread; the core marks the title user-custom so
    /// auto-titling won't overwrite it. Returns `false` on failure / no such thread.
    func renameThread(backendId: String, title: String) -> Bool
    /// Export a persisted thread to a Markdown file under
    /// `~/.codescribe/transcriptions/YYYY-MM-DD/`. Returns the absolute path of the
    /// written file, or `nil` on failure. `assistantOnly` keeps only assistant turns.
    func exportThreadMarkdown(backendId: String, assistantOnly: Bool) -> String?
    /// Mint a fresh ThreadStore id for a new conversation (so it persists).
    func generateThreadId() -> String
}

// MARK: - Store

@MainActor
final class AgentChatStore: ObservableObject {
    @Published var threads: [ChatThread]
    @Published var selectedThreadID: UUID?
    @Published var draft: String = ""

    /// Images staged in the composer for the next message. Cleared when the
    /// message is dispatched.
    @Published var pendingAttachments: [PendingAttachment] = []

    /// Injected by W2-01. `nil` until then; `send` degrades gracefully.
    var engine: AgentChatEngine?

    /// Injected provider for persisted threads. `nil` → falls back to mock seed.
    var threadsProvider: ChatThreadsProviding?

    private var revealTask: Task<Void, Never>?
    private var didStartDemo = false

    init(engine: AgentChatEngine? = nil,
         threadsProvider: ChatThreadsProviding? = nil,
         threads: [ChatThread]? = nil) {
        self.engine = engine
        self.threadsProvider = threadsProvider

        let seeded: [ChatThread]
        if let threads {
            seeded = threads                                    // explicit (preview/mock)
        } else if let real = threadsProvider?.listThreads(), !real.isEmpty {
            seeded = real                                       // real persisted threads
        } else if threadsProvider != nil {
            seeded = [ChatThread(title: "New thread", meta: "now")]  // real provider, empty history
        } else {
            seeded = Self.seedThreads()                         // no provider → mock seed
        }
        self.threads = seeded
        self.selectedThreadID = seeded.first?.id
        if let first = seeded.first { loadMessagesIfNeeded(first.id) }
    }

    var currentThread: ChatThread? {
        threads.first { $0.id == selectedThreadID }
    }

    var usesRealThreadSearch: Bool { threadsProvider != nil }

    /// True while the current thread's latest assistant turn is in its pre-reply
    /// "thinking…" state. Drives the header status pill (Idle → Thinking).
    var isThinking: Bool {
        currentThread?.messages.last { $0.role == .assistant }?.isThinking ?? false
    }

    /// True while the current thread's latest assistant turn is revealing tokens.
    /// Drives the header status pill (Thinking → Streaming).
    var isStreaming: Bool {
        currentThread?.messages.last { $0.role == .assistant }?.isStreaming ?? false
    }

    // MARK: Thread ops

    func newThread() {
        let t = ChatThread(title: "New thread", meta: "now", messages: [])
        threads.insert(t, at: 0)
        selectedThreadID = t.id
        draft = ""
    }

    func refreshThreads() {
        guard let threadsProvider else { return }
        replaceThreads(
            with: threadsProvider.listThreads(),
            selectingBackendId: currentThread?.backendId,
            keepLocalDrafts: true
        )
    }

    func searchThreads(_ query: String) {
        guard let threadsProvider else { return }
        let trimmed = query.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty {
            refreshThreads()
        } else {
            replaceThreads(
                with: threadsProvider.searchThreads(query: trimmed),
                selectingBackendId: currentThread?.backendId,
                keepLocalDrafts: false,
                allowEmpty: true
            )
        }
    }

    func select(_ id: UUID) {
        selectedThreadID = id
        loadMessagesIfNeeded(id)
    }

    func toggleFavorite(_ thread: ChatThread) {
        let next = !thread.isFavorite
        guard let ti = threads.firstIndex(where: { $0.id == thread.id }) else { return }
        if let backendId = thread.backendId {
            guard threadsProvider?.setThreadFavorite(backendId: backendId, isFavorite: next) == true else { return }
        }
        threads[ti].isFavorite = next
    }

    /// Rename a thread from the rail. Persists through the threads provider when
    /// the thread is backed on disk; a not-yet-persisted local thread is renamed
    /// in memory only. No-ops on an empty or unchanged title. The chat header
    /// reads `currentThread.title`, so it updates reactively too.
    func rename(_ thread: ChatThread, to newTitle: String) {
        let trimmed = newTitle.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, trimmed != thread.title,
              let ti = threads.firstIndex(where: { $0.id == thread.id }) else { return }
        if let backendId = thread.backendId {
            guard threadsProvider?.renameThread(backendId: backendId, title: trimmed) == true else { return }
        }
        threads[ti].title = trimmed
    }

    /// Export a thread to a Markdown transcript on disk, returning the file path
    /// so the caller can reveal it in Finder. Only persisted threads (with a
    /// backend id) can be exported; a not-yet-saved local thread returns `nil`.
    func exportMarkdown(_ thread: ChatThread, assistantOnly: Bool) -> String? {
        guard let backendId = thread.backendId else { return nil }
        return threadsProvider?.exportThreadMarkdown(backendId: backendId, assistantOnly: assistantOnly)
    }

    func delete(_ thread: ChatThread) {
        if let backendId = thread.backendId {
            guard threadsProvider?.deleteThread(backendId: backendId) == true else { return }
        }
        threads.removeAll { $0.id == thread.id }
        if selectedThreadID == thread.id {
            selectedThreadID = threads.first?.id
            if let selectedThreadID { loadMessagesIfNeeded(selectedThreadID) }
        }
        if threads.isEmpty {
            newThread()
        }
    }

    /// Lazily pull a persisted thread's messages the first time it is selected.
    private func loadMessagesIfNeeded(_ id: UUID) {
        guard let provider = threadsProvider,
              let ti = threads.firstIndex(where: { $0.id == id }),
              let backendId = threads[ti].backendId,
              !threads[ti].messagesLoaded else { return }
        threads[ti].messages = provider.loadMessages(backendId: backendId)
        threads[ti].messagesLoaded = true
    }

    /// Resolve (and lazily mint) the ThreadStore id for a thread so the agent
    /// persists the conversation under a stable id across turns + restarts.
    private func ensureBackendId(_ threadID: UUID) -> String {
        guard let ti = threads.firstIndex(where: { $0.id == threadID }) else {
            return "t_\(UUID().uuidString)"
        }
        if let existing = threads[ti].backendId { return existing }
        let id = threadsProvider?.generateThreadId() ?? "t_\(UUID().uuidString)"
        threads[ti].backendId = id
        threads[ti].messagesLoaded = true  // freshly-minted thread starts in sync
        return id
    }

    // MARK: Attachments (composer staging)

    /// Stage image files chosen in the composer, de-duplicating by URL.
    func addAttachments(_ urls: [URL]) {
        let before = pendingAttachments.count
        for url in urls where !pendingAttachments.contains(where: { $0.url == url }) {
            pendingAttachments.append(PendingAttachment(url: url))
        }
        attachLog.info(
            "addAttachments: incoming=\(urls.count, privacy: .public) staged=\(self.pendingAttachments.count - before, privacy: .public) (post-dedupe) pendingAttachments.count=\(self.pendingAttachments.count, privacy: .public)"
        )
    }

    /// Remove a staged attachment before it is sent.
    func removeAttachment(_ id: UUID) {
        pendingAttachments.removeAll { $0.id == id }
    }

    /// True when there is something to send: text, at least one staged image, or
    /// both. Drives the send button's enabled state.
    var canSend: Bool {
        !draft.trimmingCharacters(in: .whitespaces).isEmpty || !pendingAttachments.isEmpty
    }

    // MARK: Send (real single-shot FFI round-trip)

    func send() {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        let staged = pendingAttachments
        let attachmentPaths = staged.map { $0.url.path }
        attachLog.info(
            "send: building request attachmentPaths.count=\(attachmentPaths.count, privacy: .public) text.isEmpty=\(text.isEmpty, privacy: .public)"
        )
        guard (!text.isEmpty || !attachmentPaths.isEmpty), let threadID = selectedThreadID else { return }
        draft = ""
        pendingAttachments = []

        // Carry the staged attachments onto the You bubble so the sender sees a
        // chip (name + optional thumbnail) for what they attached.
        let sent = staged.map { MessageAttachment(name: $0.name, url: $0.url) }
        append(ChatMessage(role: .you, timestamp: now(), text: text, attachments: sent), to: threadID)
        let assistant = ChatMessage(role: .assistant, timestamp: "now", text: "", isThinking: true)
        let assistantID = assistant.id
        append(assistant, to: threadID)

        let backendId = ensureBackendId(threadID)

        Task { @MainActor in
            guard let engine else {
                finish(assistantID, in: threadID,
                       text: "Engine not wired yet.")
                return
            }
            // Graceful no-key path.
            if !engine.isAvailable() {
                finish(assistantID, in: threadID,
                       text: "I can't reach the model yet — add an API key in Settings to enable assistive replies.")
                return
            }
            let start = Date()
            do {
                // REAL streaming: tokens land live as the agent emits them.
                _ = try await engine.streamReply(
                    text,
                    threadId: backendId,
                    attachmentPaths: attachmentPaths,
                    onDelta: { [weak self] delta in
                        self?.update(assistantID, in: threadID) {
                            $0.isThinking = false
                            $0.isStreaming = true
                            if $0.reasonedSeconds == nil {
                                $0.reasonedSeconds = Date().timeIntervalSince(start)
                            }
                            $0.text += delta
                        }
                    },
                    onReasoning: { _ in },
                    onTool: { [weak self] name, isError in
                        self?.recordToolActivity(name: name, isError: isError,
                                                 before: assistantID, in: threadID)
                    }
                )
                update(assistantID, in: threadID) {
                    $0.isThinking = false
                    $0.isStreaming = false
                    $0.timestamp = self.now()
                }
                refreshThreads(selectingBackendId: backendId)
            } catch {
                finish(assistantID, in: threadID,
                       text: "Something went wrong: \(error.localizedDescription)")
            }
        }
    }

    // MARK: Demo stream (reproduces the mock's mid-stream last turn)

    /// Kicks off the mock's animated final turn exactly once, so the first
    /// render matches the prototype's streaming + blink-caret state.
    func startDemoStreamIfNeeded() {
        guard !didStartDemo, let threadID = threads.first(where: { $0.isRestored })?.id else { return }
        didStartDemo = true
        let demo = ChatMessage(role: .assistant, timestamp: "now", text: "", isThinking: true)
        let id = demo.id
        append(demo, to: threadID)
        Task { @MainActor in
            try? await Task.sleep(nanoseconds: 1_600_000_000)
            startStream(
                id, in: threadID,
                fullText: "On it — patching events/bus.ts to emit once per settled retry, de-duping the store subscription on remount, and adding a regression test for the double-fire case.",
                reasoned: 2.1
            )
        }
    }

    // MARK: Simulated reveal

    private func startStream(_ id: UUID, in threadID: UUID, fullText: String, reasoned: Double) {
        revealTask?.cancel()
        update(id, in: threadID) {
            $0.isThinking = false
            $0.isStreaming = true
            $0.reasonedSeconds = reasoned
            $0.text = ""
        }
        revealTask = Task { @MainActor in
            let words = fullText.split(separator: " ", omittingEmptySubsequences: false)
            var shown = ""
            for (i, w) in words.enumerated() {
                if Task.isCancelled { return }
                shown += (i == 0 ? "" : " ") + w
                update(id, in: threadID) { $0.text = shown }
                try? await Task.sleep(nanoseconds: 95_000_000)
            }
            update(id, in: threadID) {
                $0.isStreaming = false
                $0.timestamp = self.now()
            }
        }
    }

    private func finish(_ id: UUID, in threadID: UUID, text: String) {
        update(id, in: threadID) {
            $0.isThinking = false
            $0.isStreaming = false
            $0.text = text
            $0.timestamp = self.now()
        }
    }

    // MARK: Mutation helpers

    private func append(_ message: ChatMessage, to threadID: UUID) {
        guard let ti = threads.firstIndex(where: { $0.id == threadID }) else { return }
        threads[ti].messages.append(message)
    }

    private func update(_ id: UUID, in threadID: UUID, _ body: (inout ChatMessage) -> Void) {
        guard let ti = threads.firstIndex(where: { $0.id == threadID }),
              let mi = threads[ti].messages.firstIndex(where: { $0.id == id }) else { return }
        body(&threads[ti].messages[mi])
    }

    /// Surface a completed tool call as a `.tool` activity turn placed immediately
    /// before the streaming assistant bubble (matches the mock's "What I checked").
    private func recordToolActivity(name: String, isError: Bool, before assistantID: UUID, in threadID: UUID) {
        guard let ti = threads.firstIndex(where: { $0.id == threadID }),
              let ai = threads[ti].messages.firstIndex(where: { $0.id == assistantID }) else { return }
        let line = ToolLine(verb: isError ? "failed" : "ran", detail: name)
        if ai > 0, threads[ti].messages[ai - 1].role == .tool {
            threads[ti].messages[ai - 1].toolLines.append(line)
            let n = threads[ti].messages[ai - 1].toolLines.count
            threads[ti].messages[ai - 1].toolTitle = "What I checked · \(n) tool\(n == 1 ? "" : "s")"
        } else {
            var tool = ChatMessage(role: .tool, timestamp: now(), text: "")
            tool.toolLines = [line]
            tool.toolTitle = "What I checked · 1 tool"
            threads[ti].messages.insert(tool, at: ai)
        }
    }

    private func now() -> String { Self.timeFmt.string(from: Date()) }
    private static let timeFmt: DateFormatter = {
        let f = DateFormatter()
        f.dateFormat = "HH:mm"
        return f
    }()

    private func refreshThreads(selectingBackendId backendId: String) {
        guard let threadsProvider else { return }
        replaceThreads(
            with: threadsProvider.listThreads(),
            selectingBackendId: backendId,
            keepLocalDrafts: true
        )
    }

    private func replaceThreads(
        with incoming: [ChatThread],
        selectingBackendId backendId: String?,
        keepLocalDrafts: Bool,
        allowEmpty: Bool = false
    ) {
        let previousSelectedID = selectedThreadID
        let existingByBackend = Dictionary(
            uniqueKeysWithValues: threads.compactMap { thread -> (String, ChatThread)? in
                guard let backendId = thread.backendId else { return nil }
                return (backendId, thread)
            }
        )

        var next = incoming.map { remote -> ChatThread in
            guard let backendId = remote.backendId, var existing = existingByBackend[backendId] else {
                return remote
            }
            existing.title = remote.title
            existing.meta = remote.meta
            existing.isRestored = remote.isRestored
            existing.isFavorite = remote.isFavorite
            return existing
        }

        if keepLocalDrafts {
            let locals = threads.filter { thread in
                thread.backendId == nil && (thread.id == previousSelectedID || !thread.messages.isEmpty)
            }
            next.append(contentsOf: locals)
        }

        threads = next.isEmpty && !allowEmpty ? [ChatThread(title: "New thread", meta: "now", messages: [])] : next
        if let backendId, let match = threads.first(where: { $0.backendId == backendId }) {
            selectedThreadID = match.id
        } else if let previousSelectedID, threads.contains(where: { $0.id == previousSelectedID }) {
            selectedThreadID = previousSelectedID
        } else {
            selectedThreadID = threads.first?.id
        }
        if let selectedThreadID { loadMessagesIfNeeded(selectedThreadID) }
    }

    // MARK: Seed (mock data — keeps #Preview standalone)

    static func seedThreads() -> [ChatThread] {
        var active = ChatThread(title: "auth-refactor", meta: "active · restored", isRestored: true)
        active.messages = [
            ChatMessage(role: .you, timestamp: "18:39", text: "where do we double-dispatch events?"),
            ChatMessage(
                role: .tool, timestamp: "18:39", text: "",
                toolTitle: "What I checked · 2 tools",
                toolLines: [
                    ToolLine(verb: "grep", detail: "events/bus.ts · ui/store.ts"),
                    ToolLine(verb: "read", detail: "2 files · 318 lines"),
                ]
            ),
            ChatMessage(
                role: .assistant, timestamp: "18:40",
                text: "Two spots. `events/bus.ts` re-emits on retry, and `ui/store.ts` subscribes twice on remount. Want a minimal patch plus a regression test?",
                reasonedSeconds: 2.1
            ),
            ChatMessage(role: .you, timestamp: "18:41", text: "yes, and add the test"),
        ]
        return [
            active,
            ChatThread(title: "rate-limiter spec", meta: "today · 18:40"),
            ChatThread(title: "release notes → PL", meta: "yesterday"),
            ChatThread(title: "whisper warm-start idea", meta: "yesterday"),
            ChatThread(title: "standup notes", meta: "Thu"),
        ]
    }
}

// MARK: - Preview engine (canned single-shot reply)

#if DEBUG
final class MockChatEngine: AgentChatEngine {
    func isAvailable() -> Bool { true }
    func streamReply(
        _ text: String,
        threadId: String,
        attachmentPaths: [String],
        onDelta: @escaping @MainActor (String) -> Void,
        onReasoning: @escaping @MainActor (String) -> Void,
        onTool: @escaping @MainActor (_ name: String, _ isError: Bool) -> Void
    ) async throws -> String {
        let seen = attachmentPaths.isEmpty ? "" : " (saw \(attachmentPaths.count) image\(attachmentPaths.count == 1 ? "" : "s"))"
        let reply = "On it — \(text.lowercased())\(seen). I'd start with a minimal patch and a regression test."
        var assembled = ""
        for word in reply.split(separator: " ", omittingEmptySubsequences: false) {
            try? await Task.sleep(nanoseconds: 60_000_000)
            let chunk = (assembled.isEmpty ? "" : " ") + word
            assembled += chunk
            await onDelta(chunk)
        }
        return assembled
    }
}
#endif
