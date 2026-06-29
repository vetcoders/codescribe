import Foundation

// Composite seam between the tray view-model and the codescribe core via the
// UniFFI bridge. The tray needs three slices of the engine — agent readiness,
// dictation control, and a couple of quick config toggles — plus read access to
// the most recent transcript. The concrete `RealTrayEngine` (see its own file)
// wraps CodescribeAgent / CodescribeDictation / CodescribeConfig /
// CodescribeThreads; `MockTrayEngine` keeps `#Preview` self-contained.

/// Navigation intents the tray emits. App.swift binds each one to the action
/// that actually opens the relevant window / scene / panel.
enum TrayIntent {
    case openChat      // bring up the Agent Chat window
    case openSettings  // open the Settings window
    case openOverlay   // show the transcription overlay panel
}

/// The two fast config toggles surfaced in the tray, mapped to the core's
/// router env keys consumed by `CodescribeConfig.updateConfig(key:value:)`.
enum TrayQuickToggle {
    case showDockIcon
    case transcriptionOverlay

    var configKey: String {
        switch self {
        case .showDockIcon:        return "SHOW_DOCK_ICON"
        case .transcriptionOverlay: return "TRANSCRIPTION_OVERLAY_ENABLED"
        }
    }
}

protocol TrayEngine: AnyObject {
    /// True when the assistive LLM provider can be built (gates "Show Agent").
    func isAgentAvailable() -> Bool

    /// Live dictation state. Async because the core reads it behind its mutex.
    func isRecording() async -> Bool
    func startRecording() async throws
    func stopRecording() async throws

    /// Current values for the tray's quick toggles, read from on-disk settings.
    /// `nil` when settings cannot be loaded.
    func currentToggles() -> (showDockIcon: Bool, overlayEnabled: Bool)?
    func setQuickToggle(_ toggle: TrayQuickToggle, enabled: Bool)

    /// Path of the most recent transcript artifact, or `nil` when none exist.
    func latestHistoryPath() -> String?
    /// Full text of the most recent transcript, or `nil` when unavailable.
    func latestTranscriptText() -> String?
}

// Standalone seed so the `#Preview` renders without the real core.
final class MockTrayEngine: TrayEngine {
    var recording: Bool
    var agentAvailable: Bool
    var showDockIcon: Bool
    var overlayEnabled: Bool
    var historyPath: String
    var transcriptText: String

    init(recording: Bool = false,
         agentAvailable: Bool = true,
         showDockIcon: Bool = true,
         overlayEnabled: Bool = false,
         historyPath: String = "/tmp/codescribe/history/2026-06-28-1422.md",
         transcriptText: String = "Sample transcript.") {
        self.recording = recording
        self.agentAvailable = agentAvailable
        self.showDockIcon = showDockIcon
        self.overlayEnabled = overlayEnabled
        self.historyPath = historyPath
        self.transcriptText = transcriptText
    }

    func isAgentAvailable() -> Bool { agentAvailable }

    func isRecording() async -> Bool { recording }
    func startRecording() async throws { recording = true }
    func stopRecording() async throws { recording = false }

    func currentToggles() -> (showDockIcon: Bool, overlayEnabled: Bool)? {
        (showDockIcon, overlayEnabled)
    }

    func setQuickToggle(_ toggle: TrayQuickToggle, enabled: Bool) {
        switch toggle {
        case .showDockIcon:        showDockIcon = enabled
        case .transcriptionOverlay: overlayEnabled = enabled
        }
    }

    func latestHistoryPath() -> String? { historyPath }
    func latestTranscriptText() -> String? { transcriptText }
}
