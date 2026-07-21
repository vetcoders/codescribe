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

    func availabilityDetail() -> String? {
        let availability = agent.availability()
        if availability.available { return nil }
        // The bridge always fills `detail`; the fallback keeps the chat honest
        // if an older dylib ever returns an empty reason.
        return availability.detail.isEmpty
            ? "The assistive model isn't reachable yet — open Settings → Engine to configure the assistive lane."
            : availability.detail
    }

    func generateThreadTitle(_ text: String) async throws -> String? {
        try await agent.generateThreadTitle(text: text)
    }

    func streamReply(
        _ text: String,
        threadId: String,
        attachmentPaths: [String],
        onDelta: @escaping @MainActor (String) -> Void,
        onReasoning: @escaping @MainActor (String) -> Void,
        onToolExecuting: @escaping @MainActor (_ name: String, _ id: String) -> Void,
        onToolResult: @escaping @MainActor (_ name: String, _ id: String, _ isError: Bool, _ reason: String) -> Void
    ) async throws -> String {
        let listener = StreamListener(
            onDelta: onDelta,
            onReasoning: onReasoning,
            onToolExecuting: onToolExecuting,
            onToolResult: onToolResult
        )
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

    func cancelReply(threadId: String) -> Bool {
        // Swift Task cancellation never reaches the Rust future through the
        // generated UniFFI bindings (they poll to completion), so this explicit
        // bridge call is what actually aborts the in-flight turn.
        agent.cancelTurn(threadId: threadId)
    }
}

/// Bridges Rust-side `CsAgentListener` callbacks (fired from a tokio thread) onto
/// the main actor, preserving arrival order.
final class StreamListener: CsAgentListener, @unchecked Sendable {
    private let onDelta: @MainActor (String) -> Void
    private let onReasoning: @MainActor (String) -> Void
    private let onToolExecuting: @MainActor (String, String) -> Void
    private let onToolResult: @MainActor (String, String, Bool, String) -> Void

    init(
        onDelta: @escaping @MainActor (String) -> Void,
        onReasoning: @escaping @MainActor (String) -> Void,
        onToolExecuting: @escaping @MainActor (String, String) -> Void,
        onToolResult: @escaping @MainActor (String, String, Bool, String) -> Void
    ) {
        self.onDelta = onDelta
        self.onReasoning = onReasoning
        self.onToolExecuting = onToolExecuting
        self.onToolResult = onToolResult
    }

    func onTextDelta(delta: String) {
        DispatchQueue.main.async { MainActor.assumeIsolated { self.onDelta(delta) } }
    }
    func onTextDone(text: String) {}
    func onReasoningDelta(delta: String) {
        DispatchQueue.main.async { MainActor.assumeIsolated { self.onReasoning(delta) } }
    }
    func onToolExecuting(name: String, id: String) {
        DispatchQueue.main.async { MainActor.assumeIsolated { self.onToolExecuting(name, id) } }
    }
    func onToolResult(name: String, id: String, summary: String, isError: Bool) {
        // `summary` already carries the tool's error reason on failure (see the
        // Rust AgentUiEvent::ToolResult contract); forward it so the chat row can
        // reveal the cause instead of a bare "failed".
        DispatchQueue.main.async { MainActor.assumeIsolated { self.onToolResult(name, id, isError, summary) } }
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
/// `onTurnStarted` also asks AppDelegate for a passive reveal. AppDelegate owns
/// focus policy: explicit opens activate, voice delivery never steals focus.
final class VoiceDeliveryListener: CsAgentDeliveryListener, VoiceTurnCancelling, @unchecked Sendable {
    private let store: AgentChatStore
    private let revealChat: @MainActor () -> Void
    private let voiceTurns = CodescribeHotkeys()

    @MainActor
    init(store: AgentChatStore, revealChat: @escaping @MainActor () -> Void) {
        self.store = store
        self.revealChat = revealChat
        store.voiceTurnCanceller = self
    }

    func cancelVoiceTurn(threadId: String) -> Bool {
        voiceTurns.cancelVoiceTurn(threadId: threadId)
    }

    func onTurnStarted(threadId: String, userText: String) {
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                // The transcript is now the chat's You-bubble — the overlay's job
                // is done, so it fades out instead of lingering over the reply.
                AppModel.shared.overlay.hideForAgentHandoff()
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
        DispatchQueue.main.async { MainActor.assumeIsolated { self.store.ingestVoiceReasoning(delta) } }
    }
    func onToolExecuting(name: String, id: String) {
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                self.store.ingestVoiceToolExecuting(name: name, id: id)
            }
        }
    }
    func onToolResult(name: String, id: String, summary: String, isError: Bool) {
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                self.store.ingestVoiceToolResult(name: name, id: id, isError: isError, reason: summary)
            }
        }
    }
    func onDone() {
        DispatchQueue.main.async { MainActor.assumeIsolated { self.store.ingestVoiceDone() } }
    }
    func onError(message: String) {
        DispatchQueue.main.async { MainActor.assumeIsolated { self.store.ingestVoiceError(message) } }
    }
    func onCancelled(threadId: String) {
        DispatchQueue.main.async {
            MainActor.assumeIsolated { self.store.ingestVoiceCancelled(threadId: threadId) }
        }
    }
}
