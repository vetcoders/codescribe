import AppKit
import XCTest

@testable import Codescribe

@MainActor
final class TrayViewModelTests: XCTestCase {
  func testHoldBadgeCyclePersistsAndUpdatesTrayState() {
    let engine = TrackingTrayEngine(
      showDockIcon: true,
      overlayEnabled: true,
      pasteMode: .safe,
      autoFormatLevel: .correction,
      notesMode: false,
      startInAssistive: false,
      holdBadgeOption: .eight
    )
    let model = TrayViewModel(engine: engine)
    model.refreshStatus()
    XCTAssertEqual(model.holdBadgeOption, .eight)

    model.setHoldBadgeOption(.four)
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
      pasteMode: .safe,
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
    settings.refresh()
    XCTAssertEqual(settings.holdBadgeOption, .four, "Settings must re-read persisted tray truth")

    settings.setHoldBadgeOption(.twelve)
    tray.refreshStatus()
    XCTAssertEqual(tray.holdBadgeOption, .twelve, "tray must re-read persisted Settings truth")
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
      pasteMode: .safe,
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

  /// Cold-open tray must not dump Notes / Quick settings / History open —
  /// product grew a wall of rows; default is collapsed-on-demand.
  func testTrayDisclosuresDefaultCollapsed() {
    let model = TrayViewModel()
    XCTAssertFalse(model.notesExpanded)
    XCTAssertFalse(model.diagnosticsExpanded)
    XCTAssertFalse(model.historyExpanded)
    XCTAssertFalse(model.quickSettingsExpanded)
  }

  func testTrayDisclosuresCollapseWhenPopoverCloses() {
    let model = TrayViewModel()
    model.notesExpanded = true
    model.diagnosticsExpanded = true
    model.historyExpanded = true
    model.quickSettingsExpanded = true

    model.collapseDisclosures()

    XCTAssertFalse(model.notesExpanded)
    XCTAssertFalse(model.diagnosticsExpanded)
    XCTAssertFalse(model.historyExpanded)
    XCTAssertFalse(model.quickSettingsExpanded)
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
    let cases: [(TrayStatusStore, UInt32, UInt32)] = [
      (.preview(kind: .idle, tone: .neutral), 0x55663A, 0x9DB178),
      (.preview(kind: .listening, tone: .active), 0xFF3B30, 0xFF3B30),
      (.preview(kind: .processing, tone: .active), 0xB96A24, 0xF28C45),
      (.preview(kind: .listening, tone: .active, indicatorMode: .assistive, assistive: true),
       0x9B72F2, 0x9B72F2),
      (.preview(kind: .processing, tone: .active, indicatorMode: .processing, assistive: false),
       0xB96A24, 0xF28C45),
    ]
    for (store, light, dark) in cases {
      for (name, expected) in [(NSAppearance.Name.aqua, light), (.darkAqua, dark)] {
        let appearance = try XCTUnwrap(NSAppearance(named: name))
        var resolved: NSColor?
        appearance.performAsCurrentDrawingAppearance {
          resolved = store.menuBarDotColor.map { NSColor($0).usingColorSpace(.sRGB) } ?? nil
        }
        assertColor(try XCTUnwrap(resolved), equals: NSColor(hex: expected))
      }
    }
  }

  /// The header pill already paints idle / success / listening / processing.
  /// A second "Status: Idle" row is the duplicate the operator saw; keep the
  /// extra row only for warning / critical kinds that need an attention banner.
  func testDetailStatusRowShowsOnlyForWarningAndCriticalKinds() {
    let cases: [(CsTrayStatusKind, Bool)] = [
      (.starting, false),
      (.idle, false),
      (.listening, false),
      (.processing, false),
      (.success, false),
      (.error, true),
      (.thermal, true),
      (.hotkeyConflict, true),
    ]
    for (kind, expected) in cases {
      XCTAssertEqual(
        TrayStatusStore.preview(kind: kind).showsDetailStatusRow,
        expected,
        "showsDetailStatusRow for \(String(describing: kind))"
      )
    }

    let idle = TrayStatusStore.preview(kind: .idle, label: "Status: Idle")
    XCTAssertEqual(idle.compactLabel, "Idle")
    XCTAssertFalse(idle.showsDetailStatusRow)
  }

  func testRefreshStatusReadsEntirePersistedTraySnapshot() {
    let engine = TrackingTrayEngine(
      showDockIcon: true,
      overlayEnabled: false,
      pasteMode: .comfort,
      autoFormatLevel: .smart,
      notesMode: false,
      startInAssistive: true
    )
    let model = TrayViewModel(engine: engine)
    model.showDockIcon = false
    model.overlayEnabled = true
    model.pasteMode = .off
    model.autoFormatLevel = .off
    model.notesModeEnabled = true
    model.startInAssistive = false

    model.refreshStatus()

    XCTAssertTrue(model.showDockIcon)
    XCTAssertFalse(model.overlayEnabled)
    XCTAssertEqual(model.pasteMode, .comfort)
    XCTAssertEqual(model.autoFormatLevel, .smart)
    XCTAssertFalse(model.notesModeEnabled)
    XCTAssertTrue(model.startInAssistive)
    XCTAssertEqual(engine.currentToggleReads, 1)
  }

  func testOverlayToggleResyncsToPersistedSettingsTruth() {
    let engine = TrackingTrayEngine(
      showDockIcon: true,
      overlayEnabled: false,
      pasteMode: .safe,
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

  func testPasteModeWriteReconcilesSuccessAndFailureToPersistedTruth() {
    for persists in [true, false] {
      let engine = TrackingTrayEngine(
        showDockIcon: true,
        overlayEnabled: true,
        pasteMode: .off,
        autoFormatLevel: .correction,
        notesMode: false,
        startInAssistive: false
      )
      engine.persistPasteModeWrites = persists
      let model = TrayViewModel(engine: engine)
      model.refreshStatus()

      model.setPasteMode(.comfort)

      XCTAssertEqual(model.pasteMode, persists ? .comfort : .off)
      XCTAssertEqual(engine.pasteModeWrites, [.comfort])
      XCTAssertEqual(engine.currentToggleReads, 2)
    }
  }

  /// The Quick settings row cycles Safe → Comfort → Off → Safe, one persisted
  /// write per click, each re-read from the engine.
  func testPasteModeRowCyclesThreeModesBackToStart() {
    let engine = TrackingTrayEngine(
      showDockIcon: true,
      overlayEnabled: true,
      pasteMode: .safe,
      autoFormatLevel: .correction,
      notesMode: false,
      startInAssistive: false
    )
    let model = TrayViewModel(engine: engine)
    model.refreshStatus()

    var observed: [CsPasteMode] = [model.pasteMode]
    for _ in 0..<3 {
      model.setPasteMode(model.pasteMode.next)
      observed.append(model.pasteMode)
    }

    XCTAssertEqual(observed, [.safe, .comfort, .off, .safe])
    XCTAssertEqual(engine.pasteModeWrites, [.comfort, .off, .safe])
    XCTAssertEqual(CsPasteMode.allModes.map(\.visibleName), ["Safe", "Comfort", "Off"])
  }

  func testAutoFormatWritesEveryNormalizedLevelAndReconcilesSuccess() {
    for level in FormattingPolicyOption.allCases {
      let engine = TrackingTrayEngine(
        showDockIcon: true,
        overlayEnabled: true,
        pasteMode: .safe,
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
        pasteMode: .safe,
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

  private func assertColor(
    _ actual: NSColor,
    equals expected: NSColor,
    accuracy: CGFloat = 0.005,
    file: StaticString = #filePath,
    line: UInt = #line
  ) {
    XCTAssertEqual(
      actual.redComponent, expected.redComponent, accuracy: accuracy, file: file, line: line)
    XCTAssertEqual(
      actual.greenComponent, expected.greenComponent, accuracy: accuracy, file: file, line: line)
    XCTAssertEqual(
      actual.blueComponent, expected.blueComponent, accuracy: accuracy, file: file, line: line)
    XCTAssertEqual(
      actual.alphaComponent, expected.alphaComponent, accuracy: accuracy, file: file, line: line)
  }
}

private final class TrackingTrayEngine: TrayEngine {
  var recording = false
  var agentAvailable = true
  var showDockIcon: Bool
  var overlayEnabled: Bool
  var pasteMode: CsPasteMode
  var autoFormatLevel: FormattingPolicyOption
  var notesMode: Bool
  var startInAssistive: Bool
  var holdBadgeOption: HoldBadgeOption
  var persistOverlayWrites = true
  var persistPasteModeWrites = true
  var persistAutoFormatWrites = true
  private(set) var currentToggleReads = 0
  private(set) var quickToggleWrites: [TrayQuickToggle] = []
  private(set) var pasteModeWrites: [CsPasteMode] = []
  private(set) var autoFormatWrites: [String] = []
  private(set) var holdBadgeWrites: [HoldBadgeOption] = []
  var holdBadgeReader: (() -> HoldBadgeOption)?
  var holdBadgeWriteObserver: ((HoldBadgeOption) -> Void)?

  init(
    showDockIcon: Bool,
    overlayEnabled: Bool,
    pasteMode: CsPasteMode,
    autoFormatLevel: FormattingPolicyOption,
    notesMode: Bool,
    startInAssistive: Bool,
    holdBadgeOption: HoldBadgeOption = .twelve
  ) {
    self.showDockIcon = showDockIcon
    self.overlayEnabled = overlayEnabled
    self.pasteMode = pasteMode
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
    pasteMode: CsPasteMode,
    autoFormatLevel: FormattingPolicyOption,
    notesMode: Bool,
    startInAssistive: Bool,
    holdBadgeOption: HoldBadgeOption
  )? {
    currentToggleReads += 1
    return (
      showDockIcon,
      overlayEnabled,
      pasteMode,
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

  func setPasteMode(_ mode: CsPasteMode) {
    pasteModeWrites.append(mode)
    if persistPasteModeWrites {
      pasteMode = mode
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

  func setNotesMode(_ enabled: Bool) -> Bool {
    notesMode = enabled
    return true
  }
  func setStartInAssistive(_ enabled: Bool) -> Bool {
    startInAssistive = enabled
    return true
  }
  func latestHistoryPath() -> String? { nil }
  func latestTranscriptText() -> String? { nil }
  func recentTranscripts(limit: Int) -> [TrayTranscript] { [] }
  func transcriptText(forPath path: String) -> String? { nil }
}
