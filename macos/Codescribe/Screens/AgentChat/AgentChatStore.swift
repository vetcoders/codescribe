import AppKit
import Combine
import OSLog
import SwiftUI

/// Diagnostic breadcrumbs for the attachment staging path. Filter with:
///   log show --predicate 'subsystem == "com.vetcoders.codescribe"' --info
private let attachLog = Logger(
  subsystem: Bundle.main.bundleIdentifier ?? "com.vetcoders.codescribe",
  category: "attachments"
)

/// Turn-queue diagnostics: accept / promote / cancel with queue depth and
/// thread binding. Never logs message text or attachment contents.
private let queueLog = Logger(
  subsystem: Bundle.main.bundleIdentifier ?? "com.vetcoders.codescribe",
  category: "turn-queue"
)

// MARK: - Runtime contract (read before extending this screen)
//
// This screen is backed by the real codescribe UniFFI bridge when constructed
// from AppModel: `RealChatEngine` streams assistant deltas / tool events and
// `RealThreadsEngine` reads persisted ThreadStore entries. The #Preview still
// uses local mock data. Attachments stage through the composer (picker, drag &
// drop, ⌘V paste) into `pendingAttachments` and ride `send()` to the bridge.
// Known remaining gap: restored structured tool/reasoning payloads are
// flattened by the thread adapter.

// MARK: - Engine seam (W2-01 injects the real adapter)

/// Thin, UI-only seam over the agent primitives the screen actually uses.
/// W2-01 supplies an adapter that forwards to the real `VistaEngine`
/// (mapping `assistive` → `VistaAiMode.assistive`). Kept free of bridge types
/// so the view-model + #Preview compile and render standalone.
protocol AgentChatEngine: AnyObject {
  /// True when the assistive provider can be built (keys present).
  func isAvailable() -> Bool
  /// Actionable reason the assistive lane cannot reach a model right now,
  /// `nil` when a send can proceed. Names the missing lane/endpoint/key so
  /// the chat renders honest guidance instead of a generic "add an API key".
  func availabilityDetail() -> String?
  /// Generate one isolated title from the raw first textual turn. This is a
  /// sibling request to the assistive stream and carries no conversation state.
  func generateThreadTitle(_ text: String) async throws -> String?
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
    onToolExecuting: @escaping @MainActor (_ name: String, _ id: String) -> Void,
    onToolResult:
      @escaping @MainActor (_ name: String, _ id: String, _ isError: Bool, _ reason: String) -> Void
  ) async throws -> String
  /// Abort the engine-side turn running for `threadId` (safe no-op when idle).
  /// Cancelling the Swift `Task` that awaits `streamReply` is NOT enough: the
  /// generated UniFFI bindings poll the Rust future to completion, so without
  /// this call the agent keeps executing tools (typing/clipboard/fs) after a
  /// "cancelled" turn.
  @discardableResult
  func cancelReply(threadId: String) -> Bool
  func installToolApprovalHandler(
    _ handler: @escaping @MainActor (PendingToolApproval) -> Void
  )
  @discardableResult
  func resolveToolApproval(
    _ request: PendingToolApproval, approved: Bool, remember: Bool
  ) -> Bool
}

extension AgentChatEngine {
  func installToolApprovalHandler(
    _ handler: @escaping @MainActor (PendingToolApproval) -> Void
  ) {}
  func resolveToolApproval(
    _ request: PendingToolApproval, approved: Bool, remember: Bool
  ) -> Bool { false }
  /// Publish the rail's current selection as the voice-assistive routing
  /// target (operator contract 2026-08-13: dictation goes to the thread the
  /// user is looking at; a new thread only via an explicit "+ New thread").
  /// Default no-op keeps preview/mock stores standalone.
  func setAssistiveTargetThread(backendId: String?) {}
}

/// Source-specific adapter for hotkey/voice turns owned by the shared controller
/// runtime. Kept separate from `AgentChatEngine`, whose registry owns composer
/// sends, so the single Stop action cannot cancel through the wrong backend.
protocol VoiceTurnCancelling: AnyObject {
  @discardableResult
  func cancelVoiceTurn(threadId: String) -> Bool
}

// MARK: - Models

enum ComposerTurnPhase: Equatable {
  case thinking
  case streaming
  case cancelling
}

/// The single composer-originated turn owned by the Swift UI. The local thread
/// id targets the bubble/task; the backend id is the exact Rust cancellation
/// key. `id` prevents a draining cancelled task from clearing a newer send.
struct ActiveComposerTurn: Equatable {
  let id: UUID
  let threadID: UUID
  let backendThreadID: String
  let assistantMessageID: UUID
  var phase: ComposerTurnPhase
}

struct PendingToolApproval: Identifiable, Equatable {
  var id: String { "\(sessionID):\(threadID):\(callID)" }
  let callID: String
  let sessionID: String
  let threadID: String
  let tool: String
  let server: String
  let risk: String
  let summary: String
  let command: String?
  let cwd: String?
  let paths: [String]
}

enum ChatRole {
  case you
  case tool
  case assistant
}

/// How an assistant bubble renders its body. `raw` (mono plain — exactly what
/// streamed) is the DEFAULT per the operator's C2b decision: stream and settled
/// turn look identical, rich markdown/highlight is per-bubble opt-in.
enum MessageRenderMode: Equatable {
  case raw
  case rich

  /// Pure toggle used by the meta-row raw↔rich button (XCTest-covered).
  static func nextRenderMode(after mode: MessageRenderMode) -> MessageRenderMode {
    mode == .raw ? .rich : .raw
  }
}

enum ToolLineState: Hashable {
  case running
  case succeeded
  case failed
  case cancelled
  case unknown
}

struct ToolLine: Identifiable, Hashable {
  let id: UUID
  var callID: String?
  var verb: String  // "grep", "read" — rendered olive; "failed" — terracotta
  let detail: String  // tool name or "events/bus.ts · ui/store.ts"
  var state: ToolLineState
  /// Result summary for a settled line (success summary or failure reason).
  /// `nil` for running lines and for reloaded/persisted turns that do not
  /// carry payload. Drives the expandable inspect panel.
  var reason: String?
  /// Wall-clock start of the live tool call (UI-only; not persisted).
  var startedAt: Date?
  /// Elapsed milliseconds once the call settles (UI-only; not persisted).
  var durationMs: Int?

  init(
    id: UUID = UUID(),
    callID: String? = nil,
    verb: String,
    detail: String,
    state: ToolLineState = .succeeded,
    reason: String? = nil,
    startedAt: Date? = nil,
    durationMs: Int? = nil
  ) {
    self.id = id
    self.callID = callID
    self.verb = verb
    self.detail = detail
    self.state = state
    self.reason = reason
    self.startedAt = startedAt
    self.durationMs = durationMs
  }

  /// True when the row can open an inspect disclosure (summary, call id, or timing).
  var hasInspectPayload: Bool {
    ToolInspectPresentation.hasInspectPayload(
      reason: reason,
      callID: callID,
      durationMs: durationMs
    )
  }

  /// Plain-text technical dump for copy (name, status, duration, call id, summary).
  var technicalCopyText: String {
    ToolInspectPresentation.technicalCopy(
      verb: verb,
      detail: detail,
      state: state,
      reason: reason,
      callID: callID,
      durationMs: durationMs
    )
  }
}

/// Pure presentation helpers for tool-activity inspect (testable without SwiftUI).
enum ToolInspectPresentation {
  static func hasInspectPayload(reason: String?, callID: String?, durationMs: Int?) -> Bool {
    if let reason, !reason.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
      return true
    }
    if let callID, !callID.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
      return true
    }
    if let durationMs, durationMs >= 0 { return true }
    return false
  }

  static func statusLabel(for state: ToolLineState) -> String {
    switch state {
    case .running: return "running"
    case .succeeded: return "succeeded"
    case .failed: return "failed"
    case .cancelled: return "cancelled"
    case .unknown: return "ended"
    }
  }

  static func durationLabel(ms: Int?) -> String? {
    guard let ms, ms >= 0 else { return nil }
    if ms < 1000 { return "\(ms) ms" }
    let seconds = Double(ms) / 1000.0
    if seconds < 10 {
      return String(format: "%.1f s", seconds)
    }
    return "\(Int(seconds.rounded())) s"
  }

  static func technicalCopy(
    verb: String,
    detail: String,
    state: ToolLineState,
    reason: String?,
    callID: String?,
    durationMs: Int?
  ) -> String {
    var lines: [String] = [
      "tool: \(detail)",
      "verb: \(verb)",
      "status: \(statusLabel(for: state))",
    ]
    if let durationLabel = durationLabel(ms: durationMs) {
      lines.append("duration: \(durationLabel)")
    }
    if let callID, !callID.isEmpty {
      lines.append("call_id: \(callID)")
    }
    if let reason, !reason.isEmpty {
      lines.append("summary: \(reason)")
    }
    return lines.joined(separator: "\n")
  }
}

struct ChatMessage: Identifiable {
  let id = UUID()
  var role: ChatRole
  var timestamp: String
  /// Body text. May contain `backtick` code spans for assistant/you turns.
  var text: String

  /// Files attached to a sent user turn (empty otherwise). Rendered as chips in
  /// the You bubble. Restored attachment names/types are recovered from the
  /// Swift-side metadata sidecar because the bridge's persisted message JSON
  /// carries image blocks but not original file names.
  var attachments: [MessageAttachment] = []

  // Assistive wire split (U17). For a voice-assistive user turn the engine
  // sends a fixed prompt skeleton to the LLM; the bubble must show the spoken
  // instruction, not the skeleton. `text` holds the display text; the fields
  // below carry the rest of the wire truth (nil for composer/plain turns).
  /// Full prompt as sent to the model ("Copy full prompt" / debug). Non-nil
  /// only when `text` was rewritten from an assistive skeleton.
  var wireText: String? = nil
  /// ZAZNACZONY_TEKST captured with the turn, shown behind the context chip.
  var contextSelection: String? = nil
  /// Frontmost app from the KONTEKST section, shown behind the context chip.
  var contextApp: String? = nil

  // Tool-activity turn
  var toolTitle: String = ""  // "What I checked · 2 tools"
  var toolLines: [ToolLine] = []

  // Assistant turn
  var reasonedSeconds: Double? = nil
  var isThinking: Bool = false  // pre-reply "thinking…" state
  var isStreaming: Bool = false  // word-reveal in progress (shows caret)
  var wasStopped: Bool = false  // cancelled terminal; partial text remains intact
  var reasoning: String = ""  // streamed model reasoning, rendered separately
  var renderMode: MessageRenderMode = .rich
}

/// An image the user staged in the composer but has not sent yet. Referenced by
/// file URL (NSOpenPanel / clipboard-saved temp file); the send path forwards the
/// path to the bridge, which loads + validates the bytes.
struct PendingAttachment: Identifiable, Hashable {
  let id = UUID()
  let url: URL
  var name: String { url.lastPathComponent }
  var type: String { MessageAttachment.inferredType(name: name, url: url) }
  var previewAttachment: MessageAttachment { MessageAttachment(name: name, url: url, type: type) }
}

/// An attachment carried by a *sent* chat message, surfaced as a chip in the You
/// bubble. `url` points at the source file for an optional inline thumbnail; it
/// is nil for restored turns (the persisted thread has no source path), in which
/// case the chip shows the filename only.
struct MessageAttachment: Identifiable, Hashable {
  let id = UUID()
  let name: String
  let url: URL?
  let type: String

  init(name: String, url: URL?, type: String? = nil) {
    self.name = name
    self.url = url
    self.type = type ?? Self.inferredType(name: name, url: url)
  }

  static func inferredType(name: String, url: URL?) -> String {
    let ext =
      (url?.pathExtension.isEmpty == false ? url?.pathExtension : nil)
      ?? (name as NSString).pathExtension
    switch ext.lowercased() {
    case "png": return "image/png"
    case "jpg", "jpeg": return "image/jpeg"
    case "gif": return "image/gif"
    case "webp": return "image/webp"
    case "bmp": return "image/bmp"
    case "tif", "tiff": return "image/tiff"
    default: return ext.isEmpty ? "file" : "file/\(ext.lowercased())"
    }
  }
}

struct ChatThread: Identifiable {
  let id = UUID()
  var title: String
  var meta: String  // mono subtitle, e.g. "active · restored" / "today 18:40 · gpt-5 · 1.2k tok"
  var isRestored: Bool = false
  var isFavorite: Bool = false
  var backendId: String? = nil  // codescribe ThreadStore id (nil = local-only, not yet persisted)
  var messagesLoaded: Bool = false  // lazy-load guard for persisted threads
  var messages: [ChatMessage] = []
  var updatedAt: Date? = nil  // nil (local-only draft) groups under Today
  var model: String? = nil
  var totalTokens: UInt64? = nil
}

/// Shared Swift-side title guard for coordinator results, manual renames, and
/// rail fallbacks. The durable owner is ThreadStore; this policy prevents a
/// provider failure or stale legacy row from flashing transport punctuation in
/// the live model before disk truth refreshes.
enum ThreadTitlePolicy {
  static func normalized(_ value: String?, limit: Int = 72) -> String? {
    guard let value else { return nil }
    let collapsed = strippingContextMarkers(from: value)
      .split(whereSeparator: \Character.isWhitespace)
      .joined(separator: " ")
    guard !collapsed.hasPrefix("<<<"),
      collapsed.contains(where: { $0.isLetter || $0.isNumber })
    else { return nil }
    return String(collapsed.prefix(limit))
  }

  static func firstUserExcerpt(in messages: [ChatMessage], limit: Int = 72) -> String? {
    guard let message = messages.first(where: { $0.role == .you }) else { return nil }
    let presented = AssistivePromptParser.presented(message)
    return normalized(presented.text, limit: limit)
  }

  /// Vowel inventory used to recognise word fragments left behind by a
  /// mid-word context-marker capture. Mirrors `TITLE_FRAGMENT_VOWELS` in
  /// `core/agent/thread_store.rs` (the durable owner of title derivation).
  private static let fragmentVowels = Set("aeiouyąęóàáâäãåèéêëìíîïòôöõùúûü")

  /// Remove `{selection_N}` / `{image_N}` context-bucket markers from a
  /// title candidate. Mirror of the Rust `strip_context_markers`: the
  /// overlay space-pads a marker even when the capture lands mid-word
  /// ("mnie" -> "mn {selection_1} ie"), so after removal a letter run of
  /// two or more characters without any vowel is treated as a split-word
  /// fragment and glued back without a space; otherwise a single space
  /// stays. Titles only — message bodies keep their markers untouched.
  static func strippingContextMarkers(from text: String) -> String {
    guard text.contains("{selection_") || text.contains("{image_") else { return text }
    var chars = Array(text)
    while let marker = contextMarkerRange(in: chars) {
      var leftEnd = marker.lowerBound
      while leftEnd > 0, chars[leftEnd - 1].isWhitespace { leftEnd -= 1 }
      var rightStart = marker.upperBound
      while rightStart < chars.count, chars[rightStart].isWhitespace { rightStart += 1 }
      // Unpadded marker (no whitespace on either side) is the overlay's
      // lossless mid-word form ("mn{selection_1}ie") — glue without the
      // vowel heuristic; that heuristic only serves legacy padded texts.
      let noGap = leftEnd == marker.lowerBound && rightStart == marker.upperBound
      let keepSpace =
        leftEnd > 0
        && rightStart < chars.count
        && !noGap
        && !gluesSplitWord(chars: chars, leftEnd: leftEnd, rightStart: rightStart)
      chars.replaceSubrange(leftEnd..<rightStart, with: keepSpace ? [" "] : [])
    }
    return String(chars)
  }

  private static func contextMarkerRange(in chars: [Character]) -> Range<Int>? {
    var open = 0
    while open < chars.count {
      defer { open += 1 }
      guard chars[open] == "{" else { continue }
      for label in ["selection_", "image_"] {
        let labelChars = Array(label)
        let digitsStart = open + 1 + labelChars.count
        guard digitsStart <= chars.count,
          Array(chars[(open + 1)..<digitsStart]) == labelChars
        else { continue }
        var close = digitsStart
        while close < chars.count, chars[close].isASCII, chars[close].isNumber {
          close += 1
        }
        if close > digitsStart, close < chars.count, chars[close] == "}" {
          return open..<(close + 1)
        }
      }
    }
    return nil
  }

  private static func gluesSplitWord(chars: [Character], leftEnd: Int, rightStart: Int) -> Bool {
    var left: [Character] = []
    var index = leftEnd - 1
    while index >= 0, chars[index].isLetter {
      left.append(chars[index])
      index -= 1
    }
    var right: [Character] = []
    index = rightStart
    while index < chars.count, chars[index].isLetter {
      right.append(chars[index])
      index += 1
    }
    return fragmentLacksVowel(left) || fragmentLacksVowel(right)
  }

  private static func fragmentLacksVowel(_ fragment: [Character]) -> Bool {
    fragment.count >= 2
      && !fragment.contains { ch in
        ch.lowercased().contains { fragmentVowels.contains($0) }
      }
  }
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
  /// Persist a generated title without overriding a user-custom title.
  /// Returns `false` while the first turn has not created the thread on disk,
  /// when the user already owns the title, or on persistence failure.
  func setGeneratedTitle(backendId: String, title: String) -> Bool
  /// Export a persisted thread to a Markdown file under
  /// `~/.codescribe/transcriptions/YYYY-MM-DD/`. Returns the absolute path of the
  /// written file, or `nil` on failure. `assistantOnly` keeps only assistant turns.
  func exportThreadMarkdown(backendId: String, assistantOnly: Bool) -> String?
  /// Mint a fresh ThreadStore id for a new conversation (so it persists).
  func generateThreadId() -> String
}

// MARK: - Composer dictation seam (voice message → transcript into the draft)

/// Lifecycle of the composer's own voice-note dictation. Independent from the
/// hotkey / overlay dictation session — this drives only the composer mic.
enum ComposerDictationPhase: Equatable {
  case idle
  case preparing  // permission / model load / start-stop transition in flight
  case recording
  case failed(String)
}

enum ComposerCaptureCommand {
  case startAssistive
  case stopAssistive
  case toggleAssistive
}

enum DictationDeliverySource: Equatable {
  case live
  case final
  case edited

  var label: String {
    switch self {
    case .live: return "live chosen"
    case .final: return "final chosen"
    case .edited: return "edited text chosen"
    }
  }
}

/// UI-only seam over the composer dictation controller. The real adapter
/// (`RealComposerDictation`, Core layer) wraps the `CodescribeDictation` bridge;
/// kept bridge-free here so the view-model + #Preview stay standalone (nil = mic
/// is a no-op, e.g. in previews).
@MainActor
protocol ComposerDictating: AnyObject {
  /// Start recording when idle, stop-and-insert when recording.
  func toggle()
  func handle(_ command: ComposerCaptureCommand)
}

// MARK: - Store

@MainActor
final class AgentChatStore: ObservableObject {
  @Published var threads: [ChatThread]
  @Published var selectedThreadID: UUID? {
    // Every selection change re-routes the voice-assistive lane to the thread
    // the user is looking at (operator contract 2026-08-13). Observers do not
    // fire during init — the seeding path publishes once explicitly.
    didSet { publishAssistiveTarget() }
  }
  @Published var draft: String = ""
  /// Monotonic UI command consumed by the composer. It carries no text and
  /// deliberately does not mutate the selected thread or staged attachments.
  @Published private(set) var composerFocusRequest: UInt64 = 0
  @Published private(set) var dictationPreview: String = ""
  @Published private(set) var dictationLivePreview: String = ""
  @Published private(set) var dictationFinalPreview: String?
  @Published private(set) var dictationFinalChangedText = false
  @Published private(set) var dictationVadActive = false
  @Published private(set) var dictationPreviewUserEdited = false
  @Published private(set) var dictationDeliverySource: DictationDeliverySource = .live

  /// Images staged in the composer for the next message. Cleared when the
  /// message is dispatched.
  @Published var pendingAttachments: [PendingAttachment] = []
  @Published private(set) var pendingToolApprovals: [PendingToolApproval] = []

  // MARK: Composer dictation

  /// Current phase of the composer's voice-note dictation. Drives the mic
  /// affordance (ripple while `.recording`) and the inline error feedback.
  @Published private(set) var dictationPhase: ComposerDictationPhase = .idle

  /// True while a hotkey / tray / overlay dictation session owns the microphone.
  /// Set from the authoritative recording lifecycle hooks (see OverlayController)
  /// so the composer mic can't open a second, colliding recorder.
  @Published var dictationBlocked: Bool = false

  /// Injected real adapter (Core). `nil` in previews / mock → mic is inert.
  var dictation: ComposerDictating?

  /// Guards the auto-clear of a `.failed` phase against a stale timer overwriting
  /// a newer state.
  private var dictationFailureToken = UUID()

  /// Toggle the composer voice note (start ↔ stop-and-insert).
  func toggleDictation() { dictation?.toggle() }

  func handleAssistiveCapture(_ command: ComposerCaptureCommand) {
    dictation?.handle(command)
  }

  func requestComposerFocus() {
    composerFocusRequest &+= 1
  }

  /// Set by the real adapter as the dictation session transitions. No-op-safe
  /// when no adapter is wired.
  func setDictationPhase(_ phase: ComposerDictationPhase) { dictationPhase = phase }

  /// Latest live voice-note preview. This is a snapshot buffer from the STT
  /// listener, not a delta stream, and stays separate from `draft` until stop.
  func beginDictationPreviewSession() {
    dictationPreview = ""
    dictationLivePreview = ""
    dictationFinalPreview = nil
    dictationFinalChangedText = false
    dictationVadActive = false
    dictationPreviewUserEdited = false
    dictationDeliverySource = .live
  }

  /// Idempotent on purpose. Apple live polls partials every ~40 ms, so during a
  /// pause the SAME text arrives ~25×/s. Publishing an unchanged value still
  /// fires `objectWillChange`, rebuilding the whole Agent window body — and the
  /// preview's `TextEditor` is NSTextView-backed, so each rebuild mutates the
  /// AppKit subtree, invalidates the window's structural regions and re-runs the
  /// deep `cursorUpdate:` walk (measured 2026-08-05: ~30% of main-thread samples
  /// in `setCursorForMouseLocation:` → `NSCursor _reallySet`, visible as the
  /// pointer flickering between I-beam and arrow, plus a PDF cursor-image reload
  /// per frame). Writing only on real change removes the whole storm at the
  /// source; see `plans/gtm-closure-260804/evidence/2026-08-05_cursor-storm-sample.txt`.
  func updateDictationPreview(_ text: String) {
    let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
    if dictationLivePreview != trimmed { dictationLivePreview = trimmed }
    guard !dictationPreviewUserEdited else { return }
    if dictationPreview != trimmed { dictationPreview = trimmed }
  }

  func editDictationPreview(_ text: String) {
    dictationPreview = text
    dictationPreviewUserEdited = true
    dictationDeliverySource = .edited
  }

  func noteDictationFinalPreview(_ text: String) {
    let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
    let final = trimmed.isEmpty ? nil : trimmed
    let changed = !trimmed.isEmpty && trimmed != dictationLivePreview
    if dictationFinalPreview != final { dictationFinalPreview = final }
    if dictationFinalChangedText != changed { dictationFinalChangedText = changed }
  }

  /// Same idempotence contract as `updateDictationPreview`: the VAD callback
  /// fires per audio chunk, and republishing an unchanged flag rebuilds the
  /// window body for nothing.
  func setDictationVadActive(_ active: Bool) {
    guard dictationVadActive != active else { return }
    dictationVadActive = active
  }

  /// Preserve both hypotheses and explicitly choose the delivery text. A
  /// materially shorter final pass may fill gaps but must never erase a better
  /// live canvas. User edits always win and cancel Assistive auto-send.
  func resolveDictationDelivery(final text: String, autoSend: Bool) -> (
    text: String, autoSend: Bool
  ) {
    let final = text.trimmingCharacters(in: .whitespacesAndNewlines)
    dictationFinalPreview = final.isEmpty ? nil : final
    dictationFinalChangedText = !final.isEmpty && final != dictationLivePreview

    if dictationPreviewUserEdited {
      dictationDeliverySource = .edited
      return (dictationPreview.trimmingCharacters(in: .whitespacesAndNewlines), false)
    }

    let live = dictationLivePreview.trimmingCharacters(in: .whitespacesAndNewlines)
    let liveWords = live.split(whereSeparator: \Character.isWhitespace).count
    let finalWords = final.split(whereSeparator: \Character.isWhitespace).count
    let finalRegressed = !live.isEmpty && (final.isEmpty || finalWords * 100 < liveWords * 85)
    let chosen = finalRegressed ? live : final
    dictationDeliverySource = finalRegressed ? .live : .final
    dictationPreview = chosen
    return (chosen, autoSend)
  }

  func clearDictationPreview() {
    dictationPreview = ""
    dictationLivePreview = ""
    dictationFinalPreview = nil
    dictationFinalChangedText = false
    dictationVadActive = false
    dictationPreviewUserEdited = false
    dictationDeliverySource = .live
  }

  /// Surface a recoverable dictation failure with a self-clearing inline message
  /// (auto-returns to `.idle` after a few seconds so the composer doesn't keep a
  /// stale error banner).
  func reportDictationFailure(_ message: String) {
    clearDictationPreview()
    dictationPhase = .failed(message)
    let token = UUID()
    dictationFailureToken = token
    Task { @MainActor in
      try? await Task.sleep(nanoseconds: 4_000_000_000)
      guard dictationFailureToken == token, case .failed = dictationPhase else { return }
      dictationPhase = .idle
    }
  }

  /// Append the explicitly resolved voice transcript to the editable draft.
  /// Preview provenance remains visible until the next capture starts.
  func appendDictatedTranscript(_ text: String) {
    let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !trimmed.isEmpty else { return }
    if draft.isEmpty {
      draft = trimmed
    } else {
      let needsSeparator = !(draft.last?.isWhitespace ?? false)
      draft += (needsSeparator ? " " : "") + trimmed
    }
  }

  /// Injected by W2-01. `nil` until then; `send` degrades gracefully.
  var engine: AgentChatEngine?

  /// Injected provider for persisted threads. `nil` → falls back to mock seed.
  var threadsProvider: ChatThreadsProviding?

  /// Backs the composer slash-command palette. `nil` (previews, unit tests
  /// without a runtime) ⇒ every command lists nothing rather than lying about
  /// what is configured.
  var paletteSource: ComposerPaletteSourcing?

  /// Entries for one palette command, resolved on demand so a freshly saved
  /// model or a just-granted tool shows up without reopening the window.
  func paletteEntries(for command: ComposerPaletteCommand) -> [ComposerPaletteEntry] {
    let start = Date()
    defer { AgentPerf.log("tool catalog load", since: start, detail: command.rawValue) }
    return paletteSource?.entries(for: command) ?? []
  }

  /// Apply a palette pick. Failures surface as a system line in the thread —
  /// silently ignoring a click would leave the operator believing the model
  /// changed when it did not.
  func applyPaletteEntry(_ entry: ComposerPaletteEntry, for command: ComposerPaletteCommand) {
    guard let paletteSource else { return }
    do {
      try paletteSource.apply(entry, for: command)
    } catch {
      guard let threadID = currentThread?.id else { return }
      append(
        ChatMessage(
          role: .tool,
          timestamp: "now",
          text: "Nie udało się zastosować „\(entry.title)”: "
            + error.localizedDescription
        ),
        to: threadID
      )
    }
  }

  private var revealTask: Task<Void, Never>?
  private var didStartDemo = false

  /// Exactly one composer send may own the composer-side cancellation path.
  @Published private(set) var activeComposerTurn: ActiveComposerTurn?

  /// Active voice-assistive turn being streamed from the core runtime (hotkey /
  /// hands-off), NOT the composer. `nil` when no voice reply is in flight. The
  /// core owns the provider call + disk persistence for this turn; the store
  /// only renders the incoming delivery events — it must never call `send()` for
  /// a voice turn, which would fire a second, composer-side provider call.
  private var voiceTurnThreadID: UUID?
  private var voiceAssistantID: UUID?
  private var voiceTurnStartedAt: Date?
  @Published private(set) var voiceTurnPhase: ComposerTurnPhase?
  weak var voiceTurnCanceller: VoiceTurnCancelling?

  /// In-flight `send()` streaming tasks keyed by thread. Tracked so deleting a
  /// thread can cancel its running reply — otherwise the task's post-stream
  /// `refreshThreads` (plus the agent's best-effort re-persist) would resurrect
  /// the just-deleted thread.
  private struct InFlightSend {
    let id: UUID
    let task: Task<Void, Never>
  }

  private var inFlightSends: [UUID: InFlightSend] = [:]

  /// Bookkeeping for the one title request allowed on a first textual turn,
  /// regardless of source: the composer `send()` and the voice ingest path
  /// (`ingestVoiceTurn` → `ingestVoiceDone`/`Error`/`Cancelled`) share this
  /// coordinator. MainActor serialization makes the turn/title completion race
  /// explicit: whichever result lands first updates this state, and the
  /// turn-side settlement flushes at most one queued write before refreshing
  /// the rail.
  private struct FirstTurnTitleState {
    let backendThreadID: String
    let generationID: UUID
    let originalTitle: String
    var streamCompleted = false
    var generationFinished = false
    var pendingGeneratedTitle: String?
    var pendingCustomTitle: String?
  }

  private var firstTurnTitleStates: [UUID: FirstTurnTitleState] = [:]
  private var titleGenerationTasks: [UUID: Task<Void, Never>] = [:]
  /// Local authority marker used to reject a late generated result even when
  /// the first disk persist and a manual rename interleave.
  private var customTitleThreadIDs: Set<UUID> = []

  /// NotificationCenter tokens for the event-driven rail refresh (wave S,
  /// cut C): window activation + cross-surface `threadsDidChange`. Removed
  /// on deinit; empty when no threads provider is wired (preview/mock).
  private var externalThreadsObservers: [NSObjectProtocol] = []
  private let licenseService: LicenseService?
  private var licenseChangeSink: AnyCancellable?

  /// `loadsThreadIndexEagerly: false` (production `AppModel` path) turns init
  /// into a light shell: no disk I/O on the MainActor bootstrap — the real
  /// thread index loads asynchronously off the main actor and merges in.
  /// The default `true` preserves the synchronous contract tests and previews
  /// rely on (threads visible immediately after init).
  init(
    engine: AgentChatEngine? = nil,
    threadsProvider: ChatThreadsProviding? = nil,
    threads: [ChatThread]? = nil,
    voiceTurnCanceller: VoiceTurnCancelling? = nil,
    licenseService: LicenseService? = nil,
    loadsThreadIndexEagerly: Bool = true
  ) {
    self.engine = engine
    self.threadsProvider = threadsProvider
    self.voiceTurnCanceller = voiceTurnCanceller
    self.licenseService = licenseService

    let seeded: [ChatThread]
    var deferredIndexLoad = false
    if let threads {
      seeded = threads  // explicit (preview/mock)
    } else if threadsProvider != nil, !loadsThreadIndexEagerly {
      seeded = [ChatThread(title: "New thread", meta: "now")]  // shell; index merges async
      deferredIndexLoad = true
    } else if let real = threadsProvider?.listThreads(), !real.isEmpty {
      seeded = real  // real persisted threads
    } else if threadsProvider != nil {
      seeded = [ChatThread(title: "New thread", meta: "now")]  // real provider, empty history
    } else {
      seeded = Self.seedThreads()  // no provider → mock seed
    }
    self.threads = seeded
    self.selectedThreadID = seeded.first?.id
    // didSet does not fire inside init — publish the seed selection once so
    // the assistive lane routes to what the rail shows from the first frame.
    publishAssistiveTarget()
    engine?.installToolApprovalHandler { [weak self] request in
      guard let self else { return }
      self.pendingToolApprovals.removeAll { $0.id == request.id }
      self.pendingToolApprovals.append(request)
    }
    if !deferredIndexLoad, let first = seeded.first { loadMessagesIfNeeded(first.id) }
    beginObservingExternalThreadChanges()
    if deferredIndexLoad {
      scheduleInitialThreadIndexLoad()
    } else if threadsProvider != nil {
      restoreAcceptedTurnsFromDisk()
    }
    licenseChangeSink = licenseService?.objectWillChange.sink { [weak self] _ in
      self?.objectWillChange.send()
    }
  }

  /// Load the persisted thread index OFF the main actor, then merge it in on
  /// the MainActor via the same `replaceThreads` path every other refresh
  /// uses. A turn accepted before the index landed owns the rail (a freshly
  /// minted thread is not on disk until its first stream completes and would
  /// be dropped by a mid-turn replace), so the merge waits for idle.
  private func scheduleInitialThreadIndexLoad() {
    Task { @MainActor [weak self] in
      guard let self, let provider = self.threadsProvider else { return }
      let start = Date()
      let loaded = await Task.detached(priority: .userInitiated) {
        provider.listThreads()
      }.value
      AgentPerf.log("thread index load", since: start, detail: "\(loaded.count) threads")
      var idleWaits = 0
      while self.activeComposerTurn != nil || self.voiceTurnPhase != nil, idleWaits < 240 {
        idleWaits += 1
        try? await Task.sleep(for: .milliseconds(500))
      }
      if !loaded.isEmpty {
        let mergeStart = Date()
        self.replaceThreads(
          with: loaded,
          selectingBackendId: self.currentThread?.backendId,
          keepLocalDrafts: true
        )
        AgentPerf.log("thread index merge + selected thread load", since: mergeStart)
      }
      // Replay messages accepted before the last app death only after the
      // index is in, so they re-bind to their persisted threads.
      self.restoreAcceptedTurnsFromDisk()
    }
  }

  deinit {
    for observer in externalThreadsObservers {
      NotificationCenter.default.removeObserver(observer)
    }
  }

  var currentThread: ChatThread? {
    threads.first { $0.id == selectedThreadID }
  }

  /// Push the rail's current selection down as the voice-assistive routing
  /// target. A selection without a backend id (freshly minted "+ New thread")
  /// publishes `nil`, which the controller reads as "mint a fresh thread on
  /// the next assistive turn".
  private func publishAssistiveTarget() {
    engine?.setAssistiveTargetThread(backendId: currentThread?.backendId)
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

  /// Active phase for the selected thread only. The composer keeps consuming
  /// this established projection, while source-specific cancellation stays
  /// behind the composer engine or voice adapter.
  var selectedComposerTurnPhase: ComposerTurnPhase? {
    if let turn = activeComposerTurn, turn.threadID == selectedThreadID {
      return turn.phase
    }
    if voiceTurnThreadID == selectedThreadID {
      return voiceTurnPhase
    }
    return nil
  }

  var isCancelling: Bool { selectedComposerTurnPhase == .cancelling }

  var currentToolApprovals: [PendingToolApproval] {
    guard let backendID = currentThread?.backendId else { return [] }
    return pendingToolApprovals.filter { $0.threadID == backendID }
  }

  func resolveToolApproval(
    _ request: PendingToolApproval, approved: Bool, remember: Bool = false
  ) {
    _ = engine?.resolveToolApproval(request, approved: approved, remember: remember)
    pendingToolApprovals.removeAll { $0.id == request.id }
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

  // MARK: External refresh (rail live refresh — wave S, cut C)

  /// Wire the event-driven rail refresh. Two triggers, zero polling:
  /// 1. `ThreadsChangeBus.threadsDidChange` — some surface finished a turn
  ///    whose persistence this store did not perform itself.
  /// 2. `NSWindow.didBecomeKeyNotification` — window activation. A thread
  ///    saved by an overlay/assistive turn while the Agent window was
  ///    inactive becomes discoverable on the next activation, no app restart
  ///    (incident 2026-07-21: the reply persisted but the open window kept
  ///    rendering the launch-time list).
  /// Provider-gated: a preview/mock store has no disk truth to re-read.
  private func beginObservingExternalThreadChanges() {
    guard threadsProvider != nil else { return }
    let handler: (Notification) -> Void = { [weak self] _ in
      MainActor.assumeIsolated { self?.scheduleExternalThreadsRefresh() }
    }
    externalThreadsObservers = [
      NotificationCenter.default.addObserver(
        forName: ThreadsChangeBus.threadsDidChange,
        object: nil,
        queue: .main,
        using: handler
      ),
      NotificationCenter.default.addObserver(
        forName: NSWindow.didBecomeKeyNotification,
        object: nil,
        queue: .main,
        using: handler
      ),
    ]
  }

  /// One pending refresh per main-queue tick. Every window in the app posts
  /// `didBecomeKey` (observer has object: nil), and AppKit fires it from
  /// INSIDE window-ordering operations (popover close → orderOut →
  /// becomeKeyWindow) — sample 2026-08-07 10:43 caught the main thread
  /// pinned 93/93 re-listing threads from within _NSPopoverCloseAndAnimate.
  /// Coalescing onto the next tick collapses the storm AND moves the disk
  /// re-read out of the notification callout.
  private var externalRefreshScheduled = false

  private func scheduleExternalThreadsRefresh() {
    guard !externalRefreshScheduled else { return }
    externalRefreshScheduled = true
    DispatchQueue.main.async { [weak self] in
      MainActor.assumeIsolated {
        guard let self else { return }
        self.externalRefreshScheduled = false
        self.refreshThreadsFromExternalChange()
      }
    }
  }

  /// Re-read persisted threads after an external change signal. Deliberately
  /// a no-op while a composer or voice turn is in flight: the turn's own
  /// terminal already refreshes with the right selection, and a mid-stream
  /// replace could drop a freshly minted thread that does not exist on disk
  /// until its first stream completes.
  func refreshThreadsFromExternalChange() {
    guard threadsProvider != nil else { return }
    guard activeComposerTurn == nil, voiceTurnPhase == nil else { return }
    refreshThreads()
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
      guard threadsProvider?.setThreadFavorite(backendId: backendId, isFavorite: next) == true
      else { return }
    }
    threads[ti].isFavorite = next
  }

  /// Rename a thread from the rail. Persists through the threads provider when
  /// the thread is backed on disk; a not-yet-persisted local thread is renamed
  /// in memory only. No-ops on an empty or unchanged title. The chat header
  /// reads `currentThread.title`, so it updates reactively too.
  func rename(_ thread: ChatThread, to newTitle: String) {
    guard let trimmed = ThreadTitlePolicy.normalized(newTitle), trimmed != thread.title,
      let ti = threads.firstIndex(where: { $0.id == thread.id })
    else { return }
    if let backendId = thread.backendId {
      if threadsProvider?.renameThread(backendId: backendId, title: trimmed) != true {
        guard queueCustomTitle(trimmed, for: thread.id, backendThreadID: backendId) else { return }
      }
    }
    customTitleThreadIDs.insert(thread.id)
    if var state = firstTurnTitleStates[thread.id] {
      state.pendingGeneratedTitle = nil
      firstTurnTitleStates[thread.id] = state
    }
    threads[ti].title = trimmed
  }

  /// Flip one bubble between raw mono and rich markdown (meta-row toggle).
  /// Per-message, in-memory only; deliberately does NOT touch the fields the
  /// scroll signature reads, so a toggle never auto-scrolls the list.
  func toggleRenderMode(messageID: UUID, in threadID: UUID) {
    update(messageID, in: threadID) {
      $0.renderMode = MessageRenderMode.nextRenderMode(after: $0.renderMode)
    }
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
      let deleted = threadsProvider?.deleteThread(backendId: backendId) == true
      // A freshly minted backend id does not exist on disk until the
      // first stream returns. In that one known race, local delete still
      // wins and the existing engine cancellation prevents persistence.
      guard deleted || firstTurnTitleStates[thread.id] != nil else { return }
      // The attachment sidecar is written before the first stream starts,
      // so the missing-file race still has local metadata to remove.
      removePersistedAttachmentMetadata(for: backendId)
    }
    titleGenerationTasks[thread.id]?.cancel()
    titleGenerationTasks[thread.id] = nil
    firstTurnTitleStates[thread.id] = nil
    customTitleThreadIDs.remove(thread.id)
    // Deleting a thread deletes its queue — in memory and on disk.
    queuedTurns.removeAll { $0.threadID == thread.id }
    if let backendId = thread.backendId {
      removeDurableAcceptedTurns(backendThreadID: backendId)
    }
    // Cancel any in-flight reply for this thread so its post-stream refresh
    // can't re-list (and the caret/finalize can't mutate) a deleted thread.
    // Swift-task cancel first (so the awaiting send sees isCancelled and
    // stays silent), then the engine-side cancel, which actually aborts the
    // Rust turn — stopping tool side effects, not just the UI updates.
    inFlightSends[thread.id]?.task.cancel()
    inFlightSends[thread.id] = nil
    if let backendId = thread.backendId {
      _ = engine?.cancelReply(threadId: backendId)
      pendingToolApprovals.removeAll { $0.threadID == backendId }
    }
    if activeComposerTurn?.threadID == thread.id {
      activeComposerTurn = nil
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
      !threads[ti].messagesLoaded
    else { return }
    let start = Date()
    defer { AgentPerf.log("selected thread load", since: start, detail: backendId) }
    // Persisted user turns carry the wire skeleton (disk keeps the LLM
    // truth); rewrite them for display so restored threads render the
    // spoken instruction, exactly like a live turn.
    threads[ti].messages = applyingPersistedAttachmentMetadata(
      to: provider.loadMessages(backendId: backendId),
      backendId: backendId
    ).map(AssistivePromptParser.presented)
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
  /// both. Drives the send button's enabled state. A turn already in flight no
  /// longer blocks acceptance — `send()` queues instead of dropping.
  var canSend: Bool {
    !isAgenticLocked
      && (!draft.trimmingCharacters(in: .whitespaces).isEmpty || !pendingAttachments.isEmpty)
  }

  var isAgenticLocked: Bool { licenseService?.canUseAgentic == false }
  var agenticBlockMessage: String? {
    isAgenticLocked ? licenseService?.agenticBlockMessage : nil
  }

  // MARK: Turn queue (messages accepted while a turn is in flight)

  /// A message the UI accepted. Durable from the moment of acceptance: it is
  /// written to the sidecar in `accept` and removed only when its turn reaches
  /// a terminal (success, provider error, or stop), so an app death in between
  /// replays it on the next launch instead of losing it.
  struct QueuedTurn: Identifiable, Equatable {
    let id: UUID
    let threadID: UUID
    let backendThreadID: String
    var text: String
    let attachments: [PendingAttachment]
    let enqueuedAt: Date
  }

  /// FIFO of accepted-but-not-yet-running messages, ordered by acceptance
  /// across all threads; per-thread order is what the contract guarantees.
  @Published private(set) var queuedTurns: [QueuedTurn] = []

  func queuedTurns(in threadID: UUID) -> [QueuedTurn] {
    queuedTurns.filter { $0.threadID == threadID }
  }

  /// Terminal-style composer history, oldest → newest. It includes already
  /// dispatched user turns and accepted queued turns, so Up can recover the
  /// exact message the operator just queued without cancelling it first.
  func composerHistory(in threadID: UUID) -> [String] {
    let sent =
      threads.first(where: { $0.id == threadID })?.messages
      .filter { $0.role == .you }
      .map(\.text) ?? []
    let queued = queuedTurns(in: threadID).map(\.text)
    return (sent + queued).reduce(into: [String]()) { result, text in
      let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
      guard !text.isEmpty, result.last != text else { return }
      result.append(text)
    }
  }

  /// Cancel one still-queued (never dispatched) message.
  func cancelQueuedTurn(_ id: UUID) {
    guard queuedTurns.contains(where: { $0.id == id }) else { return }
    queuedTurns.removeAll { $0.id == id }
    removeDurableAcceptedTurn(id: id)
  }

  /// Edit an accepted turn while it is still queued. The same durable sidecar
  /// is replaced immediately, so a crash/relaunch cannot resurrect the old
  /// wording. Attachments and FIFO position stay unchanged.
  @discardableResult
  func editQueuedTurn(_ id: UUID, text: String) -> Bool {
    guard let index = queuedTurns.firstIndex(where: { $0.id == id }) else { return false }
    let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !text.isEmpty || !queuedTurns[index].attachments.isEmpty else { return false }
    queuedTurns[index].text = text
    persistDurableAcceptedTurn(queuedTurns[index])
    return true
  }

  // MARK: Send (accept → queue → serialized dispatch)

  func send() {
    guard !isAgenticLocked else { return }
    let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
    let staged = pendingAttachments
    attachLog.info(
      "send: building request attachmentPaths.count=\(staged.count, privacy: .public) text.isEmpty=\(text.isEmpty, privacy: .public)"
    )
    guard !text.isEmpty || !staged.isEmpty, let threadID = selectedThreadID else { return }
    draft = ""
    pendingAttachments = []
    accept(text: text, staged: staged, threadID: threadID)
  }

  /// Accept a message: persist it durably, enqueue it FIFO on its thread, and
  /// let the single dispatch owner start it if the composer slot is idle.
  private func accept(text: String, staged: [PendingAttachment], threadID: UUID) {
    let backendId = ensureBackendId(threadID)
    let turn = QueuedTurn(
      id: UUID(),
      threadID: threadID,
      backendThreadID: backendId,
      text: text,
      attachments: staged,
      enqueuedAt: Date()
    )
    persistDurableAcceptedTurn(turn)
    queuedTurns.append(turn)
    queueLog.info(
      "accepted turn \(turn.id, privacy: .public) thread=\(backendId, privacy: .public) queued=\(self.queuedTurns.count, privacy: .public)"
    )
    dispatchNextQueuedTurnIfIdle()
  }

  /// The ONLY `queued → running` transition. MainActor-serialized, guarded by
  /// the single composer slot, and `startTurn` claims that slot synchronously —
  /// so exactly one owner can ever promote a queued message. Threads with an
  /// active voice turn are skipped (the core owns that thread's stream); their
  /// queued messages wait for the voice terminal.
  private func dispatchNextQueuedTurnIfIdle() {
    guard activeComposerTurn == nil else { return }
    guard
      let index = queuedTurns.firstIndex(where: { turn in
        !(voiceTurnThreadID == turn.threadID && voiceTurnPhase != nil)
      })
    else { return }
    var turn = queuedTurns.remove(at: index)
    // The local UUID can go stale while queued (a rail refresh re-mints
    // rows); the DURABLE binding is the backend thread id. Re-bind instead
    // of dropping — an explicitly deleted thread already purged its queue
    // in `delete`, so anything still here must run.
    if !threads.contains(where: { $0.id == turn.threadID }) {
      let resolvedID: UUID
      if let match = threads.first(where: { $0.backendId == turn.backendThreadID }) {
        resolvedID = match.id
      } else {
        var thread = ChatThread(title: "Restored draft", meta: "now")
        thread.backendId = turn.backendThreadID
        thread.messagesLoaded = true
        threads.insert(thread, at: 0)
        resolvedID = thread.id
      }
      turn = QueuedTurn(
        id: turn.id,
        threadID: resolvedID,
        backendThreadID: turn.backendThreadID,
        text: turn.text,
        attachments: turn.attachments,
        enqueuedAt: turn.enqueuedAt
      )
    }
    queueLog.info(
      "promoting turn \(turn.id, privacy: .public) thread=\(turn.backendThreadID, privacy: .public) queued=\(self.queuedTurns.count, privacy: .public)"
    )
    startTurn(turn)
  }

  /// Run one accepted message as the active composer turn. The acceptance id
  /// IS the turn id, so one element is traceable end-to-end (accept → queue →
  /// running → terminal → durable-record removal).
  private func startTurn(_ queued: QueuedTurn) {
    let threadID = queued.threadID
    let backendId = queued.backendThreadID
    let text = queued.text
    let staged = queued.attachments
    let attachmentPaths = staged.map { $0.url.path }
    let userTurnIndex = currentUserTurnCount(in: threadID)

    // Carry the staged attachments onto the You bubble so the sender sees a
    // chip (name + optional thumbnail) for what they attached.
    let sent = staged.map { MessageAttachment(name: $0.name, url: $0.url, type: $0.type) }
    persistAttachmentMetadata(sent, for: backendId, userTurnIndex: userTurnIndex)
    append(ChatMessage(role: .you, timestamp: now(), text: text, attachments: sent), to: threadID)
    let assistant = ChatMessage(role: .assistant, timestamp: "now", text: "", isThinking: true)
    let assistantID = assistant.id
    append(assistant, to: threadID)
    let turnID = queued.id
    activeComposerTurn = ActiveComposerTurn(
      id: turnID,
      threadID: threadID,
      backendThreadID: backendId,
      assistantMessageID: assistantID,
      phase: .thinking
    )
    if userTurnIndex == 0, !text.isEmpty, engine != nil {
      prepareFirstTurnTitle(for: threadID, backendThreadID: backendId)
    }
    let sendTask = Task { @MainActor in
      var titleStreamSettled = false
      defer {
        if !titleStreamSettled {
          settleFirstTurnTitleAfterStream(for: threadID, backendThreadID: backendId)
        }
        // The turn reached a terminal (success, provider error, or a
        // cancelled Swift task) — its outcome is visible in the UI, so
        // the durable acceptance record is consumed. Only an app death
        // BEFORE this point leaves the record behind for restart replay.
        removeDurableAcceptedTurn(id: turnID)
        releaseComposerTurn(turnID, in: threadID)
      }
      guard let engine else {
        finish(
          assistantID, in: threadID,
          text: "Engine not wired yet.")
        return
      }
      // Graceful unavailable path — the engine reports WHAT is missing
      // (lane, endpoint or key) so the reply is actionable, not generic.
      if let unavailableDetail = engine.availabilityDetail() {
        finishTitleGenerationWithoutRequest(for: threadID)
        finish(assistantID, in: threadID, text: unavailableDetail)
        return
      }
      if userTurnIndex == 0, !text.isEmpty {
        launchFirstTurnTitle(
          text,
          for: threadID,
          backendThreadID: backendId,
          engine: engine
        )
      }
      let start = Date()
      do {
        // REAL streaming: tokens land live as the agent emits them.
        let finalText = try await engine.streamReply(
          text,
          threadId: backendId,
          attachmentPaths: attachmentPaths,
          onDelta: { [weak self] delta in
            guard self?.acceptsComposerEvent(turnID, assistantID: assistantID, in: threadID) == true
            else {
              return
            }
            self?.setComposerPhase(.streaming, for: turnID)
            self?.update(assistantID, in: threadID) {
              $0.isThinking = false
              $0.isStreaming = true
              if $0.reasonedSeconds == nil {
                $0.reasonedSeconds = Date().timeIntervalSince(start)
              }
              $0.text += delta
            }
          },
          onReasoning: { [weak self] delta in
            guard self?.acceptsComposerEvent(turnID, assistantID: assistantID, in: threadID) == true
            else {
              return
            }
            self?.appendReasoning(delta, to: assistantID, in: threadID)
          },
          onToolExecuting: { [weak self] name, id in
            guard self?.acceptsComposerEvent(turnID, assistantID: assistantID, in: threadID) == true
            else {
              return
            }
            self?.recordToolStarted(name: name, callID: id, before: assistantID, in: threadID)
          },
          onToolResult: { [weak self] name, id, isError, reason in
            self?.pendingToolApprovals.removeAll {
              $0.threadID == backendId && $0.callID == id
            }
            guard self?.acceptsComposerEvent(turnID, assistantID: assistantID, in: threadID) == true
            else {
              return
            }
            self?.recordToolResult(
              name: name, callID: id, isError: isError, reason: reason,
              before: assistantID, in: threadID)
          }
        )
        settleFirstTurnTitleAfterStream(for: threadID, backendThreadID: backendId)
        titleStreamSettled = true
        // The thread may have been deleted mid-stream; drop the late
        // finalize + refresh so a cancelled send can't bring it back.
        if Task.isCancelled { return }
        finishPendingTools(before: assistantID, in: threadID)
        update(assistantID, in: threadID) {
          $0.isThinking = false
          $0.isStreaming = false
          // A provider that emits only a final TextDone (no token deltas)
          // leaves the bubble empty; fall back to the assembled return so
          // the reply is never a blank bubble.
          if $0.text.isEmpty { $0.text = finalText }
          $0.timestamp = self.now()
        }
        refreshThreads(selectingBackendId: backendId)
      } catch {
        if Task.isCancelled { return }
        finish(
          assistantID, in: threadID,
          text: "Something went wrong: \(error.localizedDescription)")
      }
    }
    inFlightSends[threadID] = InFlightSend(id: turnID, task: sendTask)
  }

  /// Launch the title lane as an independent, unstructured MainActor task.
  /// Awaiting the engine releases the actor, so this request runs concurrently
  /// with the conversational turn (composer `streamReply` or the core-owned
  /// voice stream) without escaping non-Sendable engine/provider seams. It is
  /// a stateless sibling request — it never re-enters `send()`/`streamReply`.
  private func launchFirstTurnTitle(
    _ text: String,
    for threadID: UUID,
    backendThreadID: String,
    engine: AgentChatEngine
  ) {
    guard let state = firstTurnTitleStates[threadID],
      state.backendThreadID == backendThreadID
    else { return }
    let generationID = state.generationID
    let task = Task { @MainActor [weak self] in
      do {
        let title = try await engine.generateThreadTitle(text)
        guard !Task.isCancelled else { return }
        self?.receiveGeneratedTitle(title, for: threadID, generationID: generationID)
      } catch {
        guard !Task.isCancelled else { return }
        self?.finishTitleGeneration(for: threadID, generationID: generationID)
      }
    }
    titleGenerationTasks[threadID] = task
  }

  /// Establish the race authority synchronously inside `send()`. A rail action
  /// performed immediately after `send()` returns can therefore queue a custom
  /// write or discard title work even before the unstructured task is scheduled.
  private func prepareFirstTurnTitle(for threadID: UUID, backendThreadID: String) {
    guard threadsProvider != nil,
      !customTitleThreadIDs.contains(threadID),
      firstTurnTitleStates[threadID] == nil,
      let originalTitle = threads.first(where: { $0.id == threadID })?.title
    else { return }
    firstTurnTitleStates[threadID] = FirstTurnTitleState(
      backendThreadID: backendThreadID,
      generationID: UUID(),
      originalTitle: originalTitle
    )
  }

  /// Voice entry to the SAME first-turn coordinator `send()` uses. The core
  /// runtime owns the conversational provider call and its persistence; this
  /// launches only the stateless title sibling — never `send()`/`streamReply`
  /// — so the exchange is not dispatched twice. `ingestVoiceDone` is the
  /// stream-completed settle point (core persistence has finished by then).
  private func launchVoiceFirstTurnTitle(
    _ presentedText: String,
    for threadID: UUID,
    backendThreadID: String
  ) {
    guard !presentedText.isEmpty, let engine else { return }
    prepareFirstTurnTitle(for: threadID, backendThreadID: backendThreadID)
    guard firstTurnTitleStates[threadID] != nil else { return }
    guard engine.availabilityDetail() == nil else {
      finishTitleGenerationWithoutRequest(for: threadID)
      return
    }
    launchFirstTurnTitle(
      presentedText, for: threadID, backendThreadID: backendThreadID, engine: engine)
  }

  private func finishTitleGenerationWithoutRequest(for threadID: UUID) {
    guard var state = firstTurnTitleStates[threadID] else { return }
    state.generationFinished = true
    firstTurnTitleStates[threadID] = state
    cleanUpFirstTurnTitleStateIfFinished(for: threadID)
  }

  private func receiveGeneratedTitle(_ title: String?, for threadID: UUID, generationID: UUID) {
    guard var state = firstTurnTitleStates[threadID], state.generationID == generationID else {
      return
    }
    state.generationFinished = true
    guard let trimmed = ThreadTitlePolicy.normalized(title),
      !customTitleThreadIDs.contains(threadID),
      state.pendingCustomTitle == nil,
      let ti = threads.firstIndex(where: { $0.id == threadID })
    else {
      firstTurnTitleStates[threadID] = state
      cleanUpFirstTurnTitleStateIfFinished(for: threadID)
      return
    }

    threads[ti].title = trimmed
    let persisted =
      threadsProvider?.setGeneratedTitle(
        backendId: state.backendThreadID,
        title: trimmed
      ) == true
    if !persisted {
      if state.streamCompleted {
        if threads[ti].title == trimmed { threads[ti].title = state.originalTitle }
      } else {
        state.pendingGeneratedTitle = trimmed
      }
    }
    firstTurnTitleStates[threadID] = state
    cleanUpFirstTurnTitleStateIfFinished(for: threadID)
  }

  private func finishTitleGeneration(for threadID: UUID, generationID: UUID) {
    guard var state = firstTurnTitleStates[threadID], state.generationID == generationID else {
      return
    }
    state.generationFinished = true
    firstTurnTitleStates[threadID] = state
    cleanUpFirstTurnTitleStateIfFinished(for: threadID)
  }

  /// Mark the Rust stream (and therefore its best-effort first persistence)
  /// complete, flush a queued custom rename first, otherwise retry one queued
  /// generated title exactly once. This runs before `refreshThreads`.
  private func settleFirstTurnTitleAfterStream(for threadID: UUID, backendThreadID: String) {
    guard var state = firstTurnTitleStates[threadID], state.backendThreadID == backendThreadID
    else { return }
    guard !state.streamCompleted else { return }
    state.streamCompleted = true

    if let customTitle = state.pendingCustomTitle {
      _ = threadsProvider?.renameThread(backendId: backendThreadID, title: customTitle)
      state.pendingCustomTitle = nil
      state.pendingGeneratedTitle = nil
    } else if let generatedTitle = state.pendingGeneratedTitle {
      let persisted =
        threadsProvider?.setGeneratedTitle(
          backendId: backendThreadID,
          title: generatedTitle
        ) == true
      state.pendingGeneratedTitle = nil
      if !persisted,
        !customTitleThreadIDs.contains(threadID),
        let ti = threads.firstIndex(where: { $0.id == threadID }),
        threads[ti].title == generatedTitle
      {
        threads[ti].title = state.originalTitle
      }
    }

    firstTurnTitleStates[threadID] = state
    cleanUpFirstTurnTitleStateIfFinished(for: threadID)
  }

  /// Queue a rename only for the active first-turn missing-file window.
  /// One dictionary slot means repeated UI commits collapse to the latest
  /// custom title, while generated persistence is discarded immediately.
  private func queueCustomTitle(_ title: String, for threadID: UUID, backendThreadID: String)
    -> Bool
  {
    guard var state = firstTurnTitleStates[threadID],
      state.backendThreadID == backendThreadID,
      !state.streamCompleted
    else { return false }
    state.pendingCustomTitle = title
    state.pendingGeneratedTitle = nil
    firstTurnTitleStates[threadID] = state
    return true
  }

  private func cleanUpFirstTurnTitleStateIfFinished(for threadID: UUID) {
    guard let state = firstTurnTitleStates[threadID],
      state.streamCompleted,
      state.generationFinished,
      state.pendingGeneratedTitle == nil,
      state.pendingCustomTitle == nil
    else { return }
    firstTurnTitleStates[threadID] = nil
    titleGenerationTasks[threadID] = nil
  }

  /// Stop the selected Agent turn through its owning adapter. Voice is checked
  /// first because it has no Swift waiter and must never touch the composer
  /// registry. Composer ordering remains deliberate: waiter first, Rust second.
  func stopActiveTurn() {
    if let threadID = voiceTurnThreadID,
      threadID == selectedThreadID,
      let phase = voiceTurnPhase,
      phase != .cancelling,
      let backendId = threads.first(where: { $0.id == threadID })?.backendId
    {
      voiceTurnPhase = .cancelling
      if voiceTurnCanceller?.cancelVoiceTurn(threadId: backendId) != true {
        // The runtime may have crossed its successful terminal just before
        // the click. Keep accepting that terminal instead of stranding the
        // local bubble in a false Cancelling state.
        voiceTurnPhase = phase
      }
      return
    }

    guard var turn = activeComposerTurn,
      turn.threadID == selectedThreadID,
      turn.phase != .cancelling
    else { return }

    turn.phase = .cancelling
    activeComposerTurn = turn
    inFlightSends[turn.threadID]?.task.cancel()
    let firstAcknowledgement = engine?.cancelReply(threadId: turn.backendThreadID) ?? false
    pendingToolApprovals.removeAll { $0.threadID == turn.backendThreadID }

    // A very fast Stop can beat Rust's registry setup while the provider and
    // persisted history are still loading. Retry only that unacknowledged
    // race; the UI click remains idempotent and every probe uses the same
    // exact backend id. Settle after acknowledgement (or a bounded idle race).
    Task { @MainActor [weak self] in
      var acknowledged = firstAcknowledgement
      var attempts = 0
      while !acknowledged, attempts < 80 {
        guard let self,
          self.activeComposerTurn?.id == turn.id,
          self.activeComposerTurn?.phase == .cancelling,
          let engine = self.engine
        else { break }
        attempts += 1
        try? await Task.sleep(for: .milliseconds(25))
        acknowledged = engine.cancelReply(threadId: turn.backendThreadID)
      }
      await Task.yield()
      self?.settleStoppedComposerTurn(turn)
    }
  }

  // MARK: Voice-assistive delivery (core runtime → live render, no re-send)
  //
  // These ingest the reply the CORE runtime is already streaming for a hotkey /
  // voice turn (via the bridge `CsAgentDeliveryListener`). They ONLY render:
  // insert bubbles and mutate them from deltas. They deliberately do not call
  // `send()` / `streamReply` — the core already made the provider call and
  // persists the thread to disk. Doing otherwise would double-dispatch the
  // turn. The single engine call allowed on this path is the stateless
  // first-turn title sibling (`generateThreadTitle`), which carries no
  // conversation state and never touches the thread's response chain.

  /// Open a voice turn: bind (or create) a thread for the core `backendId`
  /// and insert the You-bubble + an assistant placeholder. Delivery events
  /// never change the rail selection: a matching active thread renders live,
  /// while a turn for another thread updates in the background. Only explicit
  /// user actions (`select`, `newThread`, delete fallback) move selection.
  func ingestVoiceTurn(threadId backendId: String, userText: String) {
    // Defensive: a new voice turn can open before the previous one closed
    // (rapid double-press / a fresh session). Finalize the stale assistant
    // bubble in the UI before we overwrite the turn references below —
    // otherwise it sticks in isThinking/isStreaming forever.
    if let staleThreadID = voiceTurnThreadID, let staleID = voiceAssistantID {
      finishPendingTools(before: staleID, in: staleThreadID)
      update(staleID, in: staleThreadID) {
        $0.isThinking = false
        $0.isStreaming = false
        $0.timestamp = self.now()
      }
      // The stale turn ended without its own terminal event, so settle its
      // title coordinator here — a queued generated title must not outlive
      // the turn that owned it.
      if let staleBackendID = threads.first(where: { $0.id == staleThreadID })?.backendId {
        settleFirstTurnTitleAfterStream(for: staleThreadID, backendThreadID: staleBackendID)
      }
    }

    // The core sends the WIRE prompt (assistive skeleton); the bubble shows
    // the spoken instruction. The wire + selection/app context ride along on
    // the message for the context chip and "Copy full prompt".
    let userTurn = AssistivePromptParser.presented(
      ChatMessage(role: .you, timestamp: now(), text: userText)
    )

    let threadID: UUID
    var isFirstExchange = false
    if let existing = threads.first(where: { $0.backendId == backendId }) {
      threadID = existing.id
      loadMessagesIfNeeded(threadID)  // surface prior history before appending
    } else if let draftIndex = threads.firstIndex(where: {
      $0.id == selectedThreadID && $0.backendId == nil && $0.messages.isEmpty
    }) {
      // The user is sitting in an empty local-only draft ("+ New thread")
      // watching their dictation stream: the voice turn must land THERE.
      // Binding the draft to the core's thread id keeps the user where they
      // are; minting a parallel thread here yanked the conversation into a
      // surprise rail entry mid-sentence (UI_DIVERGENCE_AUDIT / operator
      // report 2026-08-08).
      threads[draftIndex].backendId = backendId
      threads[draftIndex].messagesLoaded = true  // freshly bound → in sync
      threads[draftIndex].title =
        ThreadTitlePolicy.normalized(userTurn.text, limit: 48) ?? "Voice chat"
      threads[draftIndex].meta = "now"
      threadID = threads[draftIndex].id
      isFirstExchange = true
    } else {
      let title = ThreadTitlePolicy.normalized(userTurn.text, limit: 48) ?? "Voice chat"
      var thread = ChatThread(title: title, meta: "now")
      thread.backendId = backendId
      thread.messagesLoaded = true  // freshly bound to a core id → in sync
      threads.insert(thread, at: 0)
      threadID = thread.id
      isFirstExchange = true
    }
    // A skeleton turn can carry context with an empty instruction (e.g. a
    // clipped dictation) — the bubble still renders for the chip.
    if !userTurn.text.isEmpty || userTurn.wireText != nil {
      append(userTurn, to: threadID)
    }
    let assistant = ChatMessage(role: .assistant, timestamp: "now", text: "", isThinking: true)
    voiceTurnThreadID = threadID
    voiceAssistantID = assistant.id
    voiceTurnStartedAt = Date()
    voiceTurnPhase = .thinking
    append(assistant, to: threadID)
    if isFirstExchange {
      launchVoiceFirstTurnTitle(userTurn.text, for: threadID, backendThreadID: backendId)
    }
  }

  /// Append a streamed token to the active voice assistant bubble.
  func ingestVoiceDelta(_ delta: String) {
    guard voiceTurnPhase != .cancelling,
      let threadID = voiceTurnThreadID, let id = voiceAssistantID
    else { return }
    voiceTurnPhase = .streaming
    update(id, in: threadID) {
      $0.isThinking = false
      $0.isStreaming = true
      if $0.reasonedSeconds == nil, let started = self.voiceTurnStartedAt {
        $0.reasonedSeconds = Date().timeIntervalSince(started)
      }
      $0.text += delta
    }
  }

  /// Append streamed model reasoning to the active voice assistant bubble.
  func ingestVoiceReasoning(_ delta: String) {
    guard voiceTurnPhase != .cancelling,
      let threadID = voiceTurnThreadID, let id = voiceAssistantID
    else { return }
    voiceTurnPhase = .streaming
    appendReasoning(delta, to: id, in: threadID)
  }

  /// Final assembled text for the turn. Only used as a fallback when the reply
  /// arrived without token deltas (otherwise the bubble already holds the text).
  func ingestVoiceTextDone(_ text: String) {
    guard voiceTurnPhase != .cancelling,
      let threadID = voiceTurnThreadID, let id = voiceAssistantID
    else { return }
    voiceTurnPhase = .streaming
    update(id, in: threadID) { if $0.text.isEmpty { $0.text = text } }
  }

  /// Surface a pending tool call for the active voice turn. The bridge's `id`
  /// is kept end-to-end so the matching result can update this row in place.
  func ingestVoiceToolExecuting(name: String, id callID: String) {
    guard voiceTurnPhase != .cancelling,
      let threadID = voiceTurnThreadID, let assistantID = voiceAssistantID
    else { return }
    voiceTurnPhase = .streaming
    recordToolStarted(name: name, callID: callID, before: assistantID, in: threadID)
  }

  /// Surface a completed tool call for the active voice turn (same rendering as
  /// the composer path's tool-activity row).
  func ingestVoiceToolResult(name: String, id callID: String, isError: Bool, reason: String) {
    guard voiceTurnPhase != .cancelling,
      let threadID = voiceTurnThreadID, let assistantID = voiceAssistantID
    else { return }
    voiceTurnPhase = .streaming
    recordToolResult(
      name: name, callID: callID, isError: isError, reason: reason, before: assistantID,
      in: threadID)
  }

  /// Finalize the active voice turn and pull disk truth (the core persisted the
  /// thread). No re-persist here — the store only mirrors what the core wrote.
  /// This is also the title coordinator's stream-completed settle point: core
  /// persistence finished before this terminal, so a queued generated title
  /// flushes exactly once here, before the rail refresh.
  func ingestVoiceDone() {
    guard voiceTurnPhase != .cancelling,
      let threadID = voiceTurnThreadID, let id = voiceAssistantID
    else { return }
    finishPendingTools(before: id, in: threadID)
    update(id, in: threadID) {
      $0.isThinking = false
      $0.isStreaming = false
      $0.timestamp = self.now()
    }
    let backendId = threads.first(where: { $0.id == threadID })?.backendId
    if let backendId {
      settleFirstTurnTitleAfterStream(for: threadID, backendThreadID: backendId)
    }
    clearVoiceTurnState()
    if let backendId { refreshThreads(selectingBackendId: backendId) }
  }

  /// Surface a runtime error on the active voice turn and close it. The core
  /// error path may not emit a separate `Done`, so clear the turn state here; a
  /// late `Done` then no-ops against the cleared state.
  func ingestVoiceError(_ message: String) {
    guard voiceTurnPhase != .cancelling,
      let threadID = voiceTurnThreadID, let id = voiceAssistantID
    else { return }
    finishPendingTools(before: id, in: threadID)
    update(id, in: threadID) {
      $0.isThinking = false
      $0.isStreaming = false
      $0.text += ($0.text.isEmpty ? "" : "\n") + "[error] " + message
      $0.timestamp = self.now()
    }
    // A failed turn persisted nothing; settling lets the coordinator try a
    // queued title once, fail against the missing thread, and restore the
    // fallback title instead of leaving the queue open forever.
    if let backendId = threads.first(where: { $0.id == threadID })?.backendId {
      settleFirstTurnTitleAfterStream(for: threadID, backendThreadID: backendId)
    }
    clearVoiceTurnState()
  }

  /// Settle the single keyed cancellation terminal. Partial text remains, an
  /// empty response becomes a quiet Stopped marker, and running tools become
  /// stopped without refreshing disk truth (the core intentionally did not
  /// persist this turn as successful).
  func ingestVoiceCancelled(threadId backendId: String) {
    guard voiceTurnPhase == .cancelling,
      let threadID = voiceTurnThreadID,
      let id = voiceAssistantID,
      threads.first(where: { $0.id == threadID })?.backendId == backendId
    else { return }
    cancelPendingTools(before: id, in: threadID)
    update(id, in: threadID) {
      $0.isThinking = false
      $0.isStreaming = false
      $0.wasStopped = true
      if $0.text.isEmpty { $0.text = "Stopped" }
      $0.timestamp = self.now()
    }
    // Mirror the composer's cancel path (its defer settles too): the core
    // did not persist this turn, so a late generated title fails to persist
    // and the fallback title survives.
    settleFirstTurnTitleAfterStream(for: threadID, backendThreadID: backendId)
    clearVoiceTurnState()
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
        fullText:
          "On it — patching events/bus.ts to emit once per settled retry, de-duping the store subscription on remount, and adding a regression test for the double-fire case.",
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
    finishPendingTools(before: id, in: threadID)
    update(id, in: threadID) {
      $0.isThinking = false
      $0.isStreaming = false
      $0.text = text
      $0.timestamp = self.now()
    }
  }

  private func acceptsComposerEvent(_ turnID: UUID, assistantID: UUID, in threadID: UUID) -> Bool {
    guard let turn = activeComposerTurn else { return false }
    return turn.id == turnID
      && turn.threadID == threadID
      && turn.assistantMessageID == assistantID
      && turn.phase != .cancelling
  }

  private func setComposerPhase(_ phase: ComposerTurnPhase, for turnID: UUID) {
    guard var turn = activeComposerTurn, turn.id == turnID, turn.phase != .cancelling else {
      return
    }
    turn.phase = phase
    activeComposerTurn = turn
  }

  private func releaseComposerTurn(_ turnID: UUID, in threadID: UUID) {
    if inFlightSends[threadID]?.id == turnID {
      inFlightSends[threadID] = nil
    }
    if activeComposerTurn?.id == turnID,
      activeComposerTurn?.phase != .cancelling
    {
      activeComposerTurn = nil
    }
    // Terminal of any kind frees the composer slot; the queue continues.
    // A provider error or cancel must never silently strand queued items.
    dispatchNextQueuedTurnIfIdle()
  }

  private func settleStoppedComposerTurn(_ turn: ActiveComposerTurn) {
    guard activeComposerTurn?.id == turn.id,
      activeComposerTurn?.phase == .cancelling
    else { return }
    cancelPendingTools(before: turn.assistantMessageID, in: turn.threadID)
    update(turn.assistantMessageID, in: turn.threadID) {
      $0.isThinking = false
      $0.isStreaming = false
      $0.wasStopped = true
      if $0.text.isEmpty { $0.text = "Stopped" }
      $0.timestamp = self.now()
    }
    if inFlightSends[turn.threadID]?.id == turn.id {
      inFlightSends[turn.threadID] = nil
    }
    activeComposerTurn = nil
    // A user Stop consumed its own turn but not the queue: FIFO continues.
    dispatchNextQueuedTurnIfIdle()
  }

  private func clearVoiceTurnState() {
    voiceTurnThreadID = nil
    voiceAssistantID = nil
    voiceTurnStartedAt = nil
    voiceTurnPhase = nil
    // The voice terminal frees its thread for queued composer messages.
    dispatchNextQueuedTurnIfIdle()
  }

  // MARK: Mutation helpers

  private func append(_ message: ChatMessage, to threadID: UUID) {
    guard let ti = threads.firstIndex(where: { $0.id == threadID }) else { return }
    threads[ti].messages.append(message)
  }

  private func update(_ id: UUID, in threadID: UUID, _ body: (inout ChatMessage) -> Void) {
    guard let ti = threads.firstIndex(where: { $0.id == threadID }),
      let mi = threads[ti].messages.firstIndex(where: { $0.id == id })
    else { return }
    body(&threads[ti].messages[mi])
  }

  private func currentUserTurnCount(in threadID: UUID) -> Int {
    threads.first(where: { $0.id == threadID })?.messages.filter { $0.role == .you }.count ?? 0
  }

  private struct PersistedAttachmentMetadata: Codable, Hashable {
    let name: String
    let type: String
    let path: String?
  }

  private struct PersistedAttachmentTurn: Codable, Hashable {
    let userTurnIndex: Int
    let attachments: [PersistedAttachmentMetadata]
  }

  private static let attachmentMetadataDefaultsKey = "AgentChatStore.attachmentMetadata.v1"

  // MARK: Durable accepted-turn sidecar (queue persistence)

  /// Wire form of an accepted message. Keyed by the durable backend thread id
  /// (the local UUID does not survive a restart); FIFO order restores from
  /// `enqueuedAtEpoch`.
  private struct DurableAcceptedTurn: Codable, Hashable {
    let id: UUID
    let backendThreadID: String
    let text: String
    let attachmentPaths: [String]
    let enqueuedAtEpoch: TimeInterval
  }

  static let acceptedTurnsDefaultsKey = "AgentChatStore.acceptedTurns.v1"

  private func persistDurableAcceptedTurn(_ turn: QueuedTurn) {
    var stored = readDurableAcceptedTurns()
    stored.removeAll { $0.id == turn.id }
    stored.append(
      DurableAcceptedTurn(
        id: turn.id,
        backendThreadID: turn.backendThreadID,
        text: turn.text,
        attachmentPaths: turn.attachments.map { $0.url.path },
        enqueuedAtEpoch: turn.enqueuedAt.timeIntervalSince1970
      ))
    writeDurableAcceptedTurns(stored)
  }

  private func removeDurableAcceptedTurn(id: UUID) {
    var stored = readDurableAcceptedTurns()
    let before = stored.count
    stored.removeAll { $0.id == id }
    guard stored.count != before else { return }
    writeDurableAcceptedTurns(stored)
  }

  private func removeDurableAcceptedTurns(backendThreadID: String) {
    var stored = readDurableAcceptedTurns()
    let before = stored.count
    stored.removeAll { $0.backendThreadID == backendThreadID }
    guard stored.count != before else { return }
    writeDurableAcceptedTurns(stored)
  }

  private func readDurableAcceptedTurns() -> [DurableAcceptedTurn] {
    guard let data = UserDefaults.standard.data(forKey: Self.acceptedTurnsDefaultsKey) else {
      return []
    }
    do {
      return try JSONDecoder().decode([DurableAcceptedTurn].self, from: data)
    } catch {
      // Corrupt payload is not "empty queue" — quarantine so the next
      // accept can rewrite a healthy sidecar, and leave an OSLog trail.
      queueLog.error(
        "durable accepted-turns decode failed; quarantining payload: \(error.localizedDescription, privacy: .public)"
      )
      UserDefaults.standard.removeObject(forKey: Self.acceptedTurnsDefaultsKey)
      return []
    }
  }

  private func writeDurableAcceptedTurns(_ turns: [DurableAcceptedTurn]) {
    do {
      let data = try JSONEncoder().encode(turns)
      UserDefaults.standard.set(data, forKey: Self.acceptedTurnsDefaultsKey)
    } catch {
      // Flagship durable queue must never fail silently (PR-68 review).
      queueLog.error(
        "durable accepted-turns encode failed (count=\(turns.count, privacy: .public)): \(error.localizedDescription, privacy: .public)"
      )
    }
  }

  /// Re-enqueue messages that were accepted before the last app death but
  /// never reached a terminal. Runs once the thread index is present (init on
  /// the eager path, after the async index merge on the deferred path).
  private func restoreAcceptedTurnsFromDisk() {
    // Stored order IS acceptance order (append-only sidecar) — no sort, so
    // two messages accepted in the same millisecond can never swap.
    let persisted = readDurableAcceptedTurns()
    guard !persisted.isEmpty else { return }
    for item in persisted {
      guard !queuedTurns.contains(where: { $0.id == item.id }) else { continue }
      let threadID: UUID
      if let existing = threads.first(where: { $0.backendId == item.backendThreadID }) {
        threadID = existing.id
      } else {
        // Accepted for a thread that never reached disk (the app died
        // before its first stream persisted) — re-mint a bound shell.
        var thread = ChatThread(title: "Restored draft", meta: "now")
        thread.backendId = item.backendThreadID
        thread.messagesLoaded = true
        threads.insert(thread, at: 0)
        threadID = thread.id
      }
      queuedTurns.append(
        QueuedTurn(
          id: item.id,
          threadID: threadID,
          backendThreadID: item.backendThreadID,
          text: item.text,
          attachments: item.attachmentPaths.map {
            PendingAttachment(url: URL(fileURLWithPath: $0))
          },
          enqueuedAt: Date(timeIntervalSince1970: item.enqueuedAtEpoch)
        ))
    }
    dispatchNextQueuedTurnIfIdle()
  }

  private func persistAttachmentMetadata(
    _ attachments: [MessageAttachment],
    for backendId: String,
    userTurnIndex: Int
  ) {
    guard !attachments.isEmpty else { return }
    var sidecar = readAttachmentMetadataSidecar()
    var turns = sidecar[backendId, default: []]
    turns.removeAll { $0.userTurnIndex == userTurnIndex }
    turns.append(
      PersistedAttachmentTurn(
        userTurnIndex: userTurnIndex,
        attachments: attachments.map {
          PersistedAttachmentMetadata(name: $0.name, type: $0.type, path: $0.url?.path)
        }
      ))
    sidecar[backendId] = turns.sorted { $0.userTurnIndex < $1.userTurnIndex }
    writeAttachmentMetadataSidecar(sidecar)
  }

  private func applyingPersistedAttachmentMetadata(
    to messages: [ChatMessage],
    backendId: String
  ) -> [ChatMessage] {
    let sidecar = readAttachmentMetadataSidecar()
    let turns = sidecar[backendId] ?? []
    guard !turns.isEmpty else { return messages }
    var byUserTurn: [Int: [PersistedAttachmentMetadata]] = [:]
    for turn in turns {
      byUserTurn[turn.userTurnIndex] = turn.attachments
    }
    var userTurnIndex = 0
    var restored = messages
    for index in restored.indices where restored[index].role == .you {
      if let metadata = byUserTurn[userTurnIndex], !metadata.isEmpty {
        restored[index].attachments = metadata.map {
          let url = $0.path.map(URL.init(fileURLWithPath:))
          return MessageAttachment(name: $0.name, url: url, type: $0.type)
        }
      }
      userTurnIndex += 1
    }
    return restored
  }

  private func removePersistedAttachmentMetadata(for backendId: String) {
    var sidecar = readAttachmentMetadataSidecar()
    guard sidecar.removeValue(forKey: backendId) != nil else { return }
    writeAttachmentMetadataSidecar(sidecar)
  }

  private func readAttachmentMetadataSidecar() -> [String: [PersistedAttachmentTurn]] {
    guard let data = UserDefaults.standard.data(forKey: Self.attachmentMetadataDefaultsKey) else {
      return [:]
    }
    do {
      return try JSONDecoder().decode([String: [PersistedAttachmentTurn]].self, from: data)
    } catch {
      attachLog.error(
        "attachment metadata decode failed; quarantining payload: \(error.localizedDescription, privacy: .public)"
      )
      UserDefaults.standard.removeObject(forKey: Self.attachmentMetadataDefaultsKey)
      return [:]
    }
  }

  private func writeAttachmentMetadataSidecar(_ sidecar: [String: [PersistedAttachmentTurn]]) {
    do {
      let data = try JSONEncoder().encode(sidecar)
      UserDefaults.standard.set(data, forKey: Self.attachmentMetadataDefaultsKey)
    } catch {
      attachLog.error(
        "attachment metadata encode failed (threads=\(sidecar.count, privacy: .public)): \(error.localizedDescription, privacy: .public)"
      )
    }
  }

  /// Surface a completed tool call as a `.tool` activity turn placed immediately
  /// before the streaming assistant bubble (matches the mock's "What I checked").
  private func recordToolActivity(
    name: String, isError: Bool, reason: String, before assistantID: UUID, in threadID: UUID
  ) {
    recordToolResult(
      name: name, callID: nil, isError: isError, reason: reason, before: assistantID, in: threadID)
  }

  private func recordToolStarted(
    name: String, callID rawCallID: String, before assistantID: UUID, in threadID: UUID
  ) {
    let callID = rawCallID.isEmpty ? nil : rawCallID
    guard let ti = threads.firstIndex(where: { $0.id == threadID }),
      let ai = threads[ti].messages.firstIndex(where: { $0.id == assistantID })
    else { return }
    let line = ToolLine(
      callID: callID,
      verb: "tool",
      detail: name,
      state: .running,
      startedAt: Date()
    )
    if let row = toolRowIndex(before: ai, inThreadAt: ti) {
      if let callID,
        let existing = threads[ti].messages[row].toolLines.firstIndex(where: { $0.callID == callID }
        )
      {
        threads[ti].messages[row].toolLines[existing] = line
      } else {
        threads[ti].messages[row].toolLines.append(line)
      }
      updateToolTitle(threadIndex: ti, messageIndex: row)
    } else {
      var tool = ChatMessage(role: .tool, timestamp: now(), text: "")
      tool.toolLines = [line]
      tool.toolTitle = Self.toolTitle(for: tool.toolLines)
      threads[ti].messages.insert(tool, at: ai)
    }
  }

  private func recordToolResult(
    name: String,
    callID rawCallID: String?,
    isError: Bool,
    reason: String,
    before assistantID: UUID,
    in threadID: UUID
  ) {
    let callID = rawCallID.flatMap { $0.isEmpty ? nil : $0 }
    guard let ti = threads.firstIndex(where: { $0.id == threadID }),
      let ai = threads[ti].messages.firstIndex(where: { $0.id == assistantID })
    else { return }
    var startedAt: Date?
    var durationMs: Int?
    if let row = toolRowIndex(before: ai, inThreadAt: ti),
      let callID,
      let existing = threads[ti].messages[row].toolLines.firstIndex(where: { $0.callID == callID })
    {
      startedAt = threads[ti].messages[row].toolLines[existing].startedAt
      if let startedAt {
        durationMs = max(0, Int(Date().timeIntervalSince(startedAt) * 1000))
      }
    }
    let line = ToolLine(
      callID: callID,
      verb: isError ? "failed" : "ran",
      detail: name,
      state: isError ? .failed : .succeeded,
      reason: reason.isEmpty ? nil : reason,
      startedAt: startedAt,
      durationMs: durationMs
    )
    if let row = toolRowIndex(before: ai, inThreadAt: ti) {
      if let callID,
        let existing = threads[ti].messages[row].toolLines.firstIndex(where: { $0.callID == callID }
        )
      {
        threads[ti].messages[row].toolLines[existing] = line
      } else {
        threads[ti].messages[row].toolLines.append(line)
      }
      updateToolTitle(threadIndex: ti, messageIndex: row)
    } else {
      var tool = ChatMessage(role: .tool, timestamp: now(), text: "")
      tool.toolLines = [line]
      tool.toolTitle = Self.toolTitle(for: tool.toolLines)
      threads[ti].messages.insert(tool, at: ai)
    }
  }

  private func finishPendingTools(before assistantID: UUID, in threadID: UUID) {
    guard let ti = threads.firstIndex(where: { $0.id == threadID }),
      let ai = threads[ti].messages.firstIndex(where: { $0.id == assistantID }),
      let row = toolRowIndex(before: ai, inThreadAt: ti)
    else { return }
    var changed = false
    for index in threads[ti].messages[row].toolLines.indices
    where threads[ti].messages[row].toolLines[index].state == .running {
      threads[ti].messages[row].toolLines[index].state = .unknown
      threads[ti].messages[row].toolLines[index].verb = "ended"
      changed = true
    }
    if changed { updateToolTitle(threadIndex: ti, messageIndex: row) }
  }

  private func cancelPendingTools(before assistantID: UUID, in threadID: UUID) {
    guard let ti = threads.firstIndex(where: { $0.id == threadID }),
      let ai = threads[ti].messages.firstIndex(where: { $0.id == assistantID }),
      let row = toolRowIndex(before: ai, inThreadAt: ti)
    else { return }
    var changed = false
    for index in threads[ti].messages[row].toolLines.indices
    where threads[ti].messages[row].toolLines[index].state == .running {
      threads[ti].messages[row].toolLines[index].state = .cancelled
      threads[ti].messages[row].toolLines[index].verb = "stopped"
      changed = true
    }
    if changed { updateToolTitle(threadIndex: ti, messageIndex: row) }
  }

  private func appendReasoning(_ delta: String, to assistantID: UUID, in threadID: UUID) {
    guard !delta.isEmpty else { return }
    update(assistantID, in: threadID) {
      $0.reasoning += delta
    }
  }

  private func toolRowIndex(before assistantIndex: Int, inThreadAt threadIndex: Int) -> Int? {
    guard assistantIndex > 0, threads[threadIndex].messages[assistantIndex - 1].role == .tool else {
      return nil
    }
    return assistantIndex - 1
  }

  private func updateToolTitle(threadIndex: Int, messageIndex: Int) {
    threads[threadIndex].messages[messageIndex].toolTitle = Self.toolTitle(
      for: threads[threadIndex].messages[messageIndex].toolLines
    )
  }

  private static func toolTitle(for lines: [ToolLine]) -> String {
    let count = lines.count
    let running = lines.filter { $0.state == .running }.count
    let cancelled = lines.filter { $0.state == .cancelled }.count
    let noun = count == 1 ? "tool" : "tools"
    if running > 0 {
      return "What I checked · \(running) running · \(count) \(noun)"
    }
    if cancelled > 0 {
      return "What I checked · \(cancelled) stopped · \(count) \(noun)"
    }
    return "What I checked · \(count) \(noun)"
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

  /// Row-level equality on everything the rail renders. Matched incoming
  /// rows reuse the existing `ChatThread` instances (same `id`s), so equal
  /// rows ⇒ identical identity set ⇒ selection resolution is a no-op too.
  private static func railRowsEqual(_ lhs: [ChatThread], _ rhs: [ChatThread]) -> Bool {
    guard lhs.count == rhs.count else { return false }
    return zip(lhs, rhs).allSatisfy { l, r in
      l.id == r.id && l.backendId == r.backendId && l.title == r.title
        && l.meta == r.meta && l.isFavorite == r.isFavorite
        && l.isRestored == r.isRestored
    }
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

    let resolved =
      next.isEmpty && !allowEmpty
      ? [ChatThread(title: "New thread", meta: "now", messages: [])] : next
    // Identical rail rows must not publish: an unchanged `threads =`
    // still tears down and rebuilds the whole window body (the 937189fd
    // lesson). Matched rows reuse existing instances (same ids), so
    // row-equality here also guarantees the selection below would not
    // move — skipping the whole tail is safe.
    if !next.isEmpty || allowEmpty, Self.railRowsEqual(resolved, threads) {
      return
    }
    threads = resolved
    // Selection is user-owned. A completion refresh may reorder or replace
    // rail rows, but it must preserve the thread the user is reading. The
    // completed backend is only a fallback when that selection disappeared.
    if let previousSelectedID, threads.contains(where: { $0.id == previousSelectedID }) {
      selectedThreadID = previousSelectedID
    } else if let backendId, let match = threads.first(where: { $0.backendId == backendId }) {
      selectedThreadID = match.id
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
        text:
          "Two spots. `events/bus.ts` re-emits on retry, and `ui/store.ts` subscribes twice on remount. Want a minimal patch plus a regression test?",
        reasonedSeconds: 2.1
      ),
      ChatMessage(role: .you, timestamp: "18:41", text: "yes, and add the test"),
    ]
    // updatedAt offsets keep the preview's recency sections honest with the
    // hardcoded meta labels.
    let day: TimeInterval = 86_400
    return [
      active,
      ChatThread(title: "rate-limiter spec", meta: "today · 18:40", updatedAt: Date()),
      ChatThread(
        title: "release notes → PL", meta: "yesterday", updatedAt: Date(timeIntervalSinceNow: -day)),
      ChatThread(
        title: "whisper warm-start idea", meta: "yesterday",
        updatedAt: Date(timeIntervalSinceNow: -day)),
      ChatThread(
        title: "standup notes", meta: "Thu", updatedAt: Date(timeIntervalSinceNow: -5 * day)),
    ]
  }
}

// MARK: - Preview engine (canned single-shot reply)

#if DEBUG
  final class MockChatEngine: AgentChatEngine {
    func isAvailable() -> Bool { true }
    func availabilityDetail() -> String? { nil }
    func generateThreadTitle(_ text: String) async throws -> String? { nil }
    func streamReply(
      _ text: String,
      threadId: String,
      attachmentPaths: [String],
      onDelta: @escaping @MainActor (String) -> Void,
      onReasoning: @escaping @MainActor (String) -> Void,
      onToolExecuting: @escaping @MainActor (_ name: String, _ id: String) -> Void,
      onToolResult:
        @escaping @MainActor (_ name: String, _ id: String, _ isError: Bool, _ reason: String) ->
        Void
    ) async throws -> String {
      let seen =
        attachmentPaths.isEmpty
        ? "" : " (saw \(attachmentPaths.count) image\(attachmentPaths.count == 1 ? "" : "s"))"
      let reply =
        "On it — \(text.lowercased())\(seen). I'd start with a minimal patch and a regression test."
      var assembled = ""
      await onReasoning("Reading the turn and checking the smallest useful next step.")
      let mockToolID = "mock-preview-tool"
      await onToolExecuting("preview-context", mockToolID)
      for word in reply.split(separator: " ", omittingEmptySubsequences: false) {
        try? await Task.sleep(nanoseconds: 60_000_000)
        let chunk = (assembled.isEmpty ? "" : " ") + word
        assembled += chunk
        await onDelta(chunk)
      }
      await onToolResult("preview-context", mockToolID, false, "mock context ready")
      return assembled
    }

    func cancelReply(threadId: String) -> Bool { false }
  }
#endif
