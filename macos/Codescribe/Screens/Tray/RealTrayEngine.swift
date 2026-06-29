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

    func startRecording() async throws {
        try await hotkeys.startRecording()
    }

    func stopRecording() async throws {
        try await hotkeys.stopRecording()
    }

    func currentToggles() -> (showDockIcon: Bool, overlayEnabled: Bool)? {
        let toggles = config.trayToggles()
        return (toggles.showDockIcon, toggles.transcriptionOverlayEnabled)
    }

    func setQuickToggle(_ toggle: TrayQuickToggle, enabled: Bool) {
        try? config.updateConfig(key: toggle.configKey, value: enabled ? "1" : "0")
    }

    func latestHistoryPath() -> String? {
        threads.recentHistory(limit: 1).first?.path
    }

    func latestTranscriptText() -> String? {
        guard let path = latestHistoryPath() else { return nil }
        return try? threads.readHistoryText(path: path)
    }
}
