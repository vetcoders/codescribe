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
    use core_foundation::number::CFNumber;

    // Create options dictionary with kAXTrustedCheckOptionPrompt = true
    let key = CFString::new("AXTrustedCheckOptionPrompt");
    let value = CFNumber::from(1i32); // true

    let options = CFDictionary::from_CFType_pairs(&[(key.as_CFType(), value.as_CFType())]);

    unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef() as *const _) }
}

#[cfg(not(target_os = "macos"))]
pub fn request_accessibility() -> bool {
    true // Not needed on other platforms
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
#[allow(dead_code)]
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

    // Microphone permission is requested automatically when we first access the mic
    // through cpal, so we just log the status
    check_all_permissions();
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
