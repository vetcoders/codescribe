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
use core_foundation::base::TCFType;
#[cfg(target_os = "macos")]
use core_foundation::string::CFString;
#[cfg(target_os = "macos")]
use dispatch::Queue;
#[cfg(target_os = "macos")]
use objc::{msg_send, runtime::Class, sel, sel_impl};
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

/// Check if Accessibility permission is granted
///
/// Accessibility permission is required for global hotkeys to work.
/// If not granted, hotkeys will silently fail to register.
#[cfg(target_os = "macos")]
pub fn check_accessibility() -> PermissionStatus {
    // Use AXIsProcessTrusted() from ApplicationServices
    // This returns true if the app has Accessibility permission
    unsafe extern "C" {
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

#[cfg(not(target_os = "macos"))]
pub fn check_input_monitoring() -> PermissionStatus {
    PermissionStatus::Granted
}

/// Prompt user to grant Accessibility permission
///
/// Opens System Settings > Privacy & Security > Accessibility
/// Returns true if the prompt was shown successfully
#[cfg(target_os = "macos")]
pub fn request_accessibility() -> bool {
    // Use AXIsProcessTrustedWithOptions() to show the system prompt
    unsafe extern "C" {
        fn AXIsProcessTrustedWithOptions(options: *const std::ffi::c_void) -> bool;
    }

    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;

    // Create options dictionary with kAXTrustedCheckOptionPrompt = true
    let key = CFString::new("AXTrustedCheckOptionPrompt");
    let value = CFBoolean::true_value();

    let options = CFDictionary::from_CFType_pairs(&[(key.as_CFType(), value.as_CFType())]);

    unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef() as *const _) }
}

#[cfg(not(target_os = "macos"))]
pub fn request_accessibility() -> bool {
    true // Not needed on other platforms
}

/// Request Input Monitoring permission (macOS)
///
/// Shows system prompt asking to allow key event listening.
#[cfg(target_os = "macos")]
pub fn request_input_monitoring() -> bool {
    unsafe extern "C" {
        fn CGRequestListenEventAccess() -> bool;
    }

    unsafe { CGRequestListenEventAccess() }
}

#[cfg(not(target_os = "macos"))]
pub fn request_input_monitoring() -> bool {
    true
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

#[cfg(not(target_os = "macos"))]
pub fn check_microphone() -> PermissionStatus {
    PermissionStatus::Granted // Not needed on other platforms
}

#[cfg(target_os = "macos")]
const MICROPHONE_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(target_os = "macos")]
const MICROPHONE_STATUS_POLL_INTERVAL: Duration = Duration::from_millis(250);
#[cfg(target_os = "macos")]
const MAIN_THREAD_DISPATCH_TIMEOUT: Duration = Duration::from_secs(2);

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

#[cfg(target_os = "macos")]
fn start_microphone_request(callback_tx: Sender<bool>) -> bool {
    use tracing::warn;

    let Some(av_class) = Class::get("AVCaptureDevice") else {
        warn!("Microphone request failed: AVCaptureDevice class unavailable.");
        return false;
    };

    let media_type = CFString::new("soun");
    unsafe {
        let request_block = block::ConcreteBlock::new(move |granted: bool| {
            let _ = callback_tx.send(granted);
        })
        .copy();

        let _: () = msg_send![
            av_class,
            requestAccessForMediaType: media_type.as_concrete_TypeRef()
            completionHandler: &*request_block
        ];
    }

    true
}

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
                    "Microphone permission denied. Enable CodeScribe in System Settings > Privacy & Security > Microphone."
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
                        "Microphone permission is denied/restricted. Enable CodeScribe in System Settings > Privacy & Security > Microphone."
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

#[cfg(not(target_os = "macos"))]
pub fn request_microphone() -> bool {
    true
}

#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

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

#[cfg(not(target_os = "macos"))]
pub fn check_screen_recording() -> PermissionStatus {
    PermissionStatus::Granted
}

/// Request screen recording permission. Returns true when granted.
#[cfg(target_os = "macos")]
pub fn request_screen_recording() -> bool {
    unsafe { CGRequestScreenCaptureAccess() }
}

#[cfg(not(target_os = "macos"))]
pub fn request_screen_recording() -> bool {
    true
}

/// Check Full Disk Access permission status.
#[cfg(target_os = "macos")]
pub fn check_full_disk_access() -> PermissionStatus {
    full_disk_access_status()
}

#[cfg(not(target_os = "macos"))]
pub fn check_full_disk_access() -> PermissionStatus {
    PermissionStatus::Granted
}

/// Request Full Disk Access by opening the relevant System Settings pane.
#[cfg(target_os = "macos")]
pub fn request_full_disk_access() -> bool {
    if check_full_disk_access() == PermissionStatus::Granted {
        return true;
    }
    open_privacy_settings("Privacy_AllFiles");
    false
}

#[cfg(not(target_os = "macos"))]
pub fn request_full_disk_access() -> bool {
    true
}

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

#[cfg(target_os = "macos")]
pub fn open_privacy_settings(deeplink: &str) {
    let url = format!(
        "x-apple.systempreferences:com.apple.preference.security?{}",
        deeplink
    );
    let _ = std::process::Command::new("open").arg(url).spawn();
}

/// Check all required permissions and log status
pub fn check_all_permissions() {
    use tracing::{info, warn};

    // Check Accessibility
    match check_accessibility() {
        PermissionStatus::Granted => {
            info!("Accessibility permission: Granted");
        }
        PermissionStatus::Denied => {
            warn!("Accessibility permission: DENIED - Global hotkeys may not work!");
            warn!("Grant access in: System Settings > Privacy & Security > Accessibility");
        }
        _ => {
            warn!("Accessibility permission: Unknown status");
        }
    }

    // Check Input Monitoring
    match check_input_monitoring() {
        PermissionStatus::Granted => {
            info!("Input Monitoring permission: Granted");
        }
        PermissionStatus::Denied => {
            warn!("Input Monitoring permission: DENIED - Hotkeys may not work!");
            warn!("Grant access in: System Settings > Privacy & Security > Input Monitoring");
        }
        _ => {
            warn!("Input Monitoring permission: Unknown status");
        }
    }

    // Check Microphone
    match check_microphone() {
        PermissionStatus::Granted => {
            info!("Microphone permission: Granted");
        }
        PermissionStatus::NotDetermined => {
            info!(
                "Microphone permission: Not determined (macOS prompt may appear on first recording attempt)."
            );
            info!(
                "If recording does not start, open System Settings > Privacy & Security > Microphone and enable CodeScribe."
            );
        }
        PermissionStatus::Denied => {
            warn!("Microphone permission: DENIED - Recording will not work!");
            warn!("Grant access in: System Settings > Privacy & Security > Microphone");
            warn!("After enabling access, restart CodeScribe if status does not refresh.");
        }
    }
}

/// Request all required permissions (with user prompts)
pub fn request_all_permissions() {
    use tracing::info;

    info!("Checking and requesting required permissions...");

    // Request Accessibility (shows system prompt if not granted)
    if check_accessibility() != PermissionStatus::Granted {
        info!("Requesting Accessibility permission...");
        request_accessibility();
    }

    // Request Input Monitoring (shows system prompt if not granted)
    if check_input_monitoring() != PermissionStatus::Granted {
        info!("Requesting Input Monitoring permission...");
        request_input_monitoring();
    }

    if check_microphone() != PermissionStatus::Granted {
        info!(
            "Microphone permission not granted yet; CodeScribe will request it when recording starts. If no prompt appears, open System Settings > Privacy & Security > Microphone."
        );
    }
}

pub fn diagnostics_report() -> String {
    use std::fmt::Write;

    let mut out = String::new();
    let _ = writeln!(&mut out, "CodeScribe diagnostics");
    let _ = writeln!(&mut out, "pid: {}", std::process::id());
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "<unknown>".to_string());
    let _ = writeln!(&mut out, "exe: {}", exe);

    if let Some(bundle_id) = current_bundle_identifier() {
        let _ = writeln!(&mut out, "bundle_id: {}", bundle_id);
    }

    let app_bundle = exe.contains(".app/Contents/MacOS/");
    let _ = writeln!(
        &mut out,
        "app_bundle: {}",
        if app_bundle { "yes" } else { "no" }
    );

    let _ = writeln!(&mut out, "accessibility: {:?}", check_accessibility());
    let _ = writeln!(&mut out, "input_monitoring: {:?}", check_input_monitoring());

    // Small, safe config hints (do not print secrets).
    for key in [
        "WHISPER_LANGUAGE",
        "HOLD_MODS",
        "HOLD_START_DELAY_MS",
        "DOUBLE_TAP_INTERVAL_MS",
        "TOGGLE_SILENCE_SEC",
        "TOGGLE_TRIGGER",
        "CODESCRIBE_BUFFERED_STREAM",
        "CODESCRIBE_STREAM_CHUNK_SEC",
    ] {
        if let Ok(val) = std::env::var(key) {
            let _ = writeln!(&mut out, "{key}: {val}");
        }
    }

    // Best-effort codesign info (helps debug TCC resets).
    #[cfg(target_os = "macos")]
    {
        let _ = writeln!(&mut out);
        let _ = writeln!(&mut out, "codesign:");
        if let Ok(output) = std::process::Command::new("codesign")
            .args(["-dv", "--verbose=2", &exe])
            .output()
        {
            let stderr = String::from_utf8_lossy(&output.stderr);
            for line in stderr.lines().take(40) {
                let _ = writeln!(&mut out, "  {}", line);
            }
        } else {
            let _ = writeln!(&mut out, "  <unavailable>");
        }
    }

    // Best-effort process list (helps spot stray CLI/daemon processes).
    #[cfg(target_os = "macos")]
    {
        let _ = writeln!(&mut out);
        let _ = writeln!(&mut out, "processes:");
        if let Ok(output) = std::process::Command::new("ps")
            .args(["-ax", "-o", "pid=,comm=,args="])
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout
                .lines()
                .filter(|l| l.to_lowercase().contains("codescribe"))
                .take(30)
            {
                let _ = writeln!(&mut out, "  {}", line.trim());
            }
        } else {
            let _ = writeln!(&mut out, "  <unavailable>");
        }
    }

    out
}

fn current_bundle_identifier() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    // If running from an .app bundle, Info.plist is usually at ../Info.plist.
    // Example: .../CodeScribe.app/Contents/MacOS/codescribe
    let info_plist = exe
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("Info.plist"))?;
    let content = std::fs::read_to_string(info_plist).ok()?;

    // Extremely small parser: find the first string after CFBundleIdentifier key.
    let key_idx = content.find("CFBundleIdentifier")?;
    let after_key = &content[key_idx..];
    let string_open = after_key.find("<string>")?;
    let after_open = &after_key[string_open + "<string>".len()..];
    let string_close = after_open.find("</string>")?;
    Some(after_open[..string_close].trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permission_status_equality() {
        assert_eq!(PermissionStatus::Granted, PermissionStatus::Granted);
        assert_ne!(PermissionStatus::Granted, PermissionStatus::Denied);
    }
}
