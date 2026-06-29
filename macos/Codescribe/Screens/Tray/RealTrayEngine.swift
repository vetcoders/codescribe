import Foundation

// Backs the tray with the REAL codescribe core via the UniFFI bridge. It is a
// composite over four thin handles:
//   • CodescribeAgent    — assistive-provider readiness gate.
//   • CodescribeDictation — start/stop/state of the streaming recorder.
//   • CodescribeConfig    — quick config toggles (settings.json / .env router).
//   • CodescribeThreads   — most-recent transcript path + text.
//
// The dictation handle is injectable: `start_recording` requires a registered
// CsTranscriptionListener, which the Overlay screen owns. App.swift should pass
// the SAME CodescribeDictation instance the overlay registered its listener on
// so the tray toggle drives one shared session (see the wiring report).
final class RealTrayEngine: TrayEngine {
    private let agent: CodescribeAgent
    private let dictation: CodescribeDictation
    private let config: CodescribeConfig
    private let threads: CodescribeThreads

    init(
        agent: CodescribeAgent = CodescribeAgent(),
        dictation: CodescribeDictation = CodescribeDictation(),
        config: CodescribeConfig = CodescribeConfig(),
        threads: CodescribeThreads = CodescribeThreads()
    ) {
        self.agent = agent
        self.dictation = dictation
        self.config = config
        self.threads = threads
    }

    func isAgentAvailable() -> Bool { agent.isAvailable() }

    func isRecording() async -> Bool { await dictation.isRecording() }

    func startRecording() async throws {
        // `nil` language → core auto-detects (CsLanguage.polish / .english).
        try await dictation.startRecording(language: nil)
    }

    func stopRecording() async throws {
        _ = try await dictation.stopRecording()
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
