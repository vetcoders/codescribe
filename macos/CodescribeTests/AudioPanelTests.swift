import XCTest

@testable import Codescribe

@MainActor
final class AudioPanelTests: XCTestCase {
  func testActiveModelHasNoRemoveActionWhileRefusedModelCanBeRemoved() {
    XCTAssertFalse(DictationWhisperModelTab.canRemoveModel(status: "active"))
    XCTAssertTrue(DictationWhisperModelTab.canRemoveModel(status: "refused"))
    XCTAssertTrue(DictationWhisperModelTab.canRemoveModel(status: "broken"))
  }

  func testStorageStatusCodesMapToReadableLabelsAndUnknownStaysVerbatim() {
    XCTAssertEqual(DictationWhisperModelTab.statusLabel("active"), "Selected model")
    XCTAssertEqual(DictationWhisperModelTab.statusLabel("usable"), "Ready")
    XCTAssertEqual(DictationWhisperModelTab.statusLabel("refused"), "Refused")
    XCTAssertEqual(DictationWhisperModelTab.statusLabel("broken"), "Broken")
    XCTAssertEqual(DictationWhisperModelTab.statusLabel("weird"), "weird")
  }

  /// Bridge refusal prose is English; the row shows one short status and
  /// keeps the raw text for the details disclosure (round 13).
  func testRefusalReasonsCollapseToShortStatuses() {
    XCTAssertEqual(
      DictationWhisperModelTab.reasonTag(
        "Quantized weights are not supported by the local engine"),
      "Unsupported")
    XCTAssertEqual(
      DictationWhisperModelTab.reasonTag("invalid Whisper tokenizer at /x/tokenizer.json"),
      "Tokenizer problem")
    XCTAssertEqual(
      DictationWhisperModelTab.reasonTag("Whisper tokenizer is missing required token"),
      "Tokenizer problem")
    XCTAssertEqual(
      DictationWhisperModelTab.reasonTag("model weights file missing"), "Incomplete files")
    XCTAssertEqual(
      DictationWhisperModelTab.reasonTag("something else"), "Failed validation")
    XCTAssertEqual(DictationWhisperModelTab.reasonTag(nil), "Unavailable")
  }

  /// `loaded` (resident weights) and `resolvedPath` (next load) are distinct
  /// facts; the row never calls the next load "in use".
  func testResidencyLabelDistinguishesLoadedFromNextLoad() {
    let sample = CsWhisperModelCatalog.sample
    XCTAssertEqual(DictationWhisperModelTab.residencyLabel(sample), "Loaded in memory")

    var pending = sample
    pending.loaded = nil
    XCTAssertEqual(
      DictationWhisperModelTab.residencyLabel(pending),
      "Not loaded yet · loads on the next recording")

    var switched = sample
    switched.loaded = sample.options[1].path
    XCTAssertEqual(
      DictationWhisperModelTab.residencyLabel(switched),
      "Loaded in memory: Large v3 · FP16 · next recording loads Large v3 Turbo · FP16")

    var unresolved = sample
    unresolved.resolvedPath = nil
    XCTAssertEqual(
      DictationWhisperModelTab.residencyLabel(unresolved),
      "The selected model cannot be resolved; the next recording has no model to load.")

    XCTAssertEqual(DictationWhisperModelTab.residencyLabel(nil), "Checking the model catalog…")
  }

  func testSelectedOptionLabelFallsBackToRawReference() {
    let sample = CsWhisperModelCatalog.sample
    XCTAssertEqual(DictationWhisperModelTab.selectedOptionLabel(sample), "Large v3 Turbo · FP16")
    var orphan = sample
    orphan.configured = "ghost-model"
    XCTAssertEqual(DictationWhisperModelTab.selectedOptionLabel(orphan), "ghost-model")
  }

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

  func testRecordingRowTracksActivePreparingAndFinishingDespiteAdmissionChange() {
    let cases: [(Bool?, Bool, Bool, String)] = [
      (true, false, false, "Recording in progress"),
      (true, true, false, "Starting recording…"),
      (true, false, true, "Finishing recording…"),
      (nil, false, false, "Checking recording activity…"),
    ]
    for (recording, preparing, processing, expected) in cases {
      let rows = audioReadinessSteps(
        input: .sample, microphonePermission: .denied, admission: nil,
        dictationShortcut: "Hold Fn/Globe", recording: recording,
        preparing: preparing, processing: processing)
      XCTAssertEqual(rows.last?.title, expected)
      XCTAssertNotEqual(rows.last?.title, "Ready to record")
    }
  }

  func testReadinessCockpitShowsEveryPrerequisiteAtOnce() {
    let ready = audioReadinessSteps(
      input: .sample,
      microphonePermission: .granted,
      admission: .sampleGranted,
      dictationShortcut: "Hold Fn/Globe"
    )
    XCTAssertEqual(ready.map(\.id), [.microphone, .calibration, .sealLane, .recording])
    XCTAssertEqual(ready.map(\.tone), [.healthy, .healthy, .healthy, .healthy])
    XCTAssertEqual(ready.last?.title, "Ready to record")
    XCTAssertTrue(ready.last?.detail.contains("Hold Fn/Globe") == true)

    let missing = audioReadinessSteps(
      input: .sample,
      microphonePermission: .granted,
      admission: .sampleMissing,
      dictationShortcut: "Hold Fn/Globe"
    )
    XCTAssertEqual(missing[0].tone, .healthy)
    XCTAssertEqual(missing[1].tone, .unavailable)
    XCTAssertEqual(missing[2].tone, .healthy)
    XCTAssertEqual(missing[3].tone, .unavailable)
    XCTAssertTrue(missing[1].title.contains("Calibration"))

    let checking = audioReadinessSteps(
      input: .sample,
      microphonePermission: .granted,
      admission: nil,
      dictationShortcut: "Hold Fn/Globe"
    )
    XCTAssertEqual(checking.map(\.tone), [.healthy, .fallback, .fallback, .fallback])

    let denied = audioReadinessSteps(
      input: .sample,
      microphonePermission: .denied,
      admission: .sampleGranted,
      dictationShortcut: "Hold Fn/Globe"
    )
    XCTAssertEqual(denied[0].tone, .unavailable)
    XCTAssertEqual(denied[1].tone, .fallback)
    XCTAssertEqual(denied[2].tone, .fallback)
    XCTAssertEqual(denied[3].tone, .unavailable)
    XCTAssertTrue(denied[0].detail.contains("Privacy & Security"))

    let vadUnavailable = CsAdmissionReadiness(
      ready: false,
      code: "admission_seal_vad_unavailable",
      message: "Silero VAD failed to load",
      deviceName: "Fixture Mic",
      sampleRate: 48_000,
      calibrationVersion: "cal-fixture",
      calibrationStatus: "sealed",
      calibrationPath: "/tmp/calibration.json",
      calibratedDevices: ["Fixture Mic"],
      sealLaneArmed: true,
      sealLaneSettingArmed: true,
      sealLaneSource: "settings",
      sealLaneEnv: "CODESCRIBE_SILERO_FUSION"
    )
    let vadSteps = audioReadinessSteps(
      input: .sample,
      microphonePermission: .granted,
      admission: vadUnavailable,
      dictationShortcut: "Hold Fn/Globe"
    )
    XCTAssertEqual(vadSteps[2].tone, .unavailable)
    XCTAssertEqual(vadSteps[2].title, "Silero VAD did not load")
  }

  /// The sentence under "Keep completed recordings" describes the SELECTED
  /// choice, and only a choice that actually expires something carries one:
  /// "Forever" says everything in its own value. Every sentence keeps the
  /// distinction between recorded audio and text history.
  func testRetentionSentenceIsAFunctionOfTheSelectedChoice() throws {
    XCTAssertNil(audioRetentionDetail("forever"), "indefinite retention needs no sentence")
    // The config loader resolves an unknown value to `forever`; so must the copy.
    XCTAssertNil(audioRetentionDetail("whenever"))
    XCTAssertNil(audioRetentionDetail(""))

    let off = try XCTUnwrap(audioRetentionDetail("off"))
    XCTAssertTrue(off.contains("discarded"))

    let expiring = ["off", "24h", "7_days", "30_days"].compactMap(audioRetentionDetail)
    XCTAssertEqual(expiring.count, 4, "each expiring choice states its own rule")
    XCTAssertEqual(Set(expiring).count, 4)
    for sentence in expiring {
      XCTAssertTrue(sentence.contains("Text history"), sentence)
    }

    XCTAssertTrue(try XCTUnwrap(audioRetentionDetail("24h")).contains("24 hours"))
    XCTAssertTrue(try XCTUnwrap(audioRetentionDetail("7_days")).contains("7 days"))
    XCTAssertTrue(try XCTUnwrap(audioRetentionDetail("30_days")).contains("30 days"))
  }

  /// The microphone card answers one question — which input records — and
  /// carries a sentence only when something is actually wrong with it.
  func testMicrophoneCardNamesTheRuntimeInputAndNoticesOnlyRealProblems() {
    let healthy = audioInputCardState(
      CsAudioInputSnapshot(
        devices: ["MacBook Pro Microphone"],
        configuredDevice: nil,
        runtimeDevice: "MacBook Pro Microphone",
        configuredDeviceAvailable: true,
        fallbackToDefault: false,
        runtimeConfigurationMatches: true
      ))
    XCTAssertEqual(healthy.current, "Currently using: MacBook Pro Microphone")
    XCTAssertNil(healthy.notice, "a working microphone needs no sentence")
    XCTAssertNil(healthy.noticeTone)

    let unapplied = audioInputCardState(
      CsAudioInputSnapshot(
        devices: ["MacBook Pro Microphone", "USB Studio Mic"],
        configuredDevice: "USB Studio Mic",
        runtimeDevice: "MacBook Pro Microphone",
        configuredDeviceAvailable: true,
        fallbackToDefault: false,
        runtimeConfigurationMatches: false
      ))
    XCTAssertEqual(unapplied.current, "Currently using: MacBook Pro Microphone")
    XCTAssertEqual(unapplied.noticeTone, .fallback)
    XCTAssertTrue(unapplied.notice?.contains("Restart Codescribe") == true)

    let noHardware = audioInputCardState(
      CsAudioInputSnapshot(
        devices: [],
        configuredDevice: nil,
        runtimeDevice: nil,
        configuredDeviceAvailable: false,
        fallbackToDefault: false,
        runtimeConfigurationMatches: true
      ))
    XCTAssertEqual(noHardware.current, "Currently using: no microphone")
    XCTAssertEqual(noHardware.noticeTone, .unavailable)
    XCTAssertTrue(noHardware.notice?.contains("Connect a microphone") == true)
  }

  /// The stored profile identifier is a diagnostic, not a readiness row. It
  /// stays reachable under the details disclosure and nowhere else.
  func testCalibrationProfileIdentifierLivesOnlyInTheDetailsDisclosure() {
    let profile = "cal1-macbook-pro-microphone-1@48000hz"
    let rows = audioReadinessSteps(
      input: .sample,
      microphonePermission: .granted,
      admission: .sampleGranted,
      dictationShortcut: "Hold Fn/Globe"
    )
    for row in rows {
      XCTAssertFalse(row.title.contains(profile), "\(row.id) leaks the profile id")
      XCTAssertFalse(row.detail.contains(profile), "\(row.id) leaks the profile id")
    }
    XCTAssertEqual(rows[1].tone, .healthy)
    XCTAssertEqual(rows[1].title, "Microphone calibrated")

    let details = audioCalibrationDetails(.sampleGranted)
    XCTAssertEqual(
      details.map(\.id), ["device", "sampleRate", "profile", "status", "path"])
    XCTAssertTrue(details.contains { $0.value == profile })
    XCTAssertTrue(details.contains { $0.value.contains("48") && $0.value.contains("Hz") })

    // Nothing is invented for a verdict that carries no measurement.
    XCTAssertTrue(audioCalibrationDetails(nil).isEmpty)
    let unmeasured = audioCalibrationDetails(.sampleMissing)
    XCTAssertEqual(unmeasured.map(\.id), ["status", "path"])
    XCTAssertFalse(unmeasured.contains { $0.id == "profile" })
  }

  /// Without a bound Dictation gesture the row must not send the user after a
  /// shortcut they do not have; with one, it keeps the configured name.
  func testRecordingStartHintNamesTheConfiguredShortcutOrTheButton() {
    XCTAssertEqual(
      recordingStartHint("Hold Fn/Globe"), "Press Hold Fn/Globe or click Start recording.")
    let unbound = recordingStartHint(nil)
    XCTAssertEqual(unbound, recordingStartHint(""))
    XCTAssertTrue(unbound.contains("Start recording"))
    XCTAssertTrue(unbound.contains("Hotkeys"))
    XCTAssertFalse(unbound.contains("Press"), "no gesture is named when none is bound")

    let rows = audioReadinessSteps(
      input: .sample,
      microphonePermission: .granted,
      admission: .sampleGranted,
      dictationShortcut: nil
    )
    XCTAssertEqual(rows.last?.title, "Ready to record")
    XCTAssertEqual(rows.last?.detail, unbound)
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

  // The silence window (TOGGLE_SILENCE_SEC) is Engine-owned epoch lifecycle;
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
    // The switch moved to the Lab desk; the verdict names where it now lives.
    XCTAssertEqual(
      admissionDisplayState(settingsOff).title,
      "Seal lane is off in Settings › Lab"
    )

    let overrideOff = readiness(armed: false, settingArmed: true, source: "env_override")
    let overrideState = sealLaneControlState(overrideOff)
    XCTAssertTrue(overrideState.isOn, "toggle shows the stored product setting")
    XCTAssertFalse(overrideState.isEnabled, "env override makes Settings read-only")
    XCTAssertTrue(overrideState.detail.contains("CODESCRIBE_SILERO_FUSION"))
    XCTAssertTrue(admissionDisplayState(overrideOff).title.contains("override"))
  }

  /// The one status line is neutral only while nothing blocks recording. Every
  /// blocker replaces it with its own problem, explanation and remedy — a
  /// calm "Ready to record" may never sit above a failed prerequisite.
  func testReadinessSummaryNeverMasksABlockedPrerequisite() {
    let ready = audioReadinessSummary(
      input: .sample, microphonePermission: .granted, admission: .sampleGranted)
    XCTAssertEqual(ready.tone, .healthy)
    XCTAssertEqual(ready.title, "Ready to record")
    XCTAssertNil(ready.detail, "a working state explains nothing")
    XCTAssertEqual(ready.remedy, .none)

    let denied = audioReadinessSummary(
      input: .sample, microphonePermission: .denied, admission: .sampleGranted)
    XCTAssertEqual(denied.tone, .unavailable)
    XCTAssertEqual(denied.title, "Microphone access is off")
    XCTAssertTrue(denied.detail?.contains("Privacy & Security") == true)
    XCTAssertEqual(denied.remedy, .microphonePermission)

    let uncalibrated = audioReadinessSummary(
      input: .sample, microphonePermission: .granted, admission: .sampleMissing)
    XCTAssertEqual(uncalibrated.title, "Calibration required")
    XCTAssertEqual(uncalibrated.remedy, .calibrate)

    // The committing row is named for what it is, so as a headline it has to
    // state the consequence instead, and send the user to the one switch.
    let sealOff = readiness(armed: false, settingArmed: false, source: "settings")
    let blocked = audioReadinessSummary(
      input: .sample, microphonePermission: .granted, admission: sealOff)
    XCTAssertEqual(blocked.tone, .unavailable)
    XCTAssertEqual(blocked.title, "Recording cannot start")
    XCTAssertTrue(blocked.detail?.contains("Settings › Lab") == true)
    XCTAssertEqual(blocked.remedy, .openLab)

    // An override is removed outside the app, so Lab is not offered for it.
    let overrideOff = readiness(armed: false, settingArmed: true, source: "env_override")
    let overridden = audioReadinessSummary(
      input: .sample, microphonePermission: .granted, admission: overrideOff)
    XCTAssertEqual(overridden.remedy, .none)
    XCTAssertTrue(overridden.detail?.contains("CODESCRIBE_SILERO_FUSION") == true)

    var vadBroken = readiness(armed: true, settingArmed: true, source: "settings")
    vadBroken.code = "admission_seal_vad_unavailable"
    vadBroken.message = "Silero VAD failed to load"
    let vad = audioReadinessSummary(
      input: .sample, microphonePermission: .granted, admission: vadBroken)
    XCTAssertEqual(vad.title, "Silero VAD did not load")
    XCTAssertEqual(vad.remedy, .none, "a failed detector is not fixed from Lab")

    // A take in flight is lifecycle, not a blocker: the line follows the
    // recorder and offers nothing to repair.
    let lifecycle: [(recording: Bool?, preparing: Bool, processing: Bool, title: String)] = [
      (true, false, false, "Recording in progress"),
      (true, true, false, "Starting recording…"),
      (true, false, true, "Finishing recording…"),
    ]
    for state in lifecycle {
      let live = audioReadinessSummary(
        input: .sample, microphonePermission: .denied, admission: nil,
        recording: state.recording, preparing: state.preparing, processing: state.processing)
      XCTAssertEqual(live.title, state.title)
      XCTAssertNil(live.detail)
      XCTAssertEqual(live.remedy, .none)
    }
  }

  /// A system-fallback microphone keeps recording working, so it belongs on
  /// the microphone card as a notice — never as a readiness blocker.
  func testSystemFallbackMicrophoneIsANoticeNotABlocker() {
    let fallback = CsAudioInputSnapshot(
      devices: ["MacBook Pro Microphone"],
      configuredDevice: "Unplugged USB Mic",
      runtimeDevice: "MacBook Pro Microphone",
      configuredDeviceAvailable: false,
      fallbackToDefault: true,
      runtimeConfigurationMatches: true
    )
    let summary = audioReadinessSummary(
      input: fallback, microphonePermission: .granted, admission: .sampleGranted)
    XCTAssertEqual(summary.tone, .healthy)
    XCTAssertEqual(summary.title, "Ready to record")
    XCTAssertNil(summary.detail)
    XCTAssertNotNil(audioInputCardState(fallback).notice, "the card still says it")
  }

  /// The two short rows carry a value and nothing else; the explanation of a
  /// bad value belongs to the status line above them.
  func testReadinessFactsCarryOnlyTheirCurrentValue() {
    XCTAssertEqual(
      audioCalibrationFact(.sampleGranted, microphonePermission: .granted),
      AudioReadinessFact(
        id: .calibration, label: "Microphone calibration", value: "Ready", tone: .healthy))
    XCTAssertEqual(
      audioCalibrationFact(.sampleMissing, microphonePermission: .granted).value, "Required")
    XCTAssertEqual(
      audioCalibrationFact(.sampleGranted, microphonePermission: .denied).value, "Waiting")
    XCTAssertEqual(audioCalibrationFact(nil, microphonePermission: .granted).value, "Checking…")

    let armed = readiness(armed: true, settingArmed: true, source: "settings")
    XCTAssertEqual(
      audioSealLaneFact(armed),
      AudioReadinessFact(
        id: .sealLane, label: "Committing fragments", value: "On", tone: .healthy))
    XCTAssertEqual(
      audioSealLaneFact(readiness(armed: false, settingArmed: false, source: "settings")).value,
      "Off")
    XCTAssertEqual(audioSealLaneFact(nil).value, "Checking…")

    var vadBroken = armed
    vadBroken.code = "admission_seal_vad_unavailable"
    XCTAssertEqual(audioSealLaneFact(vadBroken).value, "Unavailable")
    XCTAssertEqual(audioSealLaneFact(vadBroken).tone, .unavailable)
  }

  /// One name for the committing row in every state, a readable switch state in
  /// its sentence, and all four readiness rows still present. The hard VAD
  /// failure keeps its own title because it is a different fact.
  func testCommittingRowKeepsOneNameAndSpellsOutTheSwitchState() {
    let name = "Committing transcript fragments"
    let settingsOn = readiness(armed: true, settingArmed: true, source: "settings")
    let settingsOff = readiness(armed: false, settingArmed: false, source: "settings")
    let overrideOn = readiness(armed: true, settingArmed: false, source: "env_override")
    let overrideOff = readiness(armed: false, settingArmed: true, source: "env_override")

    let cases: [(String, CsAdmissionReadiness?)] = [
      ("settings on", settingsOn), ("settings off", settingsOff),
      ("override on", overrideOn), ("override off", overrideOff),
      ("checking", nil),
    ]
    for (label, admission) in cases {
      let rows = audioReadinessSteps(
        input: .sample, microphonePermission: .granted, admission: admission,
        dictationShortcut: "Hold Fn/Globe")
      XCTAssertEqual(rows.map(\.id), [.microphone, .calibration, .sealLane, .recording], label)
      XCTAssertEqual(rows[2].title, name, label)
      XCTAssertFalse(rows[2].detail.isEmpty, label)
      XCTAssertFalse(rows[2].title.lowercased().contains("seal"), label)
    }

    // The same row keeps the name while the microphone step is still open.
    let denied = audioReadinessSteps(
      input: .sample, microphonePermission: .denied, admission: settingsOn,
      dictationShortcut: "Hold Fn/Globe")
    XCTAssertEqual(denied[2].title, name)
    XCTAssertEqual(denied[2].tone, .fallback)

    func committingDetail(_ admission: CsAdmissionReadiness) -> String {
      audioReadinessSteps(
        input: .sample, microphonePermission: .granted, admission: admission,
        dictationShortcut: "Hold Fn/Globe")[2].detail
    }
    XCTAssertTrue(committingDetail(settingsOn).hasPrefix("On"))
    XCTAssertTrue(committingDetail(settingsOff).hasPrefix("Off"))
    XCTAssertTrue(committingDetail(settingsOff).contains("cannot start"))
    XCTAssertTrue(committingDetail(overrideOn).hasPrefix("On"))
    XCTAssertTrue(committingDetail(overrideOn).contains("CODESCRIBE_SILERO_FUSION"))
    XCTAssertTrue(committingDetail(overrideOff).hasPrefix("Off"))
    XCTAssertTrue(committingDetail(overrideOff).contains("CODESCRIBE_SILERO_FUSION"))

    var broken = readiness(armed: true, settingArmed: true, source: "settings")
    broken.code = "admission_seal_vad_unavailable"
    broken.message = "Silero VAD failed to load"
    let brokenRows = audioReadinessSteps(
      input: .sample, microphonePermission: .granted, admission: broken,
      dictationShortcut: "Hold Fn/Globe")
    XCTAssertEqual(brokenRows[2].title, "Silero VAD did not load")
    XCTAssertEqual(brokenRows[2].detail, "Silero VAD failed to load")
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
    XCTAssertNil(model.calibrationStartedAt)
    XCTAssertTrue(model.calibrationNotice?.contains("-54.3 dBFS") == true)
    XCTAssertTrue(model.calibrationNotice?.contains("peak -12.5 dBFS") == true)
    XCTAssertEqual(model.admission?.code, "admission_granted")
  }

  func testCalibrationProgressIsBoundedAndCountsDownHonestly() {
    XCTAssertEqual(SettingsViewModel.calibrationProgress(elapsedSeconds: -1), 0)
    XCTAssertEqual(SettingsViewModel.calibrationProgress(elapsedSeconds: 5), 0.5)
    XCTAssertEqual(SettingsViewModel.calibrationProgress(elapsedSeconds: 20), 1)
    XCTAssertEqual(SettingsViewModel.calibrationRemainingSeconds(elapsedSeconds: 0), 10)
    XCTAssertEqual(SettingsViewModel.calibrationRemainingSeconds(elapsedSeconds: 9.2), 1)
    XCTAssertEqual(SettingsViewModel.calibrationRemainingSeconds(elapsedSeconds: 12), 0)
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
