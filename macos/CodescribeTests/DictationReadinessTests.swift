import XCTest

@testable import Codescribe

@MainActor
final class DictationReadinessTests: XCTestCase {
  func testOnlyLocalPowerRequiresInstalledWeights() {
    for lane in [DictationLane.appleOnly, .localPower, .cloud] {
      for model in [LocalModelReadiness.notInstalled, .downloading, .cancelled, .failed] {
        let verdict = evaluateDictationReadiness(
          lane: lane, permissions: .allGranted, localModel: model,
          admission: .sampleGranted, admissionError: nil)
        XCTAssertEqual(verdict.isReady, lane != .localPower)
        XCTAssertEqual(verdict.localModelBlocker, lane == .localPower ? model : nil)
      }
    }
  }

  func testOptionalScreenAndDiskGrantsNeverBlockDictation() {
    var permissions = PermissionSnapshot.allGranted
    permissions.screenRecording = .denied
    permissions.fullDiskAccess = .notDetermined
    let verdict = evaluateDictationReadiness(
      lane: .appleOnly, permissions: permissions, localModel: .notInstalled,
      admission: .sampleGranted, admissionError: nil)
    XCTAssertTrue(verdict.isReady)
    XCTAssertTrue(verdict.missingPermissions.isEmpty)
  }

  func testEachRequiredGrantBlocksEvenWithAnOtherwiseReadyController() {
    for kind in PermissionKind.allCases where kind.isRequiredForSetup {
      for state in [PermissionState.denied, .notDetermined] {
        var permissions = PermissionSnapshot.allGranted
        switch kind {
        case .microphone: permissions.microphone = state
        case .accessibility: permissions.accessibility = state
        case .inputMonitoring: permissions.inputMonitoring = state
        case .speechRecognition: permissions.speechRecognition = state
        case .screenRecording, .fullDiskAccess: XCTFail("optional scope entered required matrix")
        }
        let verdict = evaluateDictationReadiness(
          lane: .appleOnly, permissions: permissions, localModel: .notInstalled,
          admission: .sampleGranted, admissionError: nil)
        XCTAssertFalse(verdict.isReady)
        XCTAssertEqual(verdict.missingPermissions, [kind])
      }
    }
  }

  func testUnknownAdmissionNeverBecomesReadyAndDuplicateMicRefusalIsNotRepeated() {
    let unknown = evaluateDictationReadiness(
      lane: .appleOnly, permissions: .allGranted, localModel: .notInstalled,
      admission: nil, admissionError: "device probe failed")
    XCTAssertFalse(unknown.isReady)
    XCTAssertEqual(unknown.admissionBlocker, .admissionUnknown(error: "device probe failed"))
    var permissions = PermissionSnapshot.allGranted
    permissions.microphone = .denied
    var refusal = CsAdmissionReadiness.sampleMissing
    refusal.code = "admission_microphone_permission_unavailable"
    let denied = evaluateDictationReadiness(
      lane: .appleOnly, permissions: permissions, localModel: .notInstalled,
      admission: refusal, admissionError: nil)
    XCTAssertEqual(denied.blockers, [.permission(.microphone, .denied)])
  }

  func testMissingAndForeignDeviceCalibrationGiveSetupAndSettingsTheSameVerdict() async {
    for code in ["admission_calibration_missing", "admission_calibration_no_profile"] {
      var refusal = CsAdmissionReadiness.sampleMissing
      refusal.code = code
      let engine = MockOnboardingEngine(progress: 8)
      engine.admission = refusal
      let modelStore = WhisperDownloadStore(statusProvider: { .sampleUnavailable })
      let setup = makeSetup(engine, modelStore)
      var settingsEngine = MockSettingsEngine()
      settingsEngine.admissionReadiness = refusal
      let settings = SettingsViewModel(
        engine: settingsEngine, permissionProbe: MockPermissionProbe(.allGranted),
        whisperDownloadStore: modelStore)
      settings.refreshDictationReadiness()
      setup.finish()
      await settle { !setup.finishPending && settings.admission?.code == code }
      XCTAssertFalse(setup.dictationReadiness.isReady)
      XCTAssertTrue(engine.showOnboarding)
      XCTAssertEqual(setup.dictationReadiness, settings.dictationReadiness)
      XCTAssertTrue(setup.dictationReadiness.admissionBlocker?.isCalibration == true)
    }
  }

  func testFinishRechecksAdmissionInsteadOfSavingAStaleReadyScreen() async {
    let engine = MockOnboardingEngine(progress: 8)
    let store = WhisperDownloadStore(statusProvider: { .sampleUnavailable })
    let setup = makeSetup(engine, store)
    setup.refreshDictationReadiness()
    await settle { setup.canFinish }
    XCTAssertTrue(setup.canFinish)
    engine.admission = .sampleMissing
    setup.finish()
    await settle { !setup.finishPending }
    XCTAssertTrue(engine.showOnboarding)
    XCTAssertFalse(setup.dictationReadiness.isReady)
  }

  private func makeSetup(_ engine: MockOnboardingEngine, _ store: WhisperDownloadStore)
    -> OnboardingViewModel
  {
    OnboardingViewModel(
      engine: engine, hotkeys: MockHotkeysEngine(), agentStatus: MockAgentStatusEngine(),
      probe: MockPermissionProbe(.allGranted), whisperDownloadStore: store,
      preferredLanguages: ["en"], processInterfaceLanguage: .english)
  }

  private func settle(_ ready: () -> Bool) async {
    let deadline = Date().addingTimeInterval(1)
    while !ready(), Date() < deadline { await Task.yield() }
    XCTAssertTrue(ready(), "fixture did not settle before its deadline")
  }
}
