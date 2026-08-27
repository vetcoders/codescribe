import XCTest

@testable import Codescribe

@MainActor
final class AudioPanelTests: XCTestCase {
  func testSelectedInputWritesPromotedKeyAndSurvivesSettingsRoundTrip() {
    var writes: [(String, String)] = []
    let liveSnapshot = CsAudioInputSnapshot(
      devices: ["MacBook Pro Microphone", "USB Studio Mic"],
      configuredDevice: nil,
      runtimeDevice: "MacBook Pro Microphone",
      configuredDeviceAvailable: true,
      fallbackToDefault: false,
      runtimeConfigurationMatches: true
    )
    let writer = MockSettingsEngine(
      audioSnapshot: liveSnapshot,
      updateConfigObserver: { key, value in writes.append((key, value)) }
    )
    let firstLaunch = SettingsViewModel(
      engine: writer,
      permissionProbe: MockPermissionProbe(.allGranted)
    )

    firstLaunch.setAudioInputDevice("USB Studio Mic")

    XCTAssertEqual(writes.map(\.0), ["AUDIO_INPUT_DEVICE"])
    XCTAssertEqual(writes.map(\.1), ["USB Studio Mic"])

    var persisted = CsSettings.sample
    persisted.audioInputDevice = "USB Studio Mic"
    let restartedSnapshot = CsAudioInputSnapshot(
      devices: liveSnapshot.devices,
      configuredDevice: "USB Studio Mic",
      runtimeDevice: "USB Studio Mic",
      configuredDeviceAvailable: true,
      fallbackToDefault: false,
      runtimeConfigurationMatches: true
    )
    let reader = MockSettingsEngine(settings: persisted, audioSnapshot: restartedSnapshot)
    let restarted = SettingsViewModel(
      engine: reader,
      permissionProbe: MockPermissionProbe(.allGranted)
    )

    restarted.refreshAudioInput()

    XCTAssertEqual(reader.loadSettings().audioInputDevice, "USB Studio Mic")
    XCTAssertEqual(restarted.audioInput.runtimeDevice, "USB Studio Mic")
  }

  func testUnavailableConfiguredDeviceShowsExplicitFallbackWithoutPanic() {
    let snapshot = CsAudioInputSnapshot(
      devices: ["MacBook Pro Microphone"],
      configuredDevice: "Unplugged USB Mic",
      runtimeDevice: "MacBook Pro Microphone",
      configuredDeviceAvailable: false,
      fallbackToDefault: true,
      runtimeConfigurationMatches: true
    )

    XCTAssertEqual(
      audioInputDisplayState(snapshot),
      AudioInputDisplayState(
        tone: .fallback,
        title: "Using system fallback: MacBook Pro Microphone",
        detail: "Unplugged USB Mic is unavailable. Recording continues on the live default input."
      )
    )

    let noHardware = CsAudioInputSnapshot(
      devices: [],
      configuredDevice: "Unplugged USB Mic",
      runtimeDevice: nil,
      configuredDeviceAvailable: false,
      fallbackToDefault: true,
      runtimeConfigurationMatches: true
    )
    XCTAssertEqual(audioInputDisplayState(noHardware).tone, .unavailable)
  }

  func testSavedDeviceNeverMasqueradesAsTheCurrentRuntimeInput() {
    let snapshot = CsAudioInputSnapshot(
      devices: ["MacBook Pro Microphone", "USB Studio Mic"],
      configuredDevice: "USB Studio Mic",
      runtimeDevice: "MacBook Pro Microphone",
      configuredDeviceAvailable: true,
      fallbackToDefault: false,
      runtimeConfigurationMatches: false
    )

    XCTAssertEqual(
      audioInputDisplayState(snapshot),
      AudioInputDisplayState(
        tone: .fallback,
        title: "Currently using: MacBook Pro Microphone",
        detail:
          "Saved: USB Studio Mic. Restart Codescribe to apply it; an explicit AUDIO_INPUT_DEVICE launch override can keep a different runtime input active."
      )
    )
  }

  func testResetUsesDedicatedUnsetContractNotEmptyStringWrite() {
    var resetCalls = 0
    var writes: [(String, String)] = []
    var selected = CsSettings.sample
    selected.audioInputDevice = "USB Studio Mic"
    let engine = MockSettingsEngine(
      settings: selected,
      resetAudioInputDeviceObserver: { resetCalls += 1 },
      updateConfigObserver: { key, value in writes.append((key, value)) }
    )
    let model = SettingsViewModel(
      engine: engine,
      permissionProbe: MockPermissionProbe(.allGranted)
    )

    model.resetAudioInputDevice()

    XCTAssertEqual(resetCalls, 1)
    XCTAssertTrue(writes.isEmpty, "reset must not route an empty device string")
  }

  // Hands-free silence (TOGGLE_SILENCE_SEC) is Engine-owned epoch lifecycle;
  // its write contract is asserted in SettingsTruthTests. Audio owns only
  // hardware selection and sound feedback.
  func testAudioKnobsWriteOnlyLiveRuntimeConfigKeys() {
    var writes: [(String, String)] = []
    let model = SettingsViewModel(
      engine: MockSettingsEngine(
        updateConfigObserver: { key, value in writes.append((key, value)) }
      ),
      permissionProbe: MockPermissionProbe(.allGranted)
    )

    model.setSoundFeedbackEnabled(false)
    model.setSoundVolume(0.4)

    XCTAssertEqual(
      writes.map(\.0),
      [
        "BEEP_ON_START", "SOUND_VOLUME",
      ])
    XCTAssertEqual(writes.map(\.1), ["0", "0.40"])
  }
}

@MainActor
final class AcousticAdmissionPanelTests: XCTestCase {
  private func readiness(
    armed: Bool,
    settingArmed: Bool,
    source: String,
    message: String = "seal lane disarmed"
  ) -> CsAdmissionReadiness {
    CsAdmissionReadiness(
      ready: false,
      code: "admission_seal_lane_disarmed",
      message: message,
      deviceName: "Fixture Mic",
      sampleRate: 48_000,
      calibrationVersion: "cal2-fixture",
      calibrationStatus: "sealed",
      calibrationPath: "/tmp/calibration.json",
      calibratedDevices: ["Fixture Mic"],
      sealLaneArmed: armed,
      sealLaneSettingArmed: settingArmed,
      sealLaneSource: source,
      sealLaneEnv: "CODESCRIBE_SILERO_FUSION"
    )
  }

  func testAdmissionRowWordsTheControllerBlockerWithoutDeciding() {
    let missing = admissionDisplayState(.sampleMissing)
    XCTAssertEqual(missing.tone, .unavailable)
    XCTAssertEqual(missing.title, "Microphone not calibrated yet")
    XCTAssertTrue(missing.detail.contains("Calibrate microphone"))

    let granted = admissionDisplayState(.sampleGranted)
    XCTAssertEqual(granted.tone, .healthy)
    XCTAssertTrue(granted.title.contains("MacBook Pro Microphone"))
    XCTAssertTrue(granted.detail.contains("cal1-macbook-pro-microphone-1@48000hz"))

    let pending = admissionDisplayState(nil)
    XCTAssertEqual(pending.tone, .fallback)
  }

  func testRefreshAdmissionReadsEngineVerdict() async {
    var engine = MockSettingsEngine()
    engine.admissionReadiness = .sampleMissing
    let model = SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe(.allGranted))
    XCTAssertNil(model.admission)

    await model.refreshAdmission()

    XCTAssertEqual(model.admission?.code, "admission_calibration_missing")
    XCTAssertFalse(model.admission?.ready ?? true)
  }

  func testSealLaneControlDistinguishesSettingsFromReadOnlyOverride() {
    let settingsOff = readiness(armed: false, settingArmed: false, source: "settings")
    XCTAssertEqual(
      sealLaneControlState(settingsOff),
      SealLaneControlState(
        isOn: false,
        isEnabled: true,
        detail: "Required for committed utterances; stored in Settings."
      )
    )
    XCTAssertEqual(
      admissionDisplayState(settingsOff).title,
      "Seal lane is off in Settings › Audio"
    )

    let overrideOff = readiness(armed: false, settingArmed: true, source: "env_override")
    let overrideState = sealLaneControlState(overrideOff)
    XCTAssertTrue(overrideState.isOn, "toggle shows the stored product setting")
    XCTAssertFalse(overrideState.isEnabled, "env override makes Settings read-only")
    XCTAssertTrue(overrideState.detail.contains("CODESCRIBE_SILERO_FUSION"))
    XCTAssertTrue(admissionDisplayState(overrideOff).title.contains("override"))
  }

  func testSealLaneActionUsesTheCanonicalConfigWriter() {
    var writes: [(String, String)] = []
    let model = SettingsViewModel(
      engine: MockSettingsEngine(
        updateConfigObserver: { key, value in writes.append((key, value)) }
      ),
      permissionProbe: MockPermissionProbe(.allGranted)
    )

    model.setSealLaneArmed(false)
    model.setSealLaneArmed(true)

    XCTAssertEqual(writes.map(\.0), ["CODESCRIBE_SILERO_FUSION", "CODESCRIBE_SILERO_FUSION"])
    XCTAssertEqual(writes.map(\.1), ["0", "1"])
  }

  func testRunCalibrationStoresNoticeAndRereadsAdmission() async {
    var engine = MockSettingsEngine()
    var requestedSeconds: [UInt32] = []
    engine.admissionReadiness = .sampleGranted
    engine.calibrateEnergyObserver = { seconds in
      requestedSeconds.append(seconds)
      return .sample
    }
    let model = SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe(.allGranted))

    await model.runCalibration()

    XCTAssertEqual(requestedSeconds, [SettingsViewModel.calibrationCaptureSeconds])
    XCTAssertFalse(model.calibrationPending)
    XCTAssertTrue(model.calibrationNotice?.contains("-54.3 dBFS") == true)
    XCTAssertEqual(model.admission?.code, "admission_granted")
  }

  func testRunCalibrationRefusalIsSurfacedNotHidden() async {
    var engine = MockSettingsEngine()
    engine.admissionReadiness = .sampleMissing
    engine.calibrateEnergyObserver = { _ in
      throw NSError(
        domain: "Calibration", code: 1,
        userInfo: [
          NSLocalizedDescriptionKey: "calibration_refused: only 0.4s of active speech measured"
        ])
    }
    let model = SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe(.allGranted))

    await model.runCalibration()

    XCTAssertTrue(model.calibrationNotice?.hasPrefix("Calibration refused") == true)
    XCTAssertEqual(model.admission?.code, "admission_calibration_missing")
  }
}
