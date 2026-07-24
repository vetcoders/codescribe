import Foundation
import AVFoundation
import ApplicationServices
import CoreGraphics
import IOKit.hid
import AppKit
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
        switch self {
        case .granted: return "granted"
        case .denied: return "denied"
        case .notDetermined: return "not determined"
        }
    }
}

/// The privacy scopes codescribe touches. The first five gate live dictation /
/// hotkeys and appear in the Settings engine matrix (which enumerates them by an
/// explicit list, NOT `allCases`); `fullDiskAccess` is the optional scope used
/// only by the first-run onboarding wizard, so adding it here does not change
/// any Settings surface. `speechRecognition` is the TCC scope behind Apple live
/// dictation (`SFSpeechRecognizer`) — the bridge child process inherits the
/// app's grant, so the main app must own request + display.
enum PermissionKind: String, CaseIterable, Identifiable {
    case microphone = "Microphone"
    case accessibility = "Accessibility"
    case inputMonitoring = "Input Monitoring"
    case screenRecording = "Screen Recording"
    case speechRecognition = "Speech Recognition"
    case fullDiskAccess = "Full Disk Access"

    var id: String { rawValue }

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

    /// Scopes that can fire a first-run system dialog from our process. Once
    /// the user has decided (granted/denied), macOS never re-prompts — callers
    /// must deep-link to System Settings instead.
    var supportsInAppPermissionRequest: Bool {
        switch self {
        case .microphone, .speechRecognition, .screenRecording, .inputMonitoring:
            return true
        case .accessibility, .fullDiskAccess:
            // Accessibility and Full Disk Access are only toggled in System Settings.
            return false
        }
    }

    /// Fire the in-app TCC dialog when still undetermined. Always calls
    /// `completion` on the main queue with the post-request state (or the
    /// preflight state when the scope cannot request in-app).
    func requestInApp(completion: @escaping (PermissionState) -> Void) {
        switch self {
        case .speechRecognition:
            SpeechRecognitionPermission.request(completion: completion)
        case .microphone:
            AVCaptureDevice.requestAccess(for: .audio) { granted in
                DispatchQueue.main.async {
                    completion(granted ? .granted : NativePermissionProbe().snapshot().microphone)
                }
            }
        case .screenRecording:
            let ok = CGRequestScreenCaptureAccess()
            DispatchQueue.main.async {
                completion(ok ? .granted : NativePermissionProbe().snapshot().screenRecording)
            }
        case .inputMonitoring:
            let ok = IOHIDRequestAccess(kIOHIDRequestTypeListenEvent)
            DispatchQueue.main.async {
                completion(ok ? .granted : NativePermissionProbe().snapshot().inputMonitoring)
            }
        case .accessibility, .fullDiskAccess:
            DispatchQueue.main.async {
                completion(NativePermissionProbe().snapshot().state(self))
            }
        }
    }
}

/// Snapshot of all four scopes captured at one moment.
struct PermissionSnapshot: Equatable {
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
protocol PermissionProbing {
    func snapshot() -> PermissionSnapshot
}

/// Live system probe. Reads — never prompts.
struct NativePermissionProbe: PermissionProbing {
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
                    error.code == NSFileReadNoPermissionError {
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

/// Mock probe for previews.
struct MockPermissionProbe: PermissionProbing {
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
    static func request(completion: @escaping (PermissionState) -> Void) {
        SFSpeechRecognizer.requestAuthorization { status in
            let state: PermissionState
            switch status {
            case .authorized: state = .granted
            case .notDetermined: state = .notDetermined
            case .denied, .restricted: state = .denied
            @unknown default: state = .denied
            }
            DispatchQueue.main.async { completion(state) }
        }
    }
}
