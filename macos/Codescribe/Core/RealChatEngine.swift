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
        onTool: @escaping @MainActor (_ name: String, _ isError: Bool) -> Void
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
    private let onTool: @MainActor (String, Bool) -> Void

    init(
        onDelta: @escaping @MainActor (String) -> Void,
        onReasoning: @escaping @MainActor (String) -> Void,
        onTool: @escaping @MainActor (String, Bool) -> Void
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
        DispatchQueue.main.async { MainActor.assumeIsolated { self.onTool(name, isError) } }
    }
    func onDone() {}
    func onError(message: String) {
        DispatchQueue.main.async { MainActor.assumeIsolated { self.onDelta("\n[error] " + message) } }
    }
}
