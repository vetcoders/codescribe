import XCTest

@testable import Codescribe

/// Already-open Settings and the tray Quick Settings follow each other through
/// the one persisted snapshot. Both synthetic engines below read and write the
/// same in-memory `settings.json`; the invalidation scope carries no values.
@MainActor
final class SettingsLiveProjectionTests: XCTestCase {
  func testTrayQuickSettingWritesReachSettingsThatIsAlreadyOpen() {
    let file = SyntheticSettingsFile(pasteMode: .safe, formattingLevel: "correction")
    let scope = ConfigurationInvalidation()
    let settings = makeSettingsModel(file: file, scope: scope)
    let trayEngine = SyntheticTrayEngine(file: file)
    let tray = TrayViewModel(engine: trayEngine, configurationInvalidation: scope)
    tray.refreshQuickSettings()
    XCTAssertEqual(settings.pasteMode, .safe)

    tray.setPasteMode(.comfort)
    tray.setAutoFormatLevel(.max)

    XCTAssertEqual(settings.pasteMode, .comfort, "open Settings must show the tray's paste mode")
    XCTAssertEqual(
      FormattingPolicyOption(storedValue: settings.settings.formattingLevel), .max,
      "open Settings must show the tray's formatting level")
    XCTAssertEqual(file.writes.map { $0.key }, ["PASTE_MODE", "FORMATTING_LEVEL"])
    XCTAssertEqual(
      trayEngine.toggleReads, 3,
      "the writer re-reads once per write and never answers its own edge")
  }

  func testSettingsWritesReachQuickSettingsWithoutAWriteLoop() {
    let file = SyntheticSettingsFile(pasteMode: .safe, formattingLevel: "correction")
    let scope = ConfigurationInvalidation()
    let settings = makeSettingsModel(file: file, scope: scope)
    let trayEngine = SyntheticTrayEngine(file: file)
    let tray = TrayViewModel(engine: trayEngine, configurationInvalidation: scope)
    tray.refreshQuickSettings()

    settings.setPasteMode(.off)
    settings.setFormattingLevel("smart")

    XCTAssertEqual(tray.pasteMode, .off)
    XCTAssertEqual(tray.autoFormatLevel, .smart)
    XCTAssertEqual(
      file.writes.map { $0.key }, ["PASTE_MODE", "FORMATTING_LEVEL"],
      "observers only read: one edit is exactly one persisted write")
    XCTAssertTrue(trayEngine.writes.isEmpty, "the tray must not write back what it observed")
  }

  func testExternalWriteKeepsAnUnsavedShortcutDraft() {
    let file = SyntheticSettingsFile(pasteMode: .safe, formattingLevel: "correction")
    let scope = ConfigurationInvalidation()
    let settings = makeSettingsModel(file: file, scope: scope, hotkeys: MockHotkeysEngine())
    settings.loadHotkeys()
    settings.editDraftBinding(mode: .dictation, binding: .doubleCtrl)
    XCTAssertTrue(settings.hasPendingBindingChanges)
    let draft = settings.draftBindings
    let tray = TrayViewModel(
      engine: SyntheticTrayEngine(file: file), configurationInvalidation: scope)

    tray.setPasteMode(.comfort)

    XCTAssertEqual(settings.pasteMode, .comfort, "the persisted change still lands")
    XCTAssertEqual(
      settings.draftBindings.map(\.binding), draft.map(\.binding),
      "an external write must not reset the editor draft")
    XCTAssertTrue(settings.hasPendingBindingChanges)
  }

  func testRecorderEdgeShowsResidentWhisperAfterTheInitialSelection() async {
    let file = SyntheticSettingsFile(pasteMode: .safe, formattingLevel: "correction")
    let resident = SyntheticWhisperRuntime(loaded: nil)
    let scope = ConfigurationInvalidation()
    let settings = makeSettingsModel(file: file, scope: scope, whisper: resident)
    settings.refreshWhisperModelCatalog()
    XCTAssertNil(settings.whisperModelCatalog?.loaded)

    settings.selectWhisperModel(SyntheticWhisperRuntime.resolved)
    await waitUntil { !settings.whisperModelSwitchPending }
    XCTAssertNil(settings.whisperModelError)
    XCTAssertFalse(settings.whisperResidencySettled, "selected, but no weights are resident yet")

    // The next take loads the weights; the recorder edge is the only signal.
    resident.loaded = SyntheticWhisperRuntime.resolved
    scope.recordingLifecycleChanged()

    XCTAssertEqual(settings.whisperModelCatalog?.loaded, SyntheticWhisperRuntime.resolved)
    XCTAssertTrue(settings.whisperResidencySettled)
    XCTAssertEqual(
      DictationWhisperModelTab.residencyLabel(settings.whisperModelCatalog),
      DictationWhisperModelTab.residencyLabel(.sample),
      "the open picker must say the model is loaded without reopening Settings")
  }

  func testResidencyObservationWaitsWithoutBudgetAndStopsOnceSettled() async {
    let file = SyntheticSettingsFile(pasteMode: .safe, formattingLevel: "correction")
    let resident = SyntheticWhisperRuntime(loaded: nil)
    let scope = ConfigurationInvalidation()
    let settings = makeSettingsModel(file: file, scope: scope, whisper: resident)
    settings.whisperResidencyPollInterval = (.milliseconds(5), .milliseconds(5))
    settings.refreshWhisperModelCatalog()

    // Idle and unloaded with no recorder edge is stable truth: no reads.
    settings.beginWhisperResidencyObservation()
    XCTAssertFalse(settings.whisperResidencyPending)
    let idleReads = resident.reads
    try? await Task.sleep(for: .milliseconds(50))
    XCTAssertEqual(resident.reads, idleReads)

    // A take makes a load expected; the waiting outlives the former
    // fifteen-read budget with no further edge.
    scope.recordingLifecycleChanged()
    await waitUntil { resident.reads > idleReads + 1 + 15 }
    XCTAssertTrue(settings.whisperResidencyPending)

    // The late load lands without an edge and is shown; reads then stop.
    resident.loaded = SyntheticWhisperRuntime.resolved
    await waitUntil { settings.whisperResidencySettled }
    XCTAssertFalse(settings.whisperResidencyPending)
    let settledReads = resident.reads
    try? await Task.sleep(for: .milliseconds(50))
    XCTAssertEqual(resident.reads, settledReads, "a settled catalog is not re-read")

    // A closed window owns nothing, even with a load expected.
    resident.loaded = nil
    settings.endWhisperResidencyObservation()
    scope.recordingLifecycleChanged()
    let closedReads = resident.reads
    try? await Task.sleep(for: .milliseconds(50))
    XCTAssertEqual(resident.reads, closedReads)
  }

  func testUnscopedModelsNeitherPublishNorObserve() {
    let file = SyntheticSettingsFile(pasteMode: .safe, formattingLevel: "correction")
    let scope = ConfigurationInvalidation()
    let unscoped = makeSettingsModel(file: file, scope: nil)
    let trayEngine = SyntheticTrayEngine(file: file)
    let tray = TrayViewModel(engine: trayEngine, configurationInvalidation: scope)
    tray.refreshQuickSettings()
    let reads = trayEngine.toggleReads

    unscoped.setPasteMode(.off)

    XCTAssertEqual(trayEngine.toggleReads, reads, "a test or preview model must not wake peers")
    XCTAssertEqual(tray.pasteMode, .safe)
  }

  func testReopenedSettingsObservesColdLoadWithoutAnotherRecorderEdge() async {
    let file = SyntheticSettingsFile(pasteMode: .safe, formattingLevel: "correction")
    let resident = SyntheticWhisperRuntime(loaded: nil)
    let state = OverlayState(autoSendEnabled: { false }, micAccessProvider: { true })
    let tray = TrayViewModel(engine: MockTrayEngine())
    let settings = makeSettingsModel(
      file: file, scope: ConfigurationInvalidation(), whisper: resident,
      recordingControls: { (state: state, tray: tray) })
    settings.whisperResidencyPollInterval = (.milliseconds(5), .milliseconds(5))
    settings.beginWhisperResidencyObservation()
    settings.endWhisperResidencyObservation()

    // The window missed this take's edge while closed.
    state.handleRecordingStarted()
    defer {
      settings.endWhisperResidencyObservation()
      state.finishControllerRecording()
    }
    settings.beginWhisperResidencyObservation()
    XCTAssertTrue(settings.whisperResidencyPending)
    resident.loaded = SyntheticWhisperRuntime.resolved
    await waitUntil { settings.whisperResidencySettled }
    XCTAssertEqual(settings.whisperModelCatalog?.loaded, SyntheticWhisperRuntime.resolved)
  }

  // MARK: - Fixtures

  private func makeSettingsModel(
    file: SyntheticSettingsFile,
    scope: ConfigurationInvalidation?,
    hotkeys: HotkeysEngine? = nil,
    whisper: SyntheticWhisperRuntime = SyntheticWhisperRuntime(loaded: nil),
    recordingControls: (() -> (state: OverlayState, tray: TrayViewModel)?)? = nil
  ) -> SettingsViewModel {
    // The mock's default selection outcome is "applied"; residency is the
    // separate catalog fact under test.
    let engine = MockSettingsEngine(
      settingsLoader: { file.settings },
      whisperCatalogLoader: { whisper.catalog() },
      updateConfigObserver: { key, value in file.record(key, value) }
    )
    return SettingsViewModel(
      engine: engine,
      permissionProbe: MockPermissionProbe(),
      hotkeys: hotkeys,
      whisperDownloadStore: WhisperDownloadStore(
        statusProvider: { .sampleUnavailable }, download: { _ in .sampleUnavailable }),
      configurationInvalidation: scope,
      audioRecordingControlProvider: recordingControls,
      servingStatusProvider: { nil }
    )
  }

  private func waitUntil(
    timeout: Duration = .seconds(2), _ condition: @MainActor () -> Bool
  ) async {
    let deadline = ContinuousClock.now + timeout
    while !condition(), ContinuousClock.now < deadline {
      try? await Task.sleep(for: .milliseconds(5))
    }
    let reached = condition()
    XCTAssertTrue(reached, "condition not reached within \(timeout)")
  }
}

/// One in-memory `settings.json` both synthetic engines read and write.
@MainActor
private final class SyntheticSettingsFile {
  var settings: CsSettings
  private(set) var writes: [(key: String, value: String)] = []

  init(pasteMode: CsPasteMode, formattingLevel: String) {
    var seed = CsSettings.sample
    seed.pasteMode = pasteMode
    seed.formattingLevel = formattingLevel
    settings = seed
  }

  func record(_ key: String, _ value: String) {
    writes.append((key, value))
    switch key {
    case "PASTE_MODE":
      if let mode = CsPasteMode.allModes.first(where: { $0.wireId == value }) {
        settings.pasteMode = mode
      }
    case "FORMATTING_LEVEL":
      settings.formattingLevel = value
    default:
      break
    }
  }
}

/// Resident-engine truth the catalog reports, separate from the selection.
@MainActor
private final class SyntheticWhisperRuntime {
  static let resolved = CsWhisperModelCatalog.sample.resolvedPath!
  var loaded: String?
  private(set) var reads = 0

  init(loaded: String?) { self.loaded = loaded }

  func catalog() -> CsWhisperModelCatalog {
    reads += 1
    var catalog = CsWhisperModelCatalog.sample
    catalog.loaded = loaded
    return catalog
  }
}

/// Tray seam over the same synthetic file, mirroring `RealTrayEngine`.
@MainActor
private final class SyntheticTrayEngine: TrayEngine {
  let file: SyntheticSettingsFile
  private(set) var toggleReads = 0
  private(set) var writes: [String] = []

  init(file: SyntheticSettingsFile) { self.file = file }

  func isAgentAvailable() -> Bool { true }
  func isRecording() async -> Bool { false }
  func startRecording(assistive: Bool) async throws {}
  func stopRecording() async throws {}

  func currentToggles() -> (
    showDockIcon: Bool,
    overlayEnabled: Bool,
    pasteMode: CsPasteMode,
    autoFormatLevel: FormattingPolicyOption,
    notesMode: Bool,
    startInAssistive: Bool,
    holdBadgeOption: HoldBadgeOption
  )? {
    toggleReads += 1
    guard let level = FormattingPolicyOption(storedValue: file.settings.formattingLevel) else {
      return nil
    }
    return (true, true, file.settings.pasteMode, level, false, false, .twelve)
  }

  func setQuickToggle(_ toggle: TrayQuickToggle, enabled: Bool) {
    writes.append(toggle.configKey)
    file.record(toggle.configKey, enabled ? "1" : "0")
  }
  func setPasteMode(_ mode: CsPasteMode) {
    writes.append("PASTE_MODE")
    file.record("PASTE_MODE", mode.wireId)
  }
  func setAutoFormatLevel(_ level: FormattingPolicyOption) {
    writes.append("FORMATTING_LEVEL")
    file.record("FORMATTING_LEVEL", level.rawValue)
  }
  func setHoldBadgeOption(_ option: HoldBadgeOption) -> Bool { false }
  func setNotesMode(_ enabled: Bool) -> Bool { false }
  func setStartInAssistive(_ enabled: Bool) -> Bool { false }
  func latestHistoryPath() -> String? { nil }
  func latestTranscriptText() -> String? { nil }
  func recentTranscripts(limit: Int) -> [TrayTranscript] { [] }
  func transcriptText(forPath path: String) -> String? { nil }
}
