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

    use core_foundation::dictionary::CFDictionary;
    use core_foundation::boolean::CFBoolean;

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
/// Note: On macOS, the permission is checked when cpal first accesses the microphone.
/// We return NotDetermined here to indicate it will be requested on first use.
#[cfg(target_os = "macos")]
pub fn check_microphone() -> PermissionStatus {
    // AVCaptureDevice requires AVFoundation framework which isn't linked by default
    // Microphone permission will be requested automatically when cpal accesses the mic
    // For now, we just return NotDetermined to indicate it hasn't been checked yet
    PermissionStatus::NotDetermined
}

#[cfg(not(target_os = "macos"))]
pub fn check_microphone() -> PermissionStatus {
    PermissionStatus::Granted // Not needed on other platforms
}

/// Request Microphone permission
///
/// Shows system dialog asking user to grant microphone access.
/// The callback will be called with the result.
#[cfg(target_os = "macos")]
pub fn request_microphone() -> bool {
    // For now, we'll just check if we can access the default input device
    // The actual permission dialog will be triggered when we first try to use the microphone
    // through cpal

    // Return true to indicate we attempted to request (actual permission granted via cpal)
    true
}

#[cfg(not(target_os = "macos"))]
pub fn request_microphone() -> bool {
    true
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
            info!("Microphone permission: Will be requested on first use");
        }
        PermissionStatus::Denied => {
            warn!("Microphone permission: DENIED - Recording will not work!");
            warn!("Grant access in: System Settings > Privacy & Security > Microphone");
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

    // Microphone permission is requested automatically when we first access the mic
    // through cpal, so we just log the status
    check_all_permissions();
}

pub fn diagnostics_report() -> String {
    use std::fmt::Write;

    let mut out = String::new();
    let _ = writeln!(&mut out, "CodeScribe diagnostics");
    let _ = writeln!(
        &mut out,
        "pid: {}",
        std::process::id()
    );
    let _ = writeln!(
        &mut out,
        "exe: {}",
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "<unknown>".to_string())
    );

    if let Some(bundle_id) = current_bundle_identifier() {
        let _ = writeln!(&mut out, "bundle_id: {}", bundle_id);
    }

    let _ = writeln!(&mut out, "accessibility: {:?}", check_accessibility());
    let _ = writeln!(&mut out, "input_monitoring: {:?}", check_input_monitoring());

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
