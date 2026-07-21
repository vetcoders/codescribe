import SwiftUI
import AppKit

// Owns the tray's state + action routing. The view is dumb: it observes this.
//
// Runtime reads (recording state, agent readiness, quick-toggle values, latest
// transcript) go through the `TrayEngine` seam. Navigation (open chat / settings)
// is emitted as `TrayIntent` through `onIntent`, which App.swift binds
// to real window opens. Other app-level actions stay as injected closures.
@MainActor
final class TrayViewModel: ObservableObject {
    // Runtime status (drives the pill + dictation row).
    @Published var isRecording: Bool
    @Published var isStartingDictation: Bool = false
    @Published var agentAvailable: Bool = true

    // Quick config toggles (reflected on disk via the engine).
    @Published var showDockIcon: Bool = true
    @Published var overlayEnabled: Bool = true
    @Published var autoPasteEnabled: Bool = true
    @Published var autoFormatLevel: FormattingPolicyOption = .correction
    @Published var notesModeEnabled: Bool = false
    @Published var startInAssistive: Bool = false
    @Published var holdBadgeOption: HoldBadgeOption = .twelve

    // Disclosure state for the nested groups. Notes is expanded by default to
    // match the static mock; Diagnostics and History are collapsed.
    @Published var notesExpanded: Bool = true
    @Published var diagnosticsExpanded: Bool = false
    @Published var historyExpanded: Bool = false

    // Transient result banner for the Notes actions ("Save selection" / "Save
    // last transcript"). Rendered inside the still-open popover so the user gets
    // an unmissable, permission-free confirmation even when the OS notification
    // path is silent (accessory app / not-yet-granted). Auto-clears.
    @Published private(set) var noteStatus: TrayActionStatus?
    private var noteStatusClearTask: Task<Void, Never>?

    // Transient "Copied" confirmation for the copy actions (history item / last
    // transcript). Same permission-free banner pattern as `noteStatus`, kept
    // separate so copy feedback shows next to the copy actions rather than in the
    // Notes group. Auto-clears.
    @Published private(set) var copyStatus: TrayActionStatus?
    private var copyStatusClearTask: Task<Void, Never>?

    // The 5 most recent transcripts, loaded when the History group is expanded
    // (cached so re-renders don't re-hit disk).
    @Published private(set) var historyItems: [TrayTranscript] = []

    private let engine: TrayEngine?
    private var holdBadgeObserver: NSObjectProtocol?

    // Navigation intents — bound by App.swift to the actual window/scene opens.
    var onIntent: (TrayIntent) -> Void = { _ in }
    var onDictationStartRequested: () -> Void = {}

    // App-level actions — injected by App.swift. Defaults are best-effort / no-op
    // so the screen is fully interactive in isolation and in #Preview.
    var onHelp: () -> Void = {}
    var onAbout: () -> Void = {}
    /// Re-open the first-run setup wizard. Bound by App.swift to the onboarding
    /// window controller; a stable auxiliary-menu entry so setup is always
    /// reachable — mid-onboarding (resume) or after completion (re-run).
    var onOpenSetupWizard: () -> Void = {}
    var onQuit: () -> Void = { NSApplication.shared.terminate(nil) }

    var onSaveLastTranscript: () -> Void = {}
    var onSaveSelection: () -> Void = {}
    var onOpenNotesFolder: () -> Void = {}
    var onOpenTodayNote: () -> Void = {}

    var onOpenLogFolder: () -> Void = {}
    var onCopyDebugInfo: () -> Void = {}

    init(engine: TrayEngine? = nil, isRecording: Bool = false) {
        self.engine = engine
        self.isRecording = isRecording
        // K4: Settings writes arrive on the bus; reload tray badge display.
        holdBadgeObserver = NotificationCenter.default.addObserver(
            forName: ConfigChangeBus.holdBadgeDidChange,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.reloadHoldBadgeFromDisk()
            }
        }
    }

    deinit {
        if let holdBadgeObserver {
            NotificationCenter.default.removeObserver(holdBadgeObserver)
        }
    }

    // MARK: - Navigation intents

    func onShowAgent() { onIntent(.openChat) }

    // MARK: - Derived status (mock copy + palette)

    /// Olive "Idle" when stopped, terracotta "Recording" when live.
    var statusText: String {
        if isStartingDictation { return "Starting" }
        return isRecording ? "Recording" : "Idle"
    }
    var statusColor: Color { (isRecording || isStartingDictation) ? CSColor.terracotta : CSColor.oliveLight }

    /// Pull prompt-free runtime flags from the engine (call on appear).
    func refreshStatus() {
        guard let engine else { return }
        if let toggles = engine.currentToggles() {
            showDockIcon = toggles.showDockIcon
            overlayEnabled = toggles.overlayEnabled
            autoPasteEnabled = toggles.autoPasteEnabled
            autoFormatLevel = toggles.autoFormatLevel
            notesModeEnabled = toggles.notesMode
            startInAssistive = toggles.startInAssistive
            holdBadgeOption = toggles.holdBadgeOption
        }
        Task { [weak self] in
            guard let self else { return }
            self.isRecording = await engine.isRecording()
        }
    }

    // MARK: - Dictation toggle

    /// Flip the dictation session, then reconcile against the engine's truth.
    func toggleDictation() {
        // Ignore re-entry while a start is still in flight: a second tap would
        // read the optimistic `isRecording = true` as `wasRecording` and fire a
        // stop before the start's Task resolves, ordering stop before start.
        // `isStartingDictation` is cleared unconditionally once that Task finishes.
        guard !isStartingDictation else { return }
        guard let engine else { isRecording.toggle(); return }
        let wasRecording = isRecording
        let shouldStartAssistive = startInAssistive
        if !wasRecording {
            isStartingDictation = true
            isRecording = true
            onDictationStartRequested()
        }
        Task { [weak self] in
            guard let self else { return }
            do {
                if wasRecording { try await engine.stopRecording() }
                else { try await engine.startRecording(assistive: shouldStartAssistive) }
            } catch {
                // Swallow: the reconcile below reflects the real session state.
            }
            self.isStartingDictation = false
            self.isRecording = await engine.isRecording()
        }
    }

    // MARK: - Quick config toggles

    func setShowDockIcon(_ enabled: Bool) {
        showDockIcon = enabled
        engine?.setQuickToggle(.showDockIcon, enabled: enabled)
        // Persisting the flag isn't enough: the app launches as an accessory
        // (LSUIElement), so flip the activation policy to actually show/hide the
        // Dock icon at runtime.
        NSApp.setActivationPolicy(enabled ? .regular : .accessory)
    }

    func setOverlayEnabled(_ enabled: Bool) {
        guard let engine else {
            overlayEnabled = enabled
            return
        }
        engine.setQuickToggle(.transcriptionOverlay, enabled: enabled)
        refreshStatus()
    }

    /// Persisted delivery policy. Re-read the complete tray snapshot after the
    /// write so a rejected save never leaves an optimistic switch behind.
    func setAutoPasteEnabled(_ enabled: Bool) {
        guard let engine else {
            autoPasteEnabled = enabled
            return
        }
        engine.setAutoPasteEnabled(enabled)
        refreshStatus()
    }

    /// Persist one of the four normalized formatting IDs, then reconcile with
    /// prompt-free settings truth even when the bridge rejects the write.
    func setAutoFormatLevel(_ level: FormattingPolicyOption) {
        guard let engine else {
            autoFormatLevel = level
            return
        }
        engine.setAutoFormatLevel(level)
        refreshStatus()
    }

    /// K3: persists immediately; next badge show uses the new size.
    /// K4: posts bus so Settings reflects the tray cycle without reopen.
    func setHoldBadgeOption(_ option: HoldBadgeOption) {
        guard let engine else {
            holdBadgeOption = option
            ConfigChangeBus.postHoldBadgeChanged()
            return
        }
        if engine.setHoldBadgeOption(option) {
            holdBadgeOption = option
            ConfigChangeBus.postHoldBadgeChanged()
        } else {
            refreshStatus()
        }
    }

    /// Peer-surface reload after Settings wrote HOLD_BADGE_SIZE / HOLD_INDICATOR.
    func reloadHoldBadgeFromDisk() {
        refreshStatus()
    }

    /// Notes Mode: dictation → daily note (no paste). Distinct from normal
    /// dictation, which pastes at the cursor. Only reflect the new state if the
    /// two-key write actually persisted — otherwise re-sync to on-disk truth so
    /// the toggle never shows a state the config doesn't hold.
    func setNotesMode(_ enabled: Bool) {
        guard let engine else {
            notesModeEnabled = enabled
            return
        }
        if engine.setNotesMode(enabled) {
            notesModeEnabled = enabled
        } else {
            refreshStatus()
        }
    }

    /// UI-initiated recording lane. Keyboard shortcuts keep their own bindings.
    func setStartInAssistive(_ enabled: Bool) {
        guard let engine else {
            startInAssistive = enabled
            return
        }
        if engine.setStartInAssistive(enabled) {
            startInAssistive = enabled
        } else {
            refreshStatus()
        }
    }

    // MARK: - History actions (route through the engine seam)

    /// Toggle the "Open history" disclosure, loading the 5 most recent
    /// transcripts from the engine the moment it opens.
    func toggleHistory() {
        historyExpanded.toggle()
        if historyExpanded {
            historyItems = engine?.recentTranscripts(limit: 5) ?? []
        }
    }

    /// Copy a chosen recent transcript's full text to the system pasteboard.
    func copyTranscript(path: String) {
        guard let text = engine?.transcriptText(forPath: path) else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
        showCopyStatus("Copied")
    }

    /// Reveal the folder holding the most recent transcript in Finder.
    func openHistoryFolder() {
        guard let path = engine?.latestHistoryPath() else { return }
        let dir = (path as NSString).deletingLastPathComponent
        NSWorkspace.shared.open(URL(fileURLWithPath: dir))
    }

    /// Copy the most recent transcript's text to the system pasteboard.
    func copyLastTranscript() {
        guard let text = engine?.latestTranscriptText() else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
        showCopyStatus("Copied")
    }

    /// Flash a transient "Copied" banner beside the copy actions, then auto-clear
    /// it (cancelling any in-flight clear so rapid copies keep the newest banner).
    private func showCopyStatus(_ message: String) {
        copyStatus = TrayActionStatus(kind: .success, message: message)
        copyStatusClearTask?.cancel()
        copyStatusClearTask = Task { @MainActor [weak self] in
            try? await Task.sleep(nanoseconds: 4_000_000_000)
            guard !Task.isCancelled else { return }
            self?.copyStatus = nil
        }
    }

    // MARK: - Notes action feedback

    /// Surface the outcome of a Notes action in the popover and auto-clear it a
    /// few seconds later. Cancels any in-flight clear so back-to-back actions
    /// don't wipe the newest banner early.
    func showNoteStatus(_ status: TrayActionStatus) {
        noteStatus = status
        noteStatusClearTask?.cancel()
        noteStatusClearTask = Task { @MainActor [weak self] in
            try? await Task.sleep(nanoseconds: 4_000_000_000)
            guard !Task.isCancelled else { return }
            self?.noteStatus = nil
        }
    }
}

/// Outcome of a tray Notes action, shown as a transient banner row.
struct TrayActionStatus: Equatable {
    enum Kind { case success, failure }
    let kind: Kind
    let message: String
}
