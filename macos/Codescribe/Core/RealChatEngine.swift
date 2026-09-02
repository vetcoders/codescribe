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
  /// Same bridge surface the voice lane uses; carries the rail-selection
  /// routing target down to the controller (operator contract 2026-08-13).
  private let assistiveRouting = CodescribeHotkeys()
  private var onToolApprovalRequested: (@MainActor (PendingToolApproval) -> Void)?

  func isAvailable() -> Bool { agent.isAvailable() }

  func setAssistiveTargetThread(backendId: String?) {
    assistiveRouting.setAssistiveTargetThread(backendId: backendId)
  }

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
    onToolResult:
      @escaping @MainActor (_ name: String, _ id: String, _ isError: Bool, _ reason: String) -> Void
  ) async throws -> String {
    let channel = AsyncStream<StreamListenerEvent>.makeStream()
    let listener = StreamListener(continuation: channel.continuation)
    let consumer = Task { @MainActor [onToolApprovalRequested] in
      for await event in channel.stream {
        switch event {
        case .textDelta(let delta): onDelta(delta)
        case .reasoningDelta(let delta): onReasoning(delta)
        case .toolExecuting(let name, let id): onToolExecuting(name, id)
        case .toolResult(let name, let id, let isError, let reason):
          onToolResult(name, id, isError, reason)
        case .toolApproval(let ffiRequest):
          onToolApprovalRequested?(
            PendingToolApproval(
              callID: ffiRequest.callId,
              sessionID: ffiRequest.sessionId,
              threadID: ffiRequest.threadId,
              tool: ffiRequest.tool,
              server: ffiRequest.server,
              risk: ffiRequest.risk,
              summary: ffiRequest.summary,
              command: ffiRequest.command,
              cwd: ffiRequest.cwd,
              paths: ffiRequest.paths
            ))
        }
      }
    }
    do {
      let result: String
      // Text-only path stays byte-identical to before; only route through the
      // vision method when the composer actually staged an image.
      if attachmentPaths.isEmpty {
        attachLog.info("RealChatEngine.streamReply: text-only path (streamReply, no attachments)")
        result = try await agent.streamReply(text: text, threadId: threadId, listener: listener)
      } else {
        attachLog.info(
          "RealChatEngine.streamReply: vision path (streamReplyWithAttachments) with \(attachmentPaths.count, privacy: .public) attachment(s)"
        )
        let attachments = attachmentPaths.map { CsAttachment(path: $0) }
        result = try await agent.streamReplyWithAttachments(
          text: text,
          threadId: threadId,
          attachments: attachments,
          listener: listener
        )
      }
      channel.continuation.finish()
      await consumer.value
      return result
    } catch {
      channel.continuation.finish()
      await consumer.value
      throw error
    }
  }

  func cancelReply(threadId: String) -> Bool {
    // Swift Task cancellation never reaches the Rust future through the
    // generated UniFFI bindings (they poll to completion), so this explicit
    // bridge call is what actually aborts the in-flight turn.
    agent.cancelTurn(threadId: threadId)
  }

  func installToolApprovalHandler(
    _ handler: @escaping @MainActor (PendingToolApproval) -> Void
  ) {
    onToolApprovalRequested = handler
  }

  func resolveToolApproval(
    _ request: PendingToolApproval, approved: Bool, remember: Bool
  ) -> Bool {
    agent.resolveToolApproval(
      sessionId: request.sessionID,
      threadId: request.threadID,
      callId: request.callID,
      approved: approved,
      remember: remember
    )
  }
}

private enum StreamListenerEvent: Sendable {
  case textDelta(String)
  case reasoningDelta(String)
  case toolExecuting(String, String)
  case toolResult(String, String, Bool, String)
  case toolApproval(CsToolApprovalRequest)
}

/// Value-only Rust callback adapter. One AsyncStream consumer on MainActor
/// preserves callback order without unchecked cross-thread state.
private final class StreamListener: CsAgentListener, Sendable {
  private let continuation: AsyncStream<StreamListenerEvent>.Continuation

  init(continuation: AsyncStream<StreamListenerEvent>.Continuation) {
    self.continuation = continuation
  }

  func onTextDelta(delta: String) {
    continuation.yield(.textDelta(delta))
  }
  func onTextDone(text: String) {}
  func onReasoningDelta(delta: String) {
    continuation.yield(.reasoningDelta(delta))
  }
  func onToolExecuting(name: String, id: String) {
    continuation.yield(.toolExecuting(name, id))
  }
  func onToolApprovalRequested(request ffiRequest: CsToolApprovalRequest) {
    continuation.yield(.toolApproval(ffiRequest))
  }
  func onToolResult(name: String, id: String, summary: String, isError: Bool) {
    // `summary` already carries the tool's error reason on failure (see the
    // Rust AgentUiEvent::ToolResult contract); forward it so the chat row can
    // reveal the cause instead of a bare "failed".
    continuation.yield(.toolResult(name, id, isError, summary))
  }
  func onDone() {}
  func onError(message: String) {
    continuation.yield(.textDelta("\n[error] " + message))
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
private enum VoiceDeliveryEvent: Sendable {
  case turnStarted(String, String)
  case textDelta(String)
  case textDone(String)
  case reasoningDelta(String)
  case toolExecuting(String, String)
  case toolResult(String, String, String, Bool)
  case done
  case error(String)
  case cancelled(String)
}

final class VoiceDeliveryListener: CsAgentDeliveryListener, VoiceTurnCancelling, Sendable {
  private let continuation: AsyncStream<VoiceDeliveryEvent>.Continuation
  private let consumer: Task<Void, Never>
  private let voiceTurns = CodescribeHotkeys()

  @MainActor
  init(store: AgentChatStore, revealChat: @escaping @MainActor @Sendable () -> Void) {
    let channel = AsyncStream<VoiceDeliveryEvent>.makeStream()
    continuation = channel.continuation
    consumer = Task { @MainActor [weak store] in
      for await event in channel.stream {
        guard let store else { return }
        switch event {
        case .turnStarted(let threadId, let userText):
          AppModel.shared.overlay.hideForAgentHandoff()
          revealChat()
          store.ingestVoiceTurn(threadId: threadId, userText: userText)
        case .textDelta(let delta): store.ingestVoiceDelta(delta)
        case .textDone(let text): store.ingestVoiceTextDone(text)
        case .reasoningDelta(let delta): store.ingestVoiceReasoning(delta)
        case .toolExecuting(let name, let id):
          store.ingestVoiceToolExecuting(name: name, id: id)
        case .toolResult(let name, let id, let summary, let isError):
          store.ingestVoiceToolResult(name: name, id: id, isError: isError, reason: summary)
        case .done:
          store.ingestVoiceDone()
          ThreadsChangeBus.postThreadsChanged()
        case .error(let message):
          store.ingestVoiceError(message)
          ThreadsChangeBus.postThreadsChanged()
        case .cancelled(let threadId):
          store.ingestVoiceCancelled(threadId: threadId)
          ThreadsChangeBus.postThreadsChanged()
        }
      }
    }
    store.voiceTurnCanceller = self
  }

  func cancelVoiceTurn(threadId: String) -> Bool {
    voiceTurns.cancelVoiceTurn(threadId: threadId)
  }

  func onTurnStarted(threadId: String, userText: String) {
    continuation.yield(.turnStarted(threadId, userText))
  }

  func onTextDelta(delta: String) {
    continuation.yield(.textDelta(delta))
  }
  func onTextDone(text: String) {
    continuation.yield(.textDone(text))
  }
  func onReasoningDelta(delta: String) {
    continuation.yield(.reasoningDelta(delta))
  }
  func onToolExecuting(name: String, id: String) {
    continuation.yield(.toolExecuting(name, id))
  }
  func onToolResult(name: String, id: String, summary: String, isError: Bool) {
    continuation.yield(.toolResult(name, id, summary, isError))
  }
  func onDone() {
    continuation.yield(.done)
  }
  func onError(message: String) {
    continuation.yield(.error(message))
  }
  func onCancelled(threadId: String) {
    continuation.yield(.cancelled(threadId))
  }

  func invalidate() {
    continuation.finish()
    consumer.cancel()
  }
}
