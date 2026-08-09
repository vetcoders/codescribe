// permissions.rs
//
// Purpose: Check and request macOS permissions for Accessibility and Microphone
//
// On macOS, apps need explicit user permission for:
// - Accessibility: Required for global hotkeys (key event monitoring)
// - Microphone: Required for audio recording
//
// This module provides functions to check permission status and prompt the user
// to grant permissions in System Settings if not already granted.

#[cfg(target_os = "macos")]
use block2::RcBlock;
#[cfg(target_os = "macos")]
use core_foundation::base::TCFType;
#[cfg(target_os = "macos")]
use core_foundation::string::CFString;
#[cfg(target_os = "macos")]
use dispatch::Queue;
#[cfg(target_os = "macos")]
use objc::{msg_send, runtime::Class, sel, sel_impl};
#[cfg(target_os = "macos")]
use objc2::runtime::Bool;
#[cfg(target_os = "macos")]
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
#[cfg(target_os = "macos")]
use std::time::{Duration, Instant};

/// Permission status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionStatus {
    /// Permission is granted
    Granted,
    /// Permission is denied
    Denied,
    /// Permission not yet requested (user hasn't been asked)
    NotDetermined,
}

/// The macOS permission classes Codescribe probes at startup and during
/// onboarding. Relocated here (out of `app/ui/onboarding/steps`) so the
/// non-UI permission model lives next to the `check_*` probes it drives and
/// survives the legacy AppKit UI excision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionKind {
    /// `AVCaptureDevice` audio TCC — required for any capture.
    Microphone,
    /// `AXIsProcessTrusted` — required for global hotkey registration.
    Accessibility,
    /// `CGPreflightListenEventAccess` — required for the global key-event tap.
    InputMonitoring,
    /// `CGPreflightScreenCaptureAccess` — required for screen capture.
    ScreenRecording,
    /// SFSpeechRecognizer TCC — required for Apple live dictation.
    SpeechRecognition,
    /// Full Disk Access — inferred by probing, since macOS exposes no preflight.
    FullDiskAccess,
}

/// Probe the live status of a single permission class. Dispatches to the
/// per-permission `check_*` probes in this module.
pub fn permission_status(kind: PermissionKind) -> PermissionStatus {
    match kind {
        PermissionKind::Microphone => check_microphone(),
        PermissionKind::Accessibility => check_accessibility(),
        PermissionKind::InputMonitoring => check_input_monitoring(),
        PermissionKind::ScreenRecording => check_screen_recording(),
        PermissionKind::SpeechRecognition => check_speech_recognition(),
        PermissionKind::FullDiskAccess => check_full_disk_access(),
    }
}

/// Check if Accessibility permission is granted
///
/// Accessibility permission is required for global hotkeys to work.
/// If not granted, hotkeys will silently fail to register.
#[cfg(target_os = "macos")]
pub fn check_accessibility() -> PermissionStatus {
    // Use AXIsProcessTrusted() from ApplicationServices
    // This returns true if the app has Accessibility permission
    unsafe extern "C" {
        /// ApplicationServices: true when this process is a trusted Accessibility
        /// client. Does not prompt; reflects the current TCC decision only.
        fn AXIsProcessTrusted() -> bool;
    }

    unsafe {
        if AXIsProcessTrusted() {
            PermissionStatus::Granted
        } else {
            PermissionStatus::Denied
        }
    }
}

/// Non-macOS stub: there is no Accessibility TCC gate, so hotkeys are always
/// allowed to register.
#[cfg(not(target_os = "macos"))]
pub fn check_accessibility() -> PermissionStatus {
    PermissionStatus::Granted // Not needed on other platforms
}

/// Check if Input Monitoring permission is granted (macOS)
///
/// This permission gates global key event listening (including CGEventTap in listen-only mode).
#[cfg(target_os = "macos")]
pub fn check_input_monitoring() -> PermissionStatus {
    unsafe extern "C" {
        /// CoreGraphics: true when this process may listen to global key events.
        /// Preflight only — it never raises the Input Monitoring prompt.
        fn CGPreflightListenEventAccess() -> bool;
    }

    unsafe {
        if CGPreflightListenEventAccess() {
            PermissionStatus::Granted
        } else {
            PermissionStatus::Denied
        }
    }
}

/// Non-macOS stub: global key-event listening is not TCC-gated off macOS.
#[cfg(not(target_os = "macos"))]
pub fn check_input_monitoring() -> PermissionStatus {
    PermissionStatus::Granted
}

/// Check Speech Recognition TCC (`SFSpeechRecognizer.authorizationStatus`).
/// Required for Apple live dictation via the STT bridge.
#[cfg(target_os = "macos")]
pub fn check_speech_recognition() -> PermissionStatus {
    unsafe {
        let Some(sf_class) = Class::get("SFSpeechRecognizer") else {
            return PermissionStatus::NotDetermined;
        };
        // SFSpeechRecognizerAuthorizationStatus:
        // 0 notDetermined, 1 denied, 2 restricted, 3 authorized
        let status: isize = msg_send![sf_class, authorizationStatus];
        match status {
            3 => PermissionStatus::Granted,
            1 | 2 => PermissionStatus::Denied,
            _ => PermissionStatus::NotDetermined,
        }
    }
}

/// Non-macOS stub: `SFSpeechRecognizer` is an Apple-only surface, so nothing
/// gates dictation here.
#[cfg(not(target_os = "macos"))]
pub fn check_speech_recognition() -> PermissionStatus {
    PermissionStatus::Granted
}

/// Check if Microphone permission is granted
///
/// Microphone permission is required for audio recording.
/// Uses `AVCaptureDevice.authorizationStatusForMediaType("soun")`.
#[cfg(target_os = "macos")]
pub fn check_microphone() -> PermissionStatus {
    unsafe {
        let Some(av_class) = Class::get("AVCaptureDevice") else {
            return PermissionStatus::NotDetermined;
        };

        // AVMediaTypeAudio fourcc
        let media_type = CFString::new("soun");
        let status: isize =
            msg_send![av_class, authorizationStatusForMediaType: media_type.as_concrete_TypeRef()];
        match status {
            3 => PermissionStatus::Granted,    // AVAuthorizationStatusAuthorized
            1 | 2 => PermissionStatus::Denied, // Restricted / Denied
            _ => PermissionStatus::NotDetermined,
        }
    }
}

/// Non-macOS stub: capture devices are not TCC-gated off macOS.
#[cfg(not(target_os = "macos"))]
pub fn check_microphone() -> PermissionStatus {
    PermissionStatus::Granted // Not needed on other platforms
}

/// Wall-clock budget for the microphone TCC prompt wait (30s).
#[cfg(target_os = "macos")]
const MICROPHONE_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Poll interval while waiting for mic authorization to settle.
#[cfg(target_os = "macos")]
const MICROPHONE_STATUS_POLL_INTERVAL: Duration = Duration::from_millis(250);
/// Timeout waiting for main-thread dispatch of TCC-sensitive work.
#[cfg(target_os = "macos")]
const MAIN_THREAD_DISPATCH_TIMEOUT: Duration = Duration::from_secs(2);

/// True when the caller is on the AppKit main thread.
///
/// Asks `NSThread` first and falls back to the Rust thread name, because the
/// microphone prompt must be armed from the main thread but this module is also
/// reachable from worker threads.
#[cfg(target_os = "macos")]
fn is_main_thread() -> bool {
    unsafe {
        if let Some(ns_thread) = Class::get("NSThread") {
            msg_send![ns_thread, isMainThread]
        } else {
            std::thread::current().name() == Some("main")
        }
    }
}

/// Fire `AVCaptureDevice.requestAccessForMediaType` and forward the completion
/// result over `callback_tx`.
///
/// Returns whether the request was *started*, not whether access was granted —
/// the grant arrives asynchronously on the channel.
#[cfg(target_os = "macos")]
fn start_microphone_request(callback_tx: Sender<bool>) -> bool {
    use tracing::warn;

    let Some(av_class) = Class::get("AVCaptureDevice") else {
        warn!("Microphone request failed: AVCaptureDevice class unavailable.");
        return false;
    };

    let media_type = CFString::new("soun");
    unsafe {
        let request_block: RcBlock<dyn Fn(Bool)> = RcBlock::new(move |granted: Bool| {
            let _ = callback_tx.send(granted.as_bool());
        });

        let _: () = msg_send![
            av_class,
            requestAccessForMediaType: media_type.as_concrete_TypeRef()
            completionHandler: &*request_block
        ];
    }

    true
}

/// Start the microphone request on the main thread, hopping there via
/// `Queue::main()` when the caller is elsewhere.
///
/// The hop is bounded by `MAIN_THREAD_DISPATCH_TIMEOUT`: a wedged or
/// not-yet-running main queue reports "not started" instead of blocking the
/// caller forever.
#[cfg(target_os = "macos")]
fn start_microphone_request_on_main_thread(callback_tx: Sender<bool>) -> bool {
    use tracing::warn;

    if is_main_thread() {
        return start_microphone_request(callback_tx);
    }

    let (started_tx, started_rx) = mpsc::channel();
    Queue::main().exec_async(move || {
        let started = start_microphone_request(callback_tx);
        let _ = started_tx.send(started);
    });

    match started_rx.recv_timeout(MAIN_THREAD_DISPATCH_TIMEOUT) {
        Ok(started) => started,
        Err(RecvTimeoutError::Timeout) => {
            warn!(
                "Microphone request dispatch timed out waiting for main thread (>{:?}).",
                MAIN_THREAD_DISPATCH_TIMEOUT
            );
            false
        }
        Err(RecvTimeoutError::Disconnected) => {
            warn!("Microphone request dispatch failed: main-thread handoff channel closed.");
            false
        }
    }
}

/// Block until the microphone grant resolves, or until
/// `MICROPHONE_REQUEST_TIMEOUT` elapses.
///
/// The completion block is not trusted as the only signal: the wait is
/// interleaved with `check_microphone()` polls every
/// `MICROPHONE_STATUS_POLL_INTERVAL`, so a callback that never fires, reports a
/// stale `false`, or has its channel dropped still resolves to the live TCC
/// status rather than hanging.
#[cfg(target_os = "macos")]
fn wait_for_microphone_resolution(callback_rx: Receiver<bool>) -> bool {
    use tracing::{info, warn};

    let started = Instant::now();
    loop {
        let elapsed = started.elapsed();
        if elapsed >= MICROPHONE_REQUEST_TIMEOUT {
            break;
        }

        let remaining = MICROPHONE_REQUEST_TIMEOUT - elapsed;
        let wait_for = remaining.min(MICROPHONE_STATUS_POLL_INTERVAL);

        match callback_rx.recv_timeout(wait_for) {
            Ok(granted) => {
                if granted {
                    info!("Microphone permission granted by system callback.");
                    return true;
                }

                let status = check_microphone();
                if status == PermissionStatus::Granted {
                    info!("Microphone callback reported false, but status is now Granted.");
                    return true;
                }

                warn!(
                    "Microphone permission denied. Enable Codescribe in System Settings > Privacy & Security > Microphone."
                );
                return false;
            }
            Err(RecvTimeoutError::Timeout) => match check_microphone() {
                PermissionStatus::Granted => {
                    info!("Microphone permission became Granted while waiting for callback.");
                    return true;
                }
                PermissionStatus::Denied => {
                    warn!(
                        "Microphone permission is denied/restricted. Enable Codescribe in System Settings > Privacy & Security > Microphone."
                    );
                    return false;
                }
                PermissionStatus::NotDetermined => {}
            },
            Err(RecvTimeoutError::Disconnected) => {
                let status = check_microphone();
                warn!(
                    "Microphone callback channel closed before completion (status: {:?}).",
                    status
                );
                return status == PermissionStatus::Granted;
            }
        }
    }

    let status = check_microphone();
    warn!(
        "Timed out waiting {:?} for microphone permission result (status: {:?}). Open System Settings > Privacy & Security > Microphone if needed.",
        MICROPHONE_REQUEST_TIMEOUT, status
    );
    status == PermissionStatus::Granted
}

/// Request Microphone permission
///
/// Shows system dialog asking user to grant microphone access.
/// Returns true when access is granted.
#[cfg(target_os = "macos")]
pub fn request_microphone() -> bool {
    use tracing::{info, warn};

    match check_microphone() {
        PermissionStatus::Granted => return true,
        PermissionStatus::Denied => {
            warn!(
                "Microphone permission already denied/restricted. Grant access in System Settings > Privacy & Security > Microphone."
            );
            return false;
        }
        PermissionStatus::NotDetermined => {
            info!("Microphone permission not determined yet; requesting system prompt.");
        }
    }

    if is_main_thread() {
        info!(
            "request_microphone() is running on main thread; using bounded polling fallback to avoid hanging on callback delivery."
        );
    }

    let (callback_tx, callback_rx) = mpsc::channel();
    if !start_microphone_request_on_main_thread(callback_tx) {
        warn!("Microphone permission request could not be started.");
        return check_microphone() == PermissionStatus::Granted;
    }

    wait_for_microphone_resolution(callback_rx)
}

/// Non-macOS stub: nothing to request, capture is always permitted.
#[cfg(not(target_os = "macos"))]
pub fn request_microphone() -> bool {
    true
}

#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    /// CoreGraphics: true when Screen Recording is already granted. Never prompts.
    fn CGPreflightScreenCaptureAccess() -> bool;
    /// CoreGraphics: raise the Screen Recording prompt and report the outcome.
    fn CGRequestScreenCaptureAccess() -> bool;
}

// Ensures `SFSpeechRecognizer` is available for `check_speech_recognition`.
#[cfg(target_os = "macos")]
#[link(name = "Speech", kind = "framework")]
unsafe extern "C" {}

/// Check screen recording permission status.
#[cfg(target_os = "macos")]
pub fn check_screen_recording() -> PermissionStatus {
    if unsafe { CGPreflightScreenCaptureAccess() } {
        PermissionStatus::Granted
    } else {
        // macOS preflight only reports granted/not-granted and does not reliably
        // distinguish "never requested" from "denied". Keep this conservative.
        PermissionStatus::NotDetermined
    }
}

/// Non-macOS stub: screen capture is not TCC-gated off macOS.
#[cfg(not(target_os = "macos"))]
pub fn check_screen_recording() -> PermissionStatus {
    PermissionStatus::Granted
}

/// Request screen recording permission. Returns true when granted.
#[cfg(target_os = "macos")]
pub fn request_screen_recording() -> bool {
    unsafe { CGRequestScreenCaptureAccess() }
}

/// Non-macOS stub: nothing to request, screen capture is always permitted.
#[cfg(not(target_os = "macos"))]
pub fn request_screen_recording() -> bool {
    true
}

/// Check Full Disk Access permission status.
#[cfg(target_os = "macos")]
pub fn check_full_disk_access() -> PermissionStatus {
    full_disk_access_status()
}

/// Non-macOS stub: there is no Full Disk Access class off macOS.
#[cfg(not(target_os = "macos"))]
pub fn check_full_disk_access() -> PermissionStatus {
    PermissionStatus::Granted
}

/// Infer Full Disk Access by probing TCC-protected directories under `$HOME`.
///
/// macOS exposes no preflight API for this class, so the status is read from
/// behaviour: a readable protected root proves `Granted`, an `ErrorKind::
/// PermissionDenied` proves `Denied`, and anything else (missing paths, empty
/// `$HOME`) stays `NotDetermined` rather than guessing.
#[cfg(target_os = "macos")]
fn full_disk_access_status() -> PermissionStatus {
    use std::path::Path;

    let home = std::env::var("HOME").unwrap_or_default();
    if home.is_empty() {
        return PermissionStatus::NotDetermined;
    }

    let protected_roots = [
        Path::new(&home).join("Library/Mail"),
        Path::new(&home).join("Library/Messages"),
        Path::new(&home).join("Library/Safari"),
    ];

    let mut saw_permission_denied = false;
    for path in protected_roots {
        match std::fs::read_dir(&path) {
            Ok(_) => return PermissionStatus::Granted,
            Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
                saw_permission_denied = true;
            }
            Err(_) => continue,
        }
    }

    if saw_permission_denied {
        PermissionStatus::Denied
    } else {
        // Could be "not requested yet" or paths absent on this machine.
        PermissionStatus::NotDetermined
    }
}

/// PermissionStatus equality smoke tests (no live TCC dialogs).
#[cfg(test)]
mod tests {
    use super::*;

    /// Granted equals itself and differs from Denied.
    #[test]
    fn test_permission_status_equality() {
        assert_eq!(PermissionStatus::Granted, PermissionStatus::Granted);
        assert_ne!(PermissionStatus::Granted, PermissionStatus::Denied);
    }
}
