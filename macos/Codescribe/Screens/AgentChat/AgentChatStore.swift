import AppKit
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
        onToolResult: @escaping @MainActor (_ name: String, _ id: String, _ isError: Bool, _ reason: String) -> Void
    ) async throws -> String
    /// Abort the engine-side turn running for `threadId` (safe no-op when idle).
    /// Cancelling the Swift `Task` that awaits `streamReply` is NOT enough: the
    /// generated UniFFI bindings poll the Rust future to completion, so without
    /// this call the agent keeps executing tools (typing/clipboard/fs) after a
    /// "cancelled" turn.
    @discardableResult
    func cancelReply(threadId: String) -> Bool
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
    var verb: String     // "grep", "read" — rendered olive; "failed" — terracotta
    let detail: String   // "events/bus.ts · ui/store.ts"
    var state: ToolLineState
    /// Failure reason for a `failed` line (from the tool's error output). `nil`
    /// for successful lines and for reloaded/persisted turns, which do not carry
    /// the reason. Drives the expandable disclosure in the tool-activity row.
    var reason: String?

    init(
        id: UUID = UUID(),
        callID: String? = nil,
        verb: String,
        detail: String,
        state: ToolLineState = .succeeded,
        reason: String? = nil
    ) {
        self.id = id
        self.callID = callID
        self.verb = verb
        self.detail = detail
        self.state = state
        self.reason = reason
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
    var toolTitle: String = ""        // "What I checked · 2 tools"
    var toolLines: [ToolLine] = []

    // Assistant turn
    var reasonedSeconds: Double? = nil
    var isThinking: Bool = false      // pre-reply "thinking…" state
    var isStreaming: Bool = false     // word-reveal in progress (shows caret)
    var wasStopped: Bool = false      // cancelled terminal; partial text remains intact
    var reasoning: String = ""        // streamed model reasoning, rendered separately
    var renderMode: MessageRenderMode = .raw  // raw default (C2b); rich = opt-in
}

/// An image the user staged in the composer but has not sent yet. Referenced by
/// file URL (NSOpenPanel / clipboard-saved temp file); the send path forwards the
/// path to the bridge, which loads + validates the bytes.
struct PendingAttachment: Identifiable, Hashable {
    let id = UUID()
    let url: URL
    var name: String { url.lastPathComponent }
    var type: String { MessageAttachment.inferredType(name: name, url: url) }
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
        let ext = (url?.pathExtension.isEmpty == false ? url?.pathExtension : nil) ?? (name as NSString).pathExtension
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
    var meta: String        // mono subtitle, e.g. "active · restored" / "today 18:40 · gpt-5 · 1.2k tok"
    var isRestored: Bool = false
    var isFavorite: Bool = false
    var backendId: String? = nil      // codescribe ThreadStore id (nil = local-only, not yet persisted)
    var messagesLoaded: Bool = false  // lazy-load guard for persisted threads
    var messages: [ChatMessage] = []
    var updatedAt: Date? = nil        // nil (local-only draft) groups under Today
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
              collapsed.contains(where: { $0.isLetter || $0.isNumber }) else { return nil }
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
            let keepSpace = leftEnd > 0
                && rightStart < chars.count
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
                      Array(chars[(open + 1)..<digitsStart]) == labelChars else { continue }
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
        fragment.count >= 2 && !fragment.contains { ch in
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
    case preparing   // permission / model load / start-stop transition in flight
    case recording
    case failed(String)
}

/// UI-only seam over the composer dictation controller. The real adapter
/// (`RealComposerDictation`, Core layer) wraps the `CodescribeDictation` bridge;
/// kept bridge-free here so the view-model + #Preview stay standalone (nil = mic
/// is a no-op, e.g. in previews).
protocol ComposerDictating: AnyObject {
    /// Start recording when idle, stop-and-insert when recording.
    func toggle()
}

// MARK: - Store

@MainActor
final class AgentChatStore: ObservableObject {
    @Published var threads: [ChatThread]
    @Published var selectedThreadID: UUID?
    @Published var draft: String = ""
    /// Monotonic UI command consumed by the composer. It carries no text and
    /// deliberately does not mutate the selected thread or staged attachments.
    @Published private(set) var composerFocusRequest: UInt64 = 0
    @Published private(set) var dictationPreview: String = ""

    /// Images staged in the composer for the next message. Cleared when the
    /// message is dispatched.
    @Published var pendingAttachments: [PendingAttachment] = []

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

    func requestComposerFocus() {
        composerFocusRequest &+= 1
    }

    /// Set by the real adapter as the dictation session transitions. No-op-safe
    /// when no adapter is wired.
    func setDictationPhase(_ phase: ComposerDictationPhase) { dictationPhase = phase }

    /// Latest live voice-note preview. This is a snapshot buffer from the STT
    /// listener, not a delta stream, and stays separate from `draft` until stop.
    func updateDictationPreview(_ text: String) {
        dictationPreview = text.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    func clearDictationPreview() {
        guard !dictationPreview.isEmpty else { return }
        dictationPreview = ""
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

    /// Append a finished voice-note transcript to the current draft with a natural
    /// separator (no auto-send — the user decides when to dispatch).
    func appendDictatedTranscript(_ text: String) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        clearDictationPreview()
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

    init(engine: AgentChatEngine? = nil,
         threadsProvider: ChatThreadsProviding? = nil,
         threads: [ChatThread]? = nil,
         voiceTurnCanceller: VoiceTurnCancelling? = nil) {
        self.engine = engine
        self.threadsProvider = threadsProvider
        self.voiceTurnCanceller = voiceTurnCanceller

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
        beginObservingExternalThreadChanges()
    }

    deinit {
        for observer in externalThreadsObservers {
            NotificationCenter.default.removeObserver(observer)
        }
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
            MainActor.assumeIsolated { self?.refreshThreadsFromExternalChange() }
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
            guard threadsProvider?.setThreadFavorite(backendId: backendId, isFavorite: next) == true else { return }
        }
        threads[ti].isFavorite = next
    }

    /// Rename a thread from the rail. Persists through the threads provider when
    /// the thread is backed on disk; a not-yet-persisted local thread is renamed
    /// in memory only. No-ops on an empty or unchanged title. The chat header
    /// reads `currentThread.title`, so it updates reactively too.
    func rename(_ thread: ChatThread, to newTitle: String) {
        guard let trimmed = ThreadTitlePolicy.normalized(newTitle), trimmed != thread.title,
              let ti = threads.firstIndex(where: { $0.id == thread.id }) else { return }
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
        // Cancel any in-flight reply for this thread so its post-stream refresh
        // can't re-list (and the caret/finalize can't mutate) a deleted thread.
        // Swift-task cancel first (so the awaiting send sees isCancelled and
        // stays silent), then the engine-side cancel, which actually aborts the
        // Rust turn — stopping tool side effects, not just the UI updates.
        inFlightSends[thread.id]?.task.cancel()
        inFlightSends[thread.id] = nil
        if let backendId = thread.backendId {
            _ = engine?.cancelReply(threadId: backendId)
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
              !threads[ti].messagesLoaded else { return }
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
    /// both. Drives the send button's enabled state.
    var canSend: Bool {
        activeComposerTurn == nil
            && !(voiceTurnThreadID == selectedThreadID && voiceTurnPhase != nil)
            && (!draft.trimmingCharacters(in: .whitespaces).isEmpty || !pendingAttachments.isEmpty)
    }

    // MARK: Send (real single-shot FFI round-trip)

    func send() {
        guard activeComposerTurn == nil,
              !(voiceTurnThreadID == selectedThreadID && voiceTurnPhase != nil) else { return }
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        let staged = pendingAttachments
        let attachmentPaths = staged.map { $0.url.path }
        attachLog.info(
            "send: building request attachmentPaths.count=\(attachmentPaths.count, privacy: .public) text.isEmpty=\(text.isEmpty, privacy: .public)"
        )
        guard (!text.isEmpty || !attachmentPaths.isEmpty), let threadID = selectedThreadID else { return }
        let backendId = ensureBackendId(threadID)
        let userTurnIndex = currentUserTurnCount(in: threadID)
        draft = ""
        pendingAttachments = []

        // Carry the staged attachments onto the You bubble so the sender sees a
        // chip (name + optional thumbnail) for what they attached.
        let sent = staged.map { MessageAttachment(name: $0.name, url: $0.url, type: $0.type) }
        persistAttachmentMetadata(sent, for: backendId, userTurnIndex: userTurnIndex)
        append(ChatMessage(role: .you, timestamp: now(), text: text, attachments: sent), to: threadID)
        let assistant = ChatMessage(role: .assistant, timestamp: "now", text: "", isThinking: true)
        let assistantID = assistant.id
        append(assistant, to: threadID)
        let turnID = UUID()
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
                releaseComposerTurn(turnID, in: threadID)
            }
            guard let engine else {
                finish(assistantID, in: threadID,
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
                        guard self?.acceptsComposerEvent(turnID, assistantID: assistantID, in: threadID) == true else {
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
                        guard self?.acceptsComposerEvent(turnID, assistantID: assistantID, in: threadID) == true else {
                            return
                        }
                        self?.appendReasoning(delta, to: assistantID, in: threadID)
                    },
                    onToolExecuting: { [weak self] name, id in
                        guard self?.acceptsComposerEvent(turnID, assistantID: assistantID, in: threadID) == true else {
                            return
                        }
                        self?.recordToolStarted(name: name, callID: id, before: assistantID, in: threadID)
                    },
                    onToolResult: { [weak self] name, id, isError, reason in
                        guard self?.acceptsComposerEvent(turnID, assistantID: assistantID, in: threadID) == true else {
                            return
                        }
                        self?.recordToolResult(name: name, callID: id, isError: isError, reason: reason,
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
                finish(assistantID, in: threadID,
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
              state.backendThreadID == backendThreadID else { return }
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
              let originalTitle = threads.first(where: { $0.id == threadID })?.title else { return }
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
        launchFirstTurnTitle(presentedText, for: threadID, backendThreadID: backendThreadID, engine: engine)
    }

    private func finishTitleGenerationWithoutRequest(for threadID: UUID) {
        guard var state = firstTurnTitleStates[threadID] else { return }
        state.generationFinished = true
        firstTurnTitleStates[threadID] = state
        cleanUpFirstTurnTitleStateIfFinished(for: threadID)
    }

    private func receiveGeneratedTitle(_ title: String?, for threadID: UUID, generationID: UUID) {
        guard var state = firstTurnTitleStates[threadID], state.generationID == generationID else { return }
        state.generationFinished = true
        guard let trimmed = ThreadTitlePolicy.normalized(title),
              !customTitleThreadIDs.contains(threadID),
              state.pendingCustomTitle == nil,
              let ti = threads.firstIndex(where: { $0.id == threadID }) else {
            firstTurnTitleStates[threadID] = state
            cleanUpFirstTurnTitleStateIfFinished(for: threadID)
            return
        }

        threads[ti].title = trimmed
        let persisted = threadsProvider?.setGeneratedTitle(
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
        guard var state = firstTurnTitleStates[threadID], state.generationID == generationID else { return }
        state.generationFinished = true
        firstTurnTitleStates[threadID] = state
        cleanUpFirstTurnTitleStateIfFinished(for: threadID)
    }

    /// Mark the Rust stream (and therefore its best-effort first persistence)
    /// complete, flush a queued custom rename first, otherwise retry one queued
    /// generated title exactly once. This runs before `refreshThreads`.
    private func settleFirstTurnTitleAfterStream(for threadID: UUID, backendThreadID: String) {
        guard var state = firstTurnTitleStates[threadID], state.backendThreadID == backendThreadID else { return }
        guard !state.streamCompleted else { return }
        state.streamCompleted = true

        if let customTitle = state.pendingCustomTitle {
            _ = threadsProvider?.renameThread(backendId: backendThreadID, title: customTitle)
            state.pendingCustomTitle = nil
            state.pendingGeneratedTitle = nil
        } else if let generatedTitle = state.pendingGeneratedTitle {
            let persisted = threadsProvider?.setGeneratedTitle(
                backendId: backendThreadID,
                title: generatedTitle
            ) == true
            state.pendingGeneratedTitle = nil
            if !persisted,
               !customTitleThreadIDs.contains(threadID),
               let ti = threads.firstIndex(where: { $0.id == threadID }),
               threads[ti].title == generatedTitle {
                threads[ti].title = state.originalTitle
            }
        }

        firstTurnTitleStates[threadID] = state
        cleanUpFirstTurnTitleStateIfFinished(for: threadID)
    }

    /// Queue a rename only for the active first-turn missing-file window.
    /// One dictionary slot means repeated UI commits collapse to the latest
    /// custom title, while generated persistence is discarded immediately.
    private func queueCustomTitle(_ title: String, for threadID: UUID, backendThreadID: String) -> Bool {
        guard var state = firstTurnTitleStates[threadID],
              state.backendThreadID == backendThreadID,
              !state.streamCompleted else { return false }
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
              state.pendingCustomTitle == nil else { return }
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
           let backendId = threads.first(where: { $0.id == threadID })?.backendId {
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
              turn.phase != .cancelling else { return }

        turn.phase = .cancelling
        activeComposerTurn = turn
        inFlightSends[turn.threadID]?.task.cancel()
        let firstAcknowledgement = engine?.cancelReply(threadId: turn.backendThreadID) ?? false

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
                      let engine = self.engine else { break }
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

    /// Open a voice turn: bind (or create) a thread for the core `backendId`,
    /// insert the You-bubble + an assistant placeholder, and select it so the live
    /// reply is visible. Subsequent `ingestVoice*` calls target this turn.
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
        } else {
            let title = ThreadTitlePolicy.normalized(userTurn.text, limit: 48) ?? "Voice chat"
            var thread = ChatThread(title: title, meta: "now")
            thread.backendId = backendId
            thread.messagesLoaded = true  // freshly bound to a core id → in sync
            threads.insert(thread, at: 0)
            threadID = thread.id
            isFirstExchange = true
        }
        selectedThreadID = threadID

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
              let threadID = voiceTurnThreadID, let id = voiceAssistantID else { return }
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
              let threadID = voiceTurnThreadID, let id = voiceAssistantID else { return }
        voiceTurnPhase = .streaming
        appendReasoning(delta, to: id, in: threadID)
    }

    /// Final assembled text for the turn. Only used as a fallback when the reply
    /// arrived without token deltas (otherwise the bubble already holds the text).
    func ingestVoiceTextDone(_ text: String) {
        guard voiceTurnPhase != .cancelling,
              let threadID = voiceTurnThreadID, let id = voiceAssistantID else { return }
        voiceTurnPhase = .streaming
        update(id, in: threadID) { if $0.text.isEmpty { $0.text = text } }
    }

    /// Surface a pending tool call for the active voice turn. The bridge's `id`
    /// is kept end-to-end so the matching result can update this row in place.
    func ingestVoiceToolExecuting(name: String, id callID: String) {
        guard voiceTurnPhase != .cancelling,
              let threadID = voiceTurnThreadID, let assistantID = voiceAssistantID else { return }
        voiceTurnPhase = .streaming
        recordToolStarted(name: name, callID: callID, before: assistantID, in: threadID)
    }

    /// Surface a completed tool call for the active voice turn (same rendering as
    /// the composer path's tool-activity row).
    func ingestVoiceToolResult(name: String, id callID: String, isError: Bool, reason: String) {
        guard voiceTurnPhase != .cancelling,
              let threadID = voiceTurnThreadID, let assistantID = voiceAssistantID else { return }
        voiceTurnPhase = .streaming
        recordToolResult(name: name, callID: callID, isError: isError, reason: reason, before: assistantID, in: threadID)
    }

    /// Finalize the active voice turn and pull disk truth (the core persisted the
    /// thread). No re-persist here — the store only mirrors what the core wrote.
    /// This is also the title coordinator's stream-completed settle point: core
    /// persistence finished before this terminal, so a queued generated title
    /// flushes exactly once here, before the rail refresh.
    func ingestVoiceDone() {
        guard voiceTurnPhase != .cancelling,
              let threadID = voiceTurnThreadID, let id = voiceAssistantID else { return }
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
              let threadID = voiceTurnThreadID, let id = voiceAssistantID else { return }
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
              threads.first(where: { $0.id == threadID })?.backendId == backendId else { return }
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
        guard var turn = activeComposerTurn, turn.id == turnID, turn.phase != .cancelling else { return }
        turn.phase = phase
        activeComposerTurn = turn
    }

    private func releaseComposerTurn(_ turnID: UUID, in threadID: UUID) {
        if inFlightSends[threadID]?.id == turnID {
            inFlightSends[threadID] = nil
        }
        if activeComposerTurn?.id == turnID,
           activeComposerTurn?.phase != .cancelling {
            activeComposerTurn = nil
        }
    }

    private func settleStoppedComposerTurn(_ turn: ActiveComposerTurn) {
        guard activeComposerTurn?.id == turn.id,
              activeComposerTurn?.phase == .cancelling else { return }
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
    }

    private func clearVoiceTurnState() {
        voiceTurnThreadID = nil
        voiceAssistantID = nil
        voiceTurnStartedAt = nil
        voiceTurnPhase = nil
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

    private func currentUserTurnCount(in threadID: UUID) -> Int {
        threads.first(where: { $0.id == threadID })?.messages.filter { $0.role == .you }.count ?? 0
    }

    private struct PersistedAttachmentMetadata: Codable, Hashable {
        let name: String
        let type: String
    }

    private struct PersistedAttachmentTurn: Codable, Hashable {
        let userTurnIndex: Int
        let attachments: [PersistedAttachmentMetadata]
    }

    private static let attachmentMetadataDefaultsKey = "AgentChatStore.attachmentMetadata.v1"

    private func persistAttachmentMetadata(
        _ attachments: [MessageAttachment],
        for backendId: String,
        userTurnIndex: Int
    ) {
        guard !attachments.isEmpty else { return }
        var sidecar = readAttachmentMetadataSidecar()
        var turns = sidecar[backendId, default: []]
        turns.removeAll { $0.userTurnIndex == userTurnIndex }
        turns.append(PersistedAttachmentTurn(
            userTurnIndex: userTurnIndex,
            attachments: attachments.map { PersistedAttachmentMetadata(name: $0.name, type: $0.type) }
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
                    MessageAttachment(name: $0.name, url: nil, type: $0.type)
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
        guard let data = UserDefaults.standard.data(forKey: Self.attachmentMetadataDefaultsKey),
              let decoded = try? JSONDecoder().decode([String: [PersistedAttachmentTurn]].self, from: data) else {
            return [:]
        }
        return decoded
    }

    private func writeAttachmentMetadataSidecar(_ sidecar: [String: [PersistedAttachmentTurn]]) {
        guard let data = try? JSONEncoder().encode(sidecar) else { return }
        UserDefaults.standard.set(data, forKey: Self.attachmentMetadataDefaultsKey)
    }

    /// Surface a completed tool call as a `.tool` activity turn placed immediately
    /// before the streaming assistant bubble (matches the mock's "What I checked").
    private func recordToolActivity(name: String, isError: Bool, reason: String, before assistantID: UUID, in threadID: UUID) {
        recordToolResult(name: name, callID: nil, isError: isError, reason: reason, before: assistantID, in: threadID)
    }

    private func recordToolStarted(name: String, callID rawCallID: String, before assistantID: UUID, in threadID: UUID) {
        let callID = rawCallID.isEmpty ? nil : rawCallID
        guard let ti = threads.firstIndex(where: { $0.id == threadID }),
              let ai = threads[ti].messages.firstIndex(where: { $0.id == assistantID }) else { return }
        let line = ToolLine(callID: callID, verb: "tool", detail: name, state: .running)
        if let row = toolRowIndex(before: ai, inThreadAt: ti) {
            if let callID,
               let existing = threads[ti].messages[row].toolLines.firstIndex(where: { $0.callID == callID }) {
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
              let ai = threads[ti].messages.firstIndex(where: { $0.id == assistantID }) else { return }
        let line = ToolLine(
            callID: callID,
            verb: isError ? "failed" : "ran",
            detail: name,
            state: isError ? .failed : .succeeded,
            reason: (isError && !reason.isEmpty) ? reason : nil
        )
        if let row = toolRowIndex(before: ai, inThreadAt: ti) {
            if let callID,
               let existing = threads[ti].messages[row].toolLines.firstIndex(where: { $0.callID == callID }) {
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
              let row = toolRowIndex(before: ai, inThreadAt: ti) else { return }
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
              let row = toolRowIndex(before: ai, inThreadAt: ti) else { return }
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
        guard assistantIndex > 0, threads[threadIndex].messages[assistantIndex - 1].role == .tool else { return nil }
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
        // updatedAt offsets keep the preview's recency sections honest with the
        // hardcoded meta labels.
        let day: TimeInterval = 86_400
        return [
            active,
            ChatThread(title: "rate-limiter spec", meta: "today · 18:40", updatedAt: Date()),
            ChatThread(title: "release notes → PL", meta: "yesterday", updatedAt: Date(timeIntervalSinceNow: -day)),
            ChatThread(title: "whisper warm-start idea", meta: "yesterday", updatedAt: Date(timeIntervalSinceNow: -day)),
            ChatThread(title: "standup notes", meta: "Thu", updatedAt: Date(timeIntervalSinceNow: -5 * day)),
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
        onToolResult: @escaping @MainActor (_ name: String, _ id: String, _ isError: Bool, _ reason: String) -> Void
    ) async throws -> String {
        let seen = attachmentPaths.isEmpty ? "" : " (saw \(attachmentPaths.count) image\(attachmentPaths.count == 1 ? "" : "s"))"
        let reply = "On it — \(text.lowercased())\(seen). I'd start with a minimal patch and a regression test."
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
