import XCTest
@testable import Codescribe

@MainActor
final class TrayViewModelTests: XCTestCase {
    func testRefreshStatusReadsEntirePersistedTraySnapshot() {
        let engine = TrackingTrayEngine(
            showDockIcon: true,
            overlayEnabled: false,
            autoPasteEnabled: false,
            autoFormatLevel: .smart,
            notesMode: false,
            startInAssistive: true
        )
        let model = TrayViewModel(engine: engine)
        model.showDockIcon = false
        model.overlayEnabled = true
        model.autoPasteEnabled = true
        model.autoFormatLevel = .off
        model.notesModeEnabled = true
        model.startInAssistive = false

        model.refreshStatus()

        XCTAssertTrue(model.showDockIcon)
        XCTAssertFalse(model.overlayEnabled)
        XCTAssertFalse(model.autoPasteEnabled)
        XCTAssertEqual(model.autoFormatLevel, .smart)
        XCTAssertFalse(model.notesModeEnabled)
        XCTAssertTrue(model.startInAssistive)
        XCTAssertEqual(engine.currentToggleReads, 1)
    }

    func testOverlayToggleResyncsToPersistedSettingsTruth() {
        let engine = TrackingTrayEngine(
            showDockIcon: true,
            overlayEnabled: false,
            autoPasteEnabled: true,
            autoFormatLevel: .correction,
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

    func testAutoPasteWriteReconcilesSuccessAndFailureToPersistedTruth() {
        for persists in [true, false] {
            let engine = TrackingTrayEngine(
                showDockIcon: true,
                overlayEnabled: true,
                autoPasteEnabled: false,
                autoFormatLevel: .correction,
                notesMode: false,
                startInAssistive: false
            )
            engine.persistAutoPasteWrites = persists
            let model = TrayViewModel(engine: engine)
            model.refreshStatus()

            model.setAutoPasteEnabled(true)

            XCTAssertEqual(model.autoPasteEnabled, persists)
            XCTAssertEqual(engine.autoPasteWrites, [true])
            XCTAssertEqual(engine.currentToggleReads, 2)
        }
    }

    func testAutoFormatWritesEveryNormalizedLevelAndReconcilesSuccess() {
        for level in FormattingPolicyOption.allCases {
            let engine = TrackingTrayEngine(
                showDockIcon: true,
                overlayEnabled: true,
                autoPasteEnabled: true,
                autoFormatLevel: level == .off ? .max : .off,
                notesMode: false,
                startInAssistive: false
            )
            let model = TrayViewModel(engine: engine)
            model.refreshStatus()

            model.setAutoFormatLevel(level)

            XCTAssertEqual(model.autoFormatLevel, level)
            XCTAssertEqual(engine.autoFormatWrites, [level.rawValue])
            XCTAssertEqual(engine.currentToggleReads, 2)
        }
    }

    func testAutoFormatRejectedWritesKeepPersistedTruthForEveryLevel() {
        for level in FormattingPolicyOption.allCases {
            let persisted: FormattingPolicyOption = level == .off ? .max : .off
            let engine = TrackingTrayEngine(
                showDockIcon: true,
                overlayEnabled: true,
                autoPasteEnabled: true,
                autoFormatLevel: persisted,
                notesMode: false,
                startInAssistive: false
            )
            engine.persistAutoFormatWrites = false
            let model = TrayViewModel(engine: engine)
            model.refreshStatus()

            model.setAutoFormatLevel(level)

            XCTAssertEqual(model.autoFormatLevel, persisted)
            XCTAssertEqual(engine.autoFormatWrites, [level.rawValue])
            XCTAssertEqual(engine.currentToggleReads, 2)
        }
    }

    func testAutoFormatPresentationHasFourExplicitAccessibleNames() {
        XCTAssertEqual(
            FormattingPolicyOption.allCases.map(\.rawValue),
            ["off", "correction", "smart", "max"]
        )
        XCTAssertEqual(
            FormattingPolicyOption.allCases.map(\.visibleName),
            ["Off", "Correction", "Smart", "Max"]
        )
    }
}

private final class TrackingTrayEngine: TrayEngine {
    var recording = false
    var agentAvailable = true
    var showDockIcon: Bool
    var overlayEnabled: Bool
    var autoPasteEnabled: Bool
    var autoFormatLevel: FormattingPolicyOption
    var notesMode: Bool
    var startInAssistive: Bool
    var persistOverlayWrites = true
    var persistAutoPasteWrites = true
    var persistAutoFormatWrites = true
    private(set) var currentToggleReads = 0
    private(set) var quickToggleWrites: [TrayQuickToggle] = []
    private(set) var autoPasteWrites: [Bool] = []
    private(set) var autoFormatWrites: [String] = []

    init(
        showDockIcon: Bool,
        overlayEnabled: Bool,
        autoPasteEnabled: Bool,
        autoFormatLevel: FormattingPolicyOption,
        notesMode: Bool,
        startInAssistive: Bool
    ) {
        self.showDockIcon = showDockIcon
        self.overlayEnabled = overlayEnabled
        self.autoPasteEnabled = autoPasteEnabled
        self.autoFormatLevel = autoFormatLevel
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
        autoPasteEnabled: Bool,
        autoFormatLevel: FormattingPolicyOption,
        notesMode: Bool,
        startInAssistive: Bool
    )? {
        currentToggleReads += 1
        return (
            showDockIcon,
            overlayEnabled,
            autoPasteEnabled,
            autoFormatLevel,
            notesMode,
            startInAssistive
        )
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

    func setAutoPasteEnabled(_ enabled: Bool) {
        autoPasteWrites.append(enabled)
        if persistAutoPasteWrites {
            autoPasteEnabled = enabled
        }
    }

    func setAutoFormatLevel(_ level: FormattingPolicyOption) {
        autoFormatWrites.append(level.rawValue)
        if persistAutoFormatWrites {
            autoFormatLevel = level
        }
    }

    func setNotesMode(_ enabled: Bool) -> Bool { notesMode = enabled; return true }
    func setStartInAssistive(_ enabled: Bool) -> Bool { startInAssistive = enabled; return true }
    func latestHistoryPath() -> String? { nil }
    func latestTranscriptText() -> String? { nil }
    func recentTranscripts(limit: Int) -> [TrayTranscript] { [] }
    func transcriptText(forPath path: String) -> String? { nil }
}
