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

    // Disclosure state for the nested groups. Notes is expanded by default to
    // match the static mock; Diagnostics is collapsed.
    @Published var notesExpanded: Bool = true
    @Published var diagnosticsExpanded: Bool = false

    private let engine: TrayEngine?

    // Navigation intents — bound by App.swift to the actual window/scene opens.
    var onIntent: (TrayIntent) -> Void = { _ in }
    var onDictationStartRequested: () -> Void = {}

    // App-level actions — injected by App.swift. Defaults are best-effort / no-op
    // so the screen is fully interactive in isolation and in #Preview.
    var onHelp: () -> Void = {}
    var onAbout: () -> Void = {}
    var onQuit: () -> Void = { NSApplication.shared.terminate(nil) }

    var onQuickNotes: () -> Void = {}
    var onSaveOnlyNotes: () -> Void = {}
    var onOpenNotesFolder: () -> Void = {}
    var onOpenTodayNote: () -> Void = {}

    var onOpenLogFolder: () -> Void = {}
    var onCopyDebugInfo: () -> Void = {}

    init(engine: TrayEngine? = nil, isRecording: Bool = false) {
        self.engine = engine
        self.isRecording = isRecording
    }

    // MARK: - Navigation intents

    func onShowAgent() { onIntent(.openChat) }
    func onOpenSettings() { onIntent(.openSettings) }

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
        }
        Task { [weak self] in
            guard let self else { return }
            self.isRecording = await engine.isRecording()
        }
    }

    // MARK: - Dictation toggle

    /// Flip the dictation session, then reconcile against the engine's truth.
    func toggleDictation() {
        guard let engine else { isRecording.toggle(); return }
        let wasRecording = isRecording
        if !wasRecording {
            isStartingDictation = true
            isRecording = true
            onDictationStartRequested()
        }
        Task { [weak self] in
            guard let self else { return }
            do {
                if wasRecording { try await engine.stopRecording() }
                else { try await engine.startRecording() }
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
    }

    func setOverlayEnabled(_ enabled: Bool) {
        overlayEnabled = enabled
        engine?.setQuickToggle(.transcriptionOverlay, enabled: enabled)
    }

    // MARK: - History actions (route through the engine seam)

    /// Reveal the most recent transcript file in Finder.
    func openHistory() {
        guard let path = engine?.latestHistoryPath() else { return }
        NSWorkspace.shared.open(URL(fileURLWithPath: path))
    }

    /// Copy the most recent transcript's text to the system pasteboard.
    func copyLastTranscript() {
        guard let text = engine?.latestTranscriptText() else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }
}
