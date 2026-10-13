import AVFoundation
import AppKit
import ApplicationServices
import CoreGraphics
import Foundation
import IOKit.hid
import Speech

// Native macOS permission probes for the Settings screen.
// Permissions are NOT exposed via FFI — they are read live from the system
// (AVAuthorization / Accessibility / IOHID / CoreGraphics) per the brief.
//
// NOTE: API-key presence is NOT a Keychain probe here. "Is a key set?" is
// answered exclusively by `CodescribeConfig.keyStatus()` (CsKeyStatus booleans),
// which reflects the core's real Keychain service. The old KeychainProbe queried
// the wrong service and always returned false — it has been removed.

/// Tri-state permission result. `.notDetermined` is rendered as actionable
/// ("open System Settings") rather than as a hard failure.
enum PermissionState: Equatable {
  case granted
  case denied
  case notDetermined

  var isGranted: Bool { self == .granted }

  /// Short mono label shown on the right of a permission row.
  var label: String {
    label(locale: Locale(identifier: Bundle.main.preferredLocalizations.first ?? "en"))
  }

  func label(locale: Locale) -> String {
    switch self {
    case .granted:
      return String(
        localized: LocalizedStringResource(
          "Granted", locale: locale, comment: "Permission row status"))
    case .denied:
      return String(
        localized: LocalizedStringResource(
          "Denied", locale: locale, comment: "Permission row status"))
    case .notDetermined:
      return String(
        localized: LocalizedStringResource(
          "Not determined", locale: locale,
          comment: "Permission row status: the user has not decided yet"
        ))
    }
  }
}

/// Native privacy scopes. Screen Recording and Full Disk Access are optional
/// feature grants; dictation and hotkey scopes retain their specific purposes.
enum PermissionKind: String, CaseIterable, Identifiable {
  case microphone = "Microphone"
  case accessibility = "Accessibility"
  case inputMonitoring = "Input Monitoring"
  case screenRecording = "Screen Recording"
  case speechRecognition = "Speech Recognition"
  case fullDiskAccess = "Full Disk Access"

  var id: String { rawValue }

  var isOptionalForSetup: Bool {
    self == .screenRecording || self == .fullDiskAccess
  }

  /// Grants setup cannot finish without. Mirrors `REQUIRED_SETUP_PERMISSIONS`
  /// in app/os/onboarding.rs: a missing one removes `setup_done` on the next
  /// launch, so setup reporting success without it would be undone at once.
  var isRequiredForSetup: Bool { !isOptionalForSetup }

  /// macOS can keep a running process untrusted for these scopes until it
  /// reopens, even after the switch in System Settings is on. The probe stays
  /// the truth; this only decides when to say so.
  var grantMayNeedReopen: Bool {
    self == .accessibility || self == .inputMonitoring || self == .screenRecording
  }

  /// One sentence for a missing grant the row's own button cannot fix: a
  /// denial macOS never asks about again, or a grant requested in this process
  /// that macOS may apply only after Codescribe reopens. `nil` otherwise.
  func repairNote(for state: PermissionState, requested: Bool, locale: Locale) -> String? {
    guard !state.isGranted else { return nil }
    if state == .denied, self == .microphone || self == .speechRecognition {
      return String(
        localized: LocalizedStringResource(
          "macOS will not ask again. Turn on Codescribe in System Settings.", locale: locale,
          comment: "Denied Microphone or Speech Recognition; System Settings is the macOS app"))
    }
    if requested, grantMayNeedReopen {
      return String(
        localized: LocalizedStringResource(
          "Already turned on? macOS may apply it only after you quit and reopen Codescribe.",
          locale: locale,
          comment: "Permission still missing after a request; macOS may need the app to restart"))
    }
    return nil
  }

  /// Display name for the privacy scope. The identity (`rawValue`) is the
  /// System Settings pane name and must never reach the screen directly.
  var displayName: String {
    displayName(locale: Locale(identifier: Bundle.main.preferredLocalizations.first ?? "en"))
  }

  func displayName(
    locale: Locale,
    osMajorVersion: Int = ProcessInfo.processInfo.operatingSystemVersion.majorVersion
  ) -> String {
    switch self {
    case .microphone: String(localized: LocalizedStringResource("Microphone", locale: locale))
    case .accessibility:
      if osMajorVersion >= 27 {
        String(localized: LocalizedStringResource("Device Control and Data Access", locale: locale))
      } else {
        String(localized: LocalizedStringResource("Accessibility", locale: locale))
      }
    case .inputMonitoring:
      String(localized: LocalizedStringResource("Input Monitoring", locale: locale))
    case .screenRecording:
      String(localized: LocalizedStringResource("Screen Recording", locale: locale))
    case .speechRecognition:
      String(localized: LocalizedStringResource("Speech Recognition", locale: locale))
    case .fullDiskAccess:
      String(localized: LocalizedStringResource("Full Disk Access", locale: locale))
    }
  }

  /// Deep-link into the matching System Settings privacy pane.
  var settingsURL: URL? {
    let base = "x-apple.systempreferences:com.apple.preference.security?"
    switch self {
    case .microphone: return URL(string: base + "Privacy_Microphone")
    case .accessibility: return URL(string: base + "Privacy_Accessibility")
    case .inputMonitoring: return URL(string: base + "Privacy_ListenEvent")
    case .screenRecording: return URL(string: base + "Privacy_ScreenCapture")
    case .speechRecognition: return URL(string: base + "Privacy_SpeechRecognition")
    case .fullDiskAccess: return URL(string: base + "Privacy_AllFiles")
    }
  }

  func openSystemSettings() {
    guard let url = settingsURL else { return }
    NSWorkspace.shared.open(url)
  }

  /// The Privacy & Security pane itself, for reviewing scopes that are already
  /// granted (the per-row deep links only appear while a scope is missing).
  static let privacySettingsURL = URL(
    string: "x-apple.systempreferences:com.apple.preference.security?Privacy")!

  static func openPrivacySettings() {
    NSWorkspace.shared.open(privacySettingsURL)
  }

  /// What a click may do for this observed state.
  ///
  /// A local history marker cannot prove TCC state after a reset. Observed
  /// authorization stays on the probe; eligibility does not depend on history.
  func repair(for observed: PermissionState) -> PermissionRepair {
    guard !observed.isGranted else {
      return PermissionRepair(requestsNatively: false, opensSettings: false)
    }
    switch self {
    case .microphone, .speechRecognition:
      if observed == .notDetermined {
        return PermissionRepair(requestsNatively: true, opensSettings: false)
      }
      return PermissionRepair(requestsNatively: false, opensSettings: true)
    case .screenRecording, .inputMonitoring, .accessibility:
      return PermissionRepair(requestsNatively: true, opensSettings: true)
    case .fullDiskAccess:
      return PermissionRepair(requestsNatively: false, opensSettings: true)
    }
  }

  /// In-app request when `repair(for:)` says one is eligible.
  ///
  /// Defaults keep the real native calls. Tests pass a client and a probe so
  /// this never raises TCC. Accessibility's prompt returns before the user
  /// answers; that return is not a grant, and the following probe is the
  /// observed state. Screen Recording and Input Monitoring treat a `true`
  /// request result as the grant when the following probe is still denied.
  /// Microphone treats a granted probe or a `true` completion as granted.
  /// Speech Recognition prefers a granted probe, otherwise the request's
  /// `PermissionState`.
  func requestInApp(
    client: any PermissionRequesting = NativePermissionRequestClient(),
    probe: any PermissionProbing = NativePermissionProbe()
  ) async -> PermissionState {
    let observed = probe.snapshot().state(self)
    guard repair(for: observed).requestsNatively else { return observed }
    switch self {
    case .speechRecognition:
      let decided = await client.requestSpeechRecognition()
      let probed = probe.snapshot().speechRecognition
      return probed == .granted ? .granted : decided
    case .microphone:
      let granted = await client.requestMicrophone()
      let probed = probe.snapshot().microphone
      if probed == .granted || granted { return .granted }
      return probed
    case .screenRecording:
      let requested = client.requestScreenCapture()
      let probed = probe.snapshot().screenRecording
      if probed == .granted { return .granted }
      return requested ? .granted : probed
    case .inputMonitoring:
      let requested = client.requestInputMonitoring()
      let probed = probe.snapshot().inputMonitoring
      if probed == .granted { return .granted }
      return requested ? .granted : probed
    case .accessibility:
      _ = client.requestAccessibilityPrompt()
      return probe.snapshot().accessibility
    case .fullDiskAccess:
      return observed
    }
  }
}

struct PermissionRepair: Equatable, Sendable {
  var requestsNatively: Bool
  var opensSettings: Bool
}

/// Snapshot of all four scopes captured at one moment.
struct PermissionSnapshot: Equatable, Sendable {
  var microphone: PermissionState
  var accessibility: PermissionState
  var inputMonitoring: PermissionState
  var screenRecording: PermissionState
  /// Speech Recognition TCC scope (Apple live dictation). Defaulted so the
  /// existing call sites and tests that build snapshots keep compiling.
  var speechRecognition: PermissionState = .notDetermined
  /// Optional scope, probed only for the onboarding wizard. Settings never
  /// reads it (its matrix lists the dictation scopes explicitly).
  var fullDiskAccess: PermissionState = .notDetermined

  func state(_ kind: PermissionKind) -> PermissionState {
    switch kind {
    case .microphone: return microphone
    case .accessibility: return accessibility
    case .inputMonitoring: return inputMonitoring
    case .screenRecording: return screenRecording
    case .speechRecognition: return speechRecognition
    case .fullDiskAccess: return fullDiskAccess
    }
  }

  /// Mock value used by #Preview and the seeded view-model.
  static let allGranted = PermissionSnapshot(
    microphone: .granted,
    accessibility: .granted,
    inputMonitoring: .granted,
    screenRecording: .granted,
    speechRecognition: .granted,
    fullDiskAccess: .granted
  )
}

// MARK: - Probing

/// Abstraction so #Preview / tests can inject deterministic states without
/// touching the real system privacy database.
protocol PermissionProbing: Sendable {
  func snapshot() -> PermissionSnapshot
}

/// Native request calls. Production uses `NativePermissionRequestClient`.
/// A test passes its own client so `requestInApp` never raises a TCC prompt.
///
/// A `true` return is a grant only for the APIs whose completion is the
/// decision. `requestAccessibilityPrompt` is not one of them: the prompt is
/// asynchronous and the return does not mean the process is trusted.
protocol PermissionRequesting: Sendable {
  func requestScreenCapture() -> Bool
  func requestInputMonitoring() -> Bool
  func requestAccessibilityPrompt() -> Bool
  func requestMicrophone() async -> Bool
  func requestSpeechRecognition() async -> PermissionState
}

struct NativePermissionRequestClient: PermissionRequesting, Sendable {
  func requestScreenCapture() -> Bool {
    CGRequestScreenCaptureAccess()
  }

  func requestInputMonitoring() -> Bool {
    IOHIDRequestAccess(kIOHIDRequestTypeListenEvent)
  }

  func requestAccessibilityPrompt() -> Bool {
    // This is the public AX options dictionary key. The SDK exports the
    // equivalent kAXTrustedCheckOptionPrompt as an unannotated mutable global.
    let options = ["AXTrustedCheckOptionPrompt": true] as CFDictionary
    return AXIsProcessTrustedWithOptions(options)
  }

  func requestMicrophone() async -> Bool {
    await withCheckedContinuation { continuation in
      AVCaptureDevice.requestAccess(for: .audio) { granted in
        continuation.resume(returning: granted)
      }
    }
  }

  func requestSpeechRecognition() async -> PermissionState {
    await SpeechRecognitionPermission.request()
  }
}

/// Live system probe. Reads — never prompts.
struct NativePermissionProbe: PermissionProbing, Sendable {
  func snapshot() -> PermissionSnapshot {
    PermissionSnapshot(
      microphone: microphoneState(),
      accessibility: AXIsProcessTrusted() ? .granted : .denied,
      inputMonitoring: inputMonitoringState(),
      screenRecording: CGPreflightScreenCaptureAccess() ? .granted : .denied,
      speechRecognition: speechRecognitionState(),
      fullDiskAccess: fullDiskAccessState()
    )
  }

  private func speechRecognitionState() -> PermissionState {
    switch SFSpeechRecognizer.authorizationStatus() {
    case .authorized: return .granted
    case .notDetermined: return .notDetermined
    case .denied, .restricted: return .denied
    @unknown default: return .denied
    }
  }

  /// Heuristic Full Disk Access probe mirroring the core's
  /// `full_disk_access_status` (app/os/permissions.rs): try to list a handful
  /// of TCC-protected roots. A successful read means granted; an explicit
  /// permission error means denied; otherwise the paths are simply absent on
  /// this machine and the scope is treated as not-yet-determined. Never prompts.
  private func fullDiskAccessState() -> PermissionState {
    let home = FileManager.default.homeDirectoryForCurrentUser
    let protectedRoots = ["Library/Mail", "Library/Messages", "Library/Safari"]
    var sawPermissionDenied = false
    for relative in protectedRoots {
      let path = home.appendingPathComponent(relative).path
      do {
        _ = try FileManager.default.contentsOfDirectory(atPath: path)
        return .granted
      } catch let error as NSError {
        if error.domain == NSCocoaErrorDomain,
          error.code == NSFileReadNoPermissionError
        {
          sawPermissionDenied = true
        }
        // Absent path (NSFileReadNoSuchFileError) → keep probing.
      }
    }
    return sawPermissionDenied ? .denied : .notDetermined
  }

  private func microphoneState() -> PermissionState {
    switch AVCaptureDevice.authorizationStatus(for: .audio) {
    case .authorized: return .granted
    case .notDetermined: return .notDetermined
    case .denied, .restricted: return .denied
    @unknown default: return .denied
    }
  }

  private func inputMonitoringState() -> PermissionState {
    switch IOHIDCheckAccess(kIOHIDRequestTypeListenEvent) {
    case kIOHIDAccessTypeGranted: return .granted
    case kIOHIDAccessTypeUnknown: return .notDetermined
    default: return .denied
    }
  }
}

/// Mock probe for previews and tests. The production probe stays native.
struct MockPermissionProbe: PermissionProbing, Sendable {
  let value: PermissionSnapshot
  init(_ value: PermissionSnapshot = .allGranted) { self.value = value }
  func snapshot() -> PermissionSnapshot { value }
}

// MARK: - Speech Recognition request (main-process TCC dialog)

/// The one deliberate exception to the "reads — never prompts" probe doctrine:
/// Speech Recognition must be REQUESTED from the main app process so the grant
/// lands on `com.vetcoders.codescribe` (the bridge child inherits it). Prompting
/// from the CLI bridge grants the terminal's identity instead — the exact trap
/// this helper closes. Only fires the system dialog while `.notDetermined`;
/// afterwards macOS never re-prompts and the caller should deep-link instead.
enum SpeechRecognitionPermission {
  static func request() async -> PermissionState {
    await withCheckedContinuation { continuation in
      SFSpeechRecognizer.requestAuthorization { status in
        let state: PermissionState
        switch status {
        case .authorized: state = .granted
        case .notDetermined: state = .notDetermined
        case .denied, .restricted: state = .denied
        @unknown default: state = .denied
        }
        continuation.resume(returning: state)
      }
    }
  }
}
