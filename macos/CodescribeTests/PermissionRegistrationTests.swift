import Foundation
import XCTest
import os

@testable import Codescribe

@MainActor
final class PermissionRegistrationTests: XCTestCase {
  func testDeniedBinaryScopesStillExecuteTheirNativeRequest() async {
    for kind in [PermissionKind.screenRecording, .accessibility, .inputMonitoring] {
      let fixture = PermissionRequestFixture()
      let action = kind.repair(for: .denied)
      XCTAssertTrue(action.requestsNatively, "A false preflight is not proof of a prior request")
      XCTAssertTrue(action.opensSettings, "A denied request still has a repair destination")

      let result = await kind.requestInApp(client: fixture, probe: fixture)

      XCTAssertEqual(fixture.requests, [kind.rawValue])
      XCTAssertEqual(result, .denied)
      XCTAssertEqual(fixture.snapshot().state(kind), .denied)
    }
  }

  func testAccessibilityPromptReturnDoesNotManufactureTrust() async {
    let fixture = PermissionRequestFixture(accessibilityPromptResult: true)
    let result = await PermissionKind.accessibility.requestInApp(client: fixture, probe: fixture)
    XCTAssertEqual(fixture.requests, [PermissionKind.accessibility.rawValue])
    XCTAssertEqual(result, .denied, "The asynchronous prompt is not the user's decision")
  }

  func testPostRequestGrantIsReadFromTheUpdatedPermissionProbe() async {
    for kind in [PermissionKind.screenRecording, .inputMonitoring, .accessibility] {
      let fixture = PermissionRequestFixture(grantOnRequest: true)
      let result = await kind.requestInApp(client: fixture, probe: fixture)
      XCTAssertEqual(fixture.requests, [kind.rawValue])
      XCTAssertEqual(result, .granted)
      XCTAssertFalse(kind.repair(for: result).requestsNatively)
      XCTAssertFalse(kind.repair(for: result).opensSettings)
    }
  }

  func testDeterminedMicrophoneAndSpeechUseSettingsWithoutRequestingAgain() async {
    for kind in [PermissionKind.microphone, .speechRecognition] {
      let fixture = PermissionRequestFixture()
      let action = kind.repair(for: .denied)
      XCTAssertFalse(action.requestsNatively)
      XCTAssertTrue(action.opensSettings)
      let result = await kind.requestInApp(client: fixture, probe: fixture)
      XCTAssertEqual(result, .denied)
      XCTAssertTrue(fixture.requests.isEmpty)
    }
  }

  func testFullDiskAccessAndAlreadyGrantedScopesNeverRequest() async {
    let denied = PermissionRequestFixture()
    let result = await PermissionKind.fullDiskAccess.requestInApp(client: denied, probe: denied)
    XCTAssertEqual(result, .denied)
    XCTAssertTrue(denied.requests.isEmpty)
    for kind in PermissionKind.allCases {
      XCTAssertFalse(kind.repair(for: .granted).requestsNatively)
      XCTAssertFalse(kind.repair(for: .granted).opensSettings)
    }
  }

  func testPermissionPaneNameTracksTheSystemVersionWithoutChangingIdentity() {
    let kind = PermissionKind.accessibility
    XCTAssertEqual(
      kind.displayName(locale: Locale(identifier: "en"), osMajorVersion: 26), "Accessibility")
    XCTAssertEqual(
      kind.displayName(locale: Locale(identifier: "en"), osMajorVersion: 27),
      "Device Control and Data Access")
    XCTAssertEqual(kind.rawValue, "Accessibility")
    XCTAssertTrue(kind.settingsURL?.absoluteString.hasSuffix("Privacy_Accessibility") == true)
  }

  func testAdmissionHeadlinesUseTheCatalogForPolishAndKeepUnknownDiagnostics() {
    let code = "admission_microphone_permission_unavailable"
    XCTAssertEqual(
      AdmissionRefusalCopy.headline(code: code, fallback: "wire", locale: Locale(identifier: "pl")),
      "Dostęp do mikrofonu jest niedostępny")
    XCTAssertEqual(
      AdmissionRefusalCopy.headline(code: code, fallback: "wire", locale: Locale(identifier: "en")),
      "Microphone access is unavailable")
    XCTAssertEqual(
      AdmissionRefusalCopy.headline(
        code: "unknown_code", fallback: "diagnostic preserved", locale: Locale(identifier: "pl")),
      "diagnostic preserved")
  }
}

/// Native calls remain in the production client. This shared lock-backed
/// requester/probe drives the production request function without host TCC.
final class PermissionRequestFixture: PermissionRequesting, PermissionProbing, Sendable {
  private struct State: Sendable {
    var snapshot = PermissionSnapshot(
      microphone: .denied, accessibility: .denied, inputMonitoring: .denied,
      screenRecording: .denied, speechRecognition: .denied, fullDiskAccess: .denied)
    var requests: [String] = []
  }
  private let state = OSAllocatedUnfairLock(initialState: State())
  private let grantOnRequest: Bool
  private let accessibilityPromptResult: Bool

  init(grantOnRequest: Bool = false, accessibilityPromptResult: Bool = false) {
    self.grantOnRequest = grantOnRequest
    self.accessibilityPromptResult = accessibilityPromptResult
  }

  var requests: [String] { state.withLock { $0.requests } }
  func snapshot() -> PermissionSnapshot { state.withLock { $0.snapshot } }

  private func request(_ kind: PermissionKind) -> Bool {
    state.withLock {
      $0.requests.append(kind.rawValue)
      guard grantOnRequest else { return false }
      switch kind {
      case .microphone: $0.snapshot.microphone = .granted
      case .accessibility: $0.snapshot.accessibility = .granted
      case .inputMonitoring: $0.snapshot.inputMonitoring = .granted
      case .screenRecording: $0.snapshot.screenRecording = .granted
      case .speechRecognition: $0.snapshot.speechRecognition = .granted
      case .fullDiskAccess: $0.snapshot.fullDiskAccess = .granted
      }
      return true
    }
  }

  func requestScreenCapture() -> Bool { request(.screenRecording) }
  func requestInputMonitoring() -> Bool { request(.inputMonitoring) }
  func requestAccessibilityPrompt() -> Bool {
    _ = request(.accessibility)
    return accessibilityPromptResult
  }
  func requestMicrophone() async -> Bool { request(.microphone) }
  func requestSpeechRecognition() async -> PermissionState {
    request(.speechRecognition) ? .granted : .denied
  }
}
