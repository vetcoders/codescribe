import AppKit
import XCTest
@testable import Codescribe

@MainActor
final class TrayViewModelTests: XCTestCase {
    func testHoldBadgeCyclePostsConfigBusForSettingsSync() {
        let engine = TrackingTrayEngine(
            showDockIcon: true,
            overlayEnabled: true,
            autoPasteEnabled: true,
            autoFormatLevel: .correction,
            notesMode: false,
            startInAssistive: false,
            holdBadgeOption: .eight
        )
        let model = TrayViewModel(engine: engine)
        model.refreshStatus()
        XCTAssertEqual(model.holdBadgeOption, .eight)

        let exp = expectation(description: "hold badge bus fire")
        let token = NotificationCenter.default.addObserver(
            forName: ConfigChangeBus.holdBadgeDidChange,
            object: nil,
            queue: .main
        ) { _ in exp.fulfill() }
        defer { NotificationCenter.default.removeObserver(token) }

        model.setHoldBadgeOption(.four)
        wait(for: [exp], timeout: 1.0)
        XCTAssertEqual(model.holdBadgeOption, .four)
        XCTAssertEqual(engine.holdBadgeWrites.last, .four)
    }

    func testHoldBadgeChangesSynchronizeTrayAndSettingsInBothDirections() {
        var persisted = CsSettings.sample
        persisted.holdIndicator = true
        persisted.holdBadgeSize = 8

        let trayEngine = TrackingTrayEngine(
            showDockIcon: true,
            overlayEnabled: true,
            autoPasteEnabled: true,
            autoFormatLevel: .correction,
            notesMode: false,
            startInAssistive: false,
            holdBadgeOption: .eight
        )
        trayEngine.holdBadgeReader = {
            HoldBadgeOption(
                indicatorEnabled: persisted.holdIndicator,
                size: persisted.holdBadgeSize
            )
        }
        trayEngine.holdBadgeWriteObserver = { option in
            persisted.holdIndicator = option != .off
            if let size = option.size { persisted.holdBadgeSize = size }
        }

        let settingsEngine = MockSettingsEngine(
            settingsLoader: { persisted },
            updateConfigManyObserver: { entries in
                for entry in entries {
                    if entry.key == "HOLD_INDICATOR" {
                        persisted.holdIndicator = entry.value == "1"
                    } else if entry.key == "HOLD_BADGE_SIZE", let size = UInt32(entry.value) {
                        persisted.holdBadgeSize = size
                    }
                }
            },
            updateConfigObserver: { key, value in
                if key == "HOLD_INDICATOR" { persisted.holdIndicator = value == "1" }
            }
        )
        let tray = TrayViewModel(engine: trayEngine)
        let settings = SettingsViewModel(engine: settingsEngine)
        tray.refreshStatus()
        settings.refresh()

        tray.setHoldBadgeOption(.four)
        XCTAssertEqual(settings.holdBadgeOption, .four, "tray write must refresh Settings")

        settings.setHoldBadgeOption(.twelve)
        XCTAssertEqual(tray.holdBadgeOption, .twelve, "Settings write must refresh tray")
    }

    /// The tray's Auto Format row cycles the full wheel: Off → Correction →
    /// Smart → Max → back to Off. One canonical order, no dead ends.
    func testAutoFormatLevelCyclesFullWheel() {
        XCTAssertEqual(FormattingPolicyOption.off.next, .correction)
        XCTAssertEqual(FormattingPolicyOption.correction.next, .smart)
        XCTAssertEqual(FormattingPolicyOption.smart.next, .max)
        XCTAssertEqual(FormattingPolicyOption.max.next, .off)
    }

    func testHoldBadgeRollingRowCyclesFiveObservedStatesBackToStart() {
        let engine = TrackingTrayEngine(
            showDockIcon: true,
            overlayEnabled: true,
            autoPasteEnabled: true,
            autoFormatLevel: .correction,
            notesMode: false,
            startInAssistive: false,
            holdBadgeOption: .off
        )
        let model = TrayViewModel(engine: engine)
        model.refreshStatus()

        var observed = [model.holdBadgeOption]
        for _ in 0..<4 {
            model.setHoldBadgeOption(model.holdBadgeOption.next)
            observed.append(model.holdBadgeOption)
        }

        XCTAssertEqual(observed, [.off, .four, .eight, .twelve, .off])
        XCTAssertEqual(engine.holdBadgeWrites, [.four, .eight, .twelve, .off])
    }

    func testDisclosureChevronUsesOneRightGlyphRotatedDownWhenExpanded() {
        switch TrayDisclosureChevron.icon {
        case .chevronRight:
            break
        default:
            XCTFail("Expandable tray rows must use the shared chevron.right glyph")
        }
        XCTAssertEqual(TrayDisclosureChevron.rotationDegrees(expanded: false), 0)
        XCTAssertEqual(TrayDisclosureChevron.rotationDegrees(expanded: true), 90)
    }

    func testStatusDotCompositesInsideGlyphBottomRightCorner() throws {
        let size = NSSize(width: 20, height: 20)
        let bounds = NSRect(origin: .zero, size: size)
        let dotRect = TrayStatusDotIcon.dotRect(in: bounds)

        XCTAssertEqual(dotRect.maxX, bounds.maxX, accuracy: 0.001)
        XCTAssertEqual(dotRect.minY, bounds.minY, accuracy: 0.001)
        XCTAssertTrue(bounds.contains(dotRect))

        let base = NSImage(size: size, flipped: false) { rect in
            NSColor.white.setFill()
            rect.fill()
            return true
        }
        base.isTemplate = true
        let dot = NSColor(srgbRed: 1, green: 0, blue: 1, alpha: 1)
        let composite = TrayStatusDotIcon.composite(base: base, dot: dot)

        XCTAssertEqual(composite.size, size)
        XCTAssertFalse(composite.isTemplate)
        let representation = try XCTUnwrap(composite.tiffRepresentation)
        let bitmap = try XCTUnwrap(NSBitmapImageRep(data: representation))
        let center = CGPoint(x: dotRect.midX, y: dotRect.midY)
        // AppKit drawing uses a bottom-left origin; NSBitmapImageRep indexes
        // the TIFF rows top-down, so mirror the y-coordinate for sampling.
        let bitmapY = bitmap.pixelsHigh - 1 - Int(center.y)
        let sampledColor = try XCTUnwrap(bitmap.colorAt(x: Int(center.x), y: bitmapY))
        let sampled = try XCTUnwrap(sampledColor.usingColorSpace(.sRGB))
        XCTAssertGreaterThan(sampled.redComponent, 0.9)
        XCTAssertLessThan(sampled.greenComponent, 0.4)
        XCTAssertGreaterThan(sampled.blueComponent, 0.9)
        XCTAssertGreaterThan(sampled.alphaComponent, 0.95)
    }

    func testTrayStatusFeedMapsReadyRecordingProcessingAndAgentColorsOneToOne() throws {
        let ready = TrayStatusStore.preview(kind: .idle, tone: .neutral)
        try assertDotColor(
            ready,
            equals: NSColor(srgbRed: 157.0 / 255.0, green: 177.0 / 255.0, blue: 120.0 / 255.0, alpha: 1)
        )

        let recording = TrayStatusStore.preview(kind: .listening, tone: .active)
        try assertDotColor(
            recording,
            equals: NSColor(srgbRed: 1, green: 59.0 / 255.0, blue: 48.0 / 255.0, alpha: 1)
        )

        let processing = TrayStatusStore.preview(kind: .processing, tone: .active)
        try assertDotColor(
            processing,
            equals: NSColor(srgbRed: 242.0 / 255.0, green: 140.0 / 255.0, blue: 69.0 / 255.0, alpha: 1)
        )

        let agent = TrayStatusStore.preview(
            kind: .listening,
            tone: .active,
            indicatorMode: .assistive,
            assistive: true
        )
        try assertDotColor(
            agent,
            equals: NSColor(srgbRed: 155.0 / 255.0, green: 114.0 / 255.0, blue: 242.0 / 255.0, alpha: 1)
        )

        let agentProcessing = TrayStatusStore.preview(
            kind: .processing,
            tone: .active,
            indicatorMode: .processing,
            assistive: false
        )
        try assertDotColor(
            agentProcessing,
            equals: NSColor(srgbRed: 242.0 / 255.0, green: 140.0 / 255.0, blue: 69.0 / 255.0, alpha: 1)
        )
    }

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

    private func assertDotColor(
        _ store: TrayStatusStore,
        equals expected: NSColor,
        file: StaticString = #filePath,
        line: UInt = #line
    ) throws {
        let color = try XCTUnwrap(store.menuBarDotColor, file: file, line: line)
        let resolved = try XCTUnwrap(NSColor(color).usingColorSpace(.sRGB), file: file, line: line)
        assertColor(resolved, equals: expected, file: file, line: line)
    }

    private func assertColor(
        _ actual: NSColor,
        equals expected: NSColor,
        accuracy: CGFloat = 0.005,
        file: StaticString = #filePath,
        line: UInt = #line
    ) {
        XCTAssertEqual(actual.redComponent, expected.redComponent, accuracy: accuracy, file: file, line: line)
        XCTAssertEqual(actual.greenComponent, expected.greenComponent, accuracy: accuracy, file: file, line: line)
        XCTAssertEqual(actual.blueComponent, expected.blueComponent, accuracy: accuracy, file: file, line: line)
        XCTAssertEqual(actual.alphaComponent, expected.alphaComponent, accuracy: accuracy, file: file, line: line)
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
    var holdBadgeOption: HoldBadgeOption
    var persistOverlayWrites = true
    var persistAutoPasteWrites = true
    var persistAutoFormatWrites = true
    private(set) var currentToggleReads = 0
    private(set) var quickToggleWrites: [TrayQuickToggle] = []
    private(set) var autoPasteWrites: [Bool] = []
    private(set) var autoFormatWrites: [String] = []
    private(set) var holdBadgeWrites: [HoldBadgeOption] = []
    var holdBadgeReader: (() -> HoldBadgeOption)?
    var holdBadgeWriteObserver: ((HoldBadgeOption) -> Void)?

    init(
        showDockIcon: Bool,
        overlayEnabled: Bool,
        autoPasteEnabled: Bool,
        autoFormatLevel: FormattingPolicyOption,
        notesMode: Bool,
        startInAssistive: Bool,
        holdBadgeOption: HoldBadgeOption = .twelve
    ) {
        self.showDockIcon = showDockIcon
        self.overlayEnabled = overlayEnabled
        self.autoPasteEnabled = autoPasteEnabled
        self.autoFormatLevel = autoFormatLevel
        self.notesMode = notesMode
        self.startInAssistive = startInAssistive
        self.holdBadgeOption = holdBadgeOption
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
        startInAssistive: Bool,
        holdBadgeOption: HoldBadgeOption
    )? {
        currentToggleReads += 1
        return (
            showDockIcon,
            overlayEnabled,
            autoPasteEnabled,
            autoFormatLevel,
            notesMode,
            startInAssistive,
            holdBadgeReader?() ?? holdBadgeOption
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

    func setHoldBadgeOption(_ option: HoldBadgeOption) -> Bool {
        holdBadgeWrites.append(option)
        holdBadgeOption = option
        holdBadgeWriteObserver?(option)
        return true
    }

    func setNotesMode(_ enabled: Bool) -> Bool { notesMode = enabled; return true }
    func setStartInAssistive(_ enabled: Bool) -> Bool { startInAssistive = enabled; return true }
    func latestHistoryPath() -> String? { nil }
    func latestTranscriptText() -> String? { nil }
    func recentTranscripts(limit: Int) -> [TrayTranscript] { [] }
    func transcriptText(forPath path: String) -> String? { nil }
}
