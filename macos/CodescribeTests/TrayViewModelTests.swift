import XCTest
@testable import Codescribe

@MainActor
final class TrayViewModelTests: XCTestCase {
    func testRefreshStatusReadsFreshOverlayTruthFromEngine() {
        let engine = TrackingTrayEngine(
            showDockIcon: true,
            overlayEnabled: false,
            notesMode: false,
            startInAssistive: false
        )
        let model = TrayViewModel(engine: engine)
        model.overlayEnabled = true

        model.refreshStatus()

        XCTAssertFalse(model.overlayEnabled)
        XCTAssertEqual(engine.currentToggleReads, 1)
    }

    func testOverlayToggleResyncsToPersistedSettingsTruth() {
        let engine = TrackingTrayEngine(
            showDockIcon: true,
            overlayEnabled: false,
            notesMode: false,
            startInAssistive: false
        )
        engine.persistOverlayWrites = false
        let model = TrayViewModel(engine: engine)
        model.refreshStatus()

        model.setOverlayEnabled(true)

        XCTAssertFalse(model.overlayEnabled)
        XCTAssertEqual(engine.quickToggleWrites, [.transcriptionOverlay])
        XCTAssertEqual(engine.currentToggleReads, 2)
    }
}

private final class TrackingTrayEngine: TrayEngine {
    var recording = false
    var agentAvailable = true
    var showDockIcon: Bool
    var overlayEnabled: Bool
    var notesMode: Bool
    var startInAssistive: Bool
    var persistOverlayWrites = true
    private(set) var currentToggleReads = 0
    private(set) var quickToggleWrites: [TrayQuickToggle] = []

    init(showDockIcon: Bool, overlayEnabled: Bool, notesMode: Bool, startInAssistive: Bool) {
        self.showDockIcon = showDockIcon
        self.overlayEnabled = overlayEnabled
        self.notesMode = notesMode
        self.startInAssistive = startInAssistive
    }

    func isAgentAvailable() -> Bool { agentAvailable }
    func isRecording() async -> Bool { recording }
    func startRecording(assistive: Bool) async throws { recording = true }
    func stopRecording() async throws { recording = false }

    func currentToggles() -> (
        showDockIcon: Bool,
        overlayEnabled: Bool,
        notesMode: Bool,
        startInAssistive: Bool
    )? {
        currentToggleReads += 1
        return (showDockIcon, overlayEnabled, notesMode, startInAssistive)
    }

    func setQuickToggle(_ toggle: TrayQuickToggle, enabled: Bool) {
        quickToggleWrites.append(toggle)
        switch toggle {
        case .showDockIcon:
            showDockIcon = enabled
        case .transcriptionOverlay:
            if persistOverlayWrites {
                overlayEnabled = enabled
            }
        }
    }

    func setNotesMode(_ enabled: Bool) -> Bool { notesMode = enabled; return true }
    func setStartInAssistive(_ enabled: Bool) -> Bool { startInAssistive = enabled; return true }
    func latestHistoryPath() -> String? { nil }
    func latestTranscriptText() -> String? { nil }
    func recentTranscripts(limit: Int) -> [TrayTranscript] { [] }
    func transcriptText(forPath path: String) -> String? { nil }
}
