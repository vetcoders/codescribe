import Foundation
import OSLog

/// Diagnostic breadcrumbs for the attachment staging path. Filter with:
///   log show --predicate 'subsystem == "com.vetcoders.codescribe"' --info
private let attachLog = Logger(
    subsystem: Bundle.main.bundleIdentifier ?? "com.vetcoders.codescribe",
    category: "attachments"
)

// Backs the Agent Chat with the REAL codescribe engine via the UniFFI bridge
// (CodescribeAgent / CsAgentListener). Streaming token deltas are hopped onto the
// main actor (FIFO) so SwiftUI @Published updates stay ordered and thread-safe.
final class RealChatEngine: AgentChatEngine {
    private let agent = CodescribeAgent()

    func isAvailable() -> Bool { agent.isAvailable() }

    func streamReply(
        _ text: String,
        threadId: String,
        attachmentPaths: [String],
        onDelta: @escaping @MainActor (String) -> Void,
        onReasoning: @escaping @MainActor (String) -> Void,
        onTool: @escaping @MainActor (_ name: String, _ isError: Bool, _ reason: String) -> Void
    ) async throws -> String {
        let listener = StreamListener(onDelta: onDelta, onReasoning: onReasoning, onTool: onTool)
        // Text-only path stays byte-identical to before; only route through the
        // vision method when the composer actually staged an image.
        if attachmentPaths.isEmpty {
            attachLog.info("RealChatEngine.streamReply: text-only path (streamReply, no attachments)")
            return try await agent.streamReply(text: text, threadId: threadId, listener: listener)
        }
        attachLog.info(
            "RealChatEngine.streamReply: vision path (streamReplyWithAttachments) with \(attachmentPaths.count, privacy: .public) attachment(s)"
        )
        let attachments = attachmentPaths.map { CsAttachment(path: $0) }
        return try await agent.streamReplyWithAttachments(
            text: text,
            threadId: threadId,
            attachments: attachments,
            listener: listener
        )
    }
}

/// Bridges Rust-side `CsAgentListener` callbacks (fired from a tokio thread) onto
/// the main actor, preserving arrival order.
final class StreamListener: CsAgentListener, @unchecked Sendable {
    private let onDelta: @MainActor (String) -> Void
    private let onReasoning: @MainActor (String) -> Void
    private let onTool: @MainActor (String, Bool, String) -> Void

    init(
        onDelta: @escaping @MainActor (String) -> Void,
        onReasoning: @escaping @MainActor (String) -> Void,
        onTool: @escaping @MainActor (String, Bool, String) -> Void
    ) {
        self.onDelta = onDelta
        self.onReasoning = onReasoning
        self.onTool = onTool
    }

    func onTextDelta(delta: String) {
        DispatchQueue.main.async { MainActor.assumeIsolated { self.onDelta(delta) } }
    }
    func onTextDone(text: String) {}
    func onReasoningDelta(delta: String) {
        DispatchQueue.main.async { MainActor.assumeIsolated { self.onReasoning(delta) } }
    }
    func onToolExecuting(name: String, id: String) {
        // Surfaced via onToolResult (completed) to avoid duplicate activity rows.
    }
    func onToolResult(name: String, id: String, summary: String, isError: Bool) {
        // `summary` already carries the tool's error reason on failure (see the
        // Rust AgentUiEvent::ToolResult contract); forward it so the chat row can
        // reveal the cause instead of a bare "failed".
        DispatchQueue.main.async { MainActor.assumeIsolated { self.onTool(name, isError, summary) } }
    }
    func onDone() {}
    func onError(message: String) {
        DispatchQueue.main.async { MainActor.assumeIsolated { self.onDelta("\n[error] " + message) } }
    }
}

/// Bridges Rust-side `CsAgentDeliveryListener` callbacks (fired from a tokio
/// thread) onto the main actor, driving `AgentChatStore` so a voice / hotkey agent
/// reply streams LIVE into the chat window instead of only landing on disk.
///
/// Mirrors `StreamListener` / `DictationListener`. Two hard rules from the design:
/// 1. It only renders incoming events — it never calls `store.send()`, which
///    would fire a second (composer-side) provider call for a turn the core is
///    already streaming.
/// 2. `AppDelegate` must keep a strong reference to it (UniFFI releases the
///    foreign callback otherwise); all store mutation hops onto the main actor.
///
/// `onTurnStarted` also reveals the chat window via `revealChat` so the user
/// actually sees the reply they just spoke for. A more selective reveal policy
/// (don't steal focus while typing elsewhere) is deferred follow-up work.
final class VoiceDeliveryListener: CsAgentDeliveryListener, @unchecked Sendable {
    private let store: AgentChatStore
    private let revealChat: @MainActor () -> Void

    init(store: AgentChatStore, revealChat: @escaping @MainActor () -> Void) {
        self.store = store
        self.revealChat = revealChat
    }

    func onTurnStarted(threadId: String, userText: String) {
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                self.revealChat()
                self.store.ingestVoiceTurn(threadId: threadId, userText: userText)
            }
        }
    }
    func onTextDelta(delta: String) {
        DispatchQueue.main.async { MainActor.assumeIsolated { self.store.ingestVoiceDelta(delta) } }
    }
    func onTextDone(text: String) {
        DispatchQueue.main.async { MainActor.assumeIsolated { self.store.ingestVoiceTextDone(text) } }
    }
    func onReasoningDelta(delta: String) {
        // Reasoning is not surfaced in the chat bubble (parity with the composer
        // StreamListener, which also drops it). Deferred to the L variant.
    }
    func onToolExecuting(name: String, id: String) {
        // Surfaced via onToolResult (completed) to avoid duplicate activity rows,
        // matching StreamListener.
    }
    func onToolResult(name: String, id: String, summary: String, isError: Bool) {
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                self.store.ingestVoiceTool(name: name, isError: isError, reason: summary)
            }
        }
    }
    func onDone() {
        DispatchQueue.main.async { MainActor.assumeIsolated { self.store.ingestVoiceDone() } }
    }
    func onError(message: String) {
        DispatchQueue.main.async { MainActor.assumeIsolated { self.store.ingestVoiceError(message) } }
    }
}
