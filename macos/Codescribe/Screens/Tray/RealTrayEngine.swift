import Foundation

// Backs the tray with the REAL codescribe core via the UniFFI bridge. It is a
// composite over four thin handles:
//   • CodescribeAgent    — assistive-provider readiness gate.
//   • CodescribeHotkeys  — shared legacy controller hotkey/recording spine.
//   • CodescribeConfig    — quick config toggles (settings.json / .env router).
//   • CodescribeThreads   — most-recent transcript path + text.
//
// Dictation deliberately routes through CodescribeHotkeys instead of
// CodescribeDictation so tray + keyboard shortcuts share one RecordingController
// and cannot open two independent overlays/recorders.
final class RealTrayEngine: TrayEngine {
    private let agent: CodescribeAgent
    private let hotkeys: CodescribeHotkeys
    private let config: CodescribeConfig
    private let threads: CodescribeThreads

    init(
        agent: CodescribeAgent = CodescribeAgent(),
        hotkeys: CodescribeHotkeys = CodescribeHotkeys(),
        config: CodescribeConfig = CodescribeConfig(),
        threads: CodescribeThreads = CodescribeThreads()
    ) {
        self.agent = agent
        self.hotkeys = hotkeys
        self.config = config
        self.threads = threads
    }

    func isAgentAvailable() -> Bool { agent.isAvailable() }

    func isRecording() async -> Bool { await hotkeys.isRecording() }

    func startRecording(assistive: Bool) async throws {
        if assistive {
            try await hotkeys.startAssistiveRecording()
        } else {
            try await hotkeys.startRecording()
        }
    }

    func stopRecording() async throws {
        try await hotkeys.stopRecording()
    }

    func currentToggles() -> (
        showDockIcon: Bool,
        overlayEnabled: Bool,
        autoPasteEnabled: Bool,
        autoFormatLevel: FormattingPolicyOption,
        notesMode: Bool,
        startInAssistive: Bool
    )? {
        let toggles = config.trayToggles()
        guard let formatLevel = FormattingPolicyOption(rawValue: toggles.formattingLevel) else {
            return nil
        }
        return (
            toggles.showDockIcon,
            toggles.transcriptionOverlayEnabled,
            toggles.autoPasteEnabled,
            formatLevel,
            toggles.notesModeEnabled,
            toggles.startAssistive
        )
    }

    func setQuickToggle(_ toggle: TrayQuickToggle, enabled: Bool) {
        try? config.updateConfig(key: toggle.configKey, value: enabled ? "1" : "0")
    }

    func setAutoPasteEnabled(_ enabled: Bool) {
        _ = try? config.setAutoPasteEnabled(enabled: enabled)
    }

    func setAutoFormatLevel(_ level: FormattingPolicyOption) {
        _ = try? config.setAutoFormatLevel(level: level.rawValue)
    }

    func setNotesMode(_ enabled: Bool) -> Bool {
        // One atomic two-key write (both flags together): a failure leaves the
        // config unchanged rather than half-set. Returns false on failure so the
        // UI can avoid faking success.
        (try? config.setNotesMode(enabled: enabled)) != nil
    }

    func setStartInAssistive(_ enabled: Bool) -> Bool {
        (try? config.updateConfig(
            key: "TRAY_START_ASSISTIVE",
            value: enabled ? "1" : "0"
        )) != nil
    }

    func latestHistoryPath() -> String? {
        // Skip failure / no-speech markers: "copy / save last transcript" must land
        // on the newest entry that actually carries copyable text, not a "failed"
        // placeholder (mirrors Rust's `TranscriptKind::is_copyable_transcript`).
        threads.recentHistory(limit: 32).first { $0.kind.isCopyableTranscript }?.path
    }

    func latestTranscriptText() -> String? {
        guard let path = latestHistoryPath() else { return nil }
        return try? threads.readHistoryText(path: path)
    }

    func recentTranscripts(limit: Int) -> [TrayTranscript] {
        threads.recentHistory(limit: UInt32(limit)).map { entry in
            TrayTranscript(path: entry.path, title: Self.historyTitle(entry))
        }
    }

    func transcriptText(forPath path: String) -> String? {
        try? threads.readHistoryText(path: path)
    }

    /// "HH:mm · <first words>" label for a history entry; falls back to the file
    /// name when the preview is empty.
    private static func historyTitle(_ entry: CsHistoryEntry) -> String {
        let date = Date(timeIntervalSince1970: TimeInterval(entry.timestampMs) / 1000)
        let time = timeFormatter.string(from: date)
        let preview = entry.preview.trimmingCharacters(in: .whitespacesAndNewlines)
        let snippet = preview.isEmpty
            ? (entry.path as NSString).lastPathComponent
            : String(preview.prefix(32))
        return "\(time) · \(snippet)"
    }

    private static let timeFormatter: DateFormatter = {
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm"
        return formatter
    }()
}

extension CsTranscriptKind {
    /// Mirrors `TranscriptKind::is_copyable_transcript` (core/state/history.rs):
    /// true for entries that carry copyable transcript text, false for the
    /// assistant interpretation and the failure / no-speech marker.
    var isCopyableTranscript: Bool {
        switch self {
        case .raw, .cloud, .formattedTranscript, .formattingFailed:
            return true
        case .assistantInterpretation, .failed:
            return false
        }
    }
}
