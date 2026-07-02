import Foundation
import AVFoundation
import ApplicationServices
import CoreGraphics
import IOKit.hid
import AppKit

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

/// The privacy scopes codescribe touches. The first four gate live dictation /
/// hotkeys and appear in the Settings engine matrix (which enumerates them by an
/// explicit list, NOT `allCases`); `fullDiskAccess` is the optional fifth scope
/// used only by the first-run onboarding wizard, so adding it here does not
/// change any Settings surface.
enum PermissionKind: String, CaseIterable, Identifiable {
    case microphone = "Microphone"
    case accessibility = "Accessibility"
    case inputMonitoring = "Input Monitoring"
    case screenRecording = "Screen Recording"
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
        case .fullDiskAccess: return URL(string: base + "Privacy_AllFiles")
        }
    }

    func openSystemSettings() {
        guard let url = settingsURL else { return }
        NSWorkspace.shared.open(url)
    }
}

/// Snapshot of all four scopes captured at one moment.
struct PermissionSnapshot: Equatable {
    var microphone: PermissionState
    var accessibility: PermissionState
    var inputMonitoring: PermissionState
    var screenRecording: PermissionState
    /// Optional fifth scope, probed only for the onboarding wizard. Settings
    /// never reads it (its matrix lists the first four explicitly).
    var fullDiskAccess: PermissionState = .notDetermined

    func state(_ kind: PermissionKind) -> PermissionState {
        switch kind {
        case .microphone: return microphone
        case .accessibility: return accessibility
        case .inputMonitoring: return inputMonitoring
        case .screenRecording: return screenRecording
        case .fullDiskAccess: return fullDiskAccess
        }
    }

    /// Mock value used by #Preview and the seeded view-model.
    static let allGranted = PermissionSnapshot(
        microphone: .granted,
        accessibility: .granted,
        inputMonitoring: .granted,
        screenRecording: .granted,
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
            fullDiskAccess: fullDiskAccessState()
        )
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
