//! Operating-system integration surface: clipboard, global hotkeys, the hold
//! badge, notifications, onboarding and permission prompts, selection capture,
//! shortcut registration, thermal state, and tray status.
//!
//! Nearly everything here is `cfg`-gated to macOS, since it speaks to AppKit
//! and the accessibility APIs directly. `hold_badge` is the exception: it
//! carries its own non-macOS no-op stubs so callers need no `cfg` of their own.

#[cfg(target_os = "macos")]
pub mod clipboard;
// Cross-platform: macOS AppKit impl + non-macOS no-op stubs live in the module.
/// Hold-to-dictate badge window (macOS AppKit + non-macOS no-op stubs).
pub mod hold_badge;
#[cfg(target_os = "macos")]
/// Global hotkey CGEventTap listener, detector, and runtime config.
pub mod hotkeys;
#[cfg(target_os = "macos")]
/// User-facing notification delivery via the macOS notification center.
pub mod notifications;
#[cfg(target_os = "macos")]
/// First-run onboarding prompts for permissions and product orientation.
pub mod onboarding;
#[cfg(target_os = "macos")]
/// Accessibility / mic / speech TCC permission probes and request helpers.
pub mod permissions;
#[cfg(target_os = "macos")]
/// Frontmost-app text selection capture for dictation and agent context.
pub mod selection;
#[cfg(target_os = "macos")]
/// Persist and resolve the registered global shortcut bindings.
pub mod shortcut_registry;
#[cfg(target_os = "macos")]
/// Thermal-pressure signals used to throttle expensive on-device work.
pub mod thermal;
#[cfg(target_os = "macos")]
/// Menu-bar tray status icons and dictation state presentation.
pub mod tray_status;

/// Objective-C object pointer alias (compatible with the `objc` crate's `msg_send!`).
/// Migrated out of `app/ui/shared/helpers` so OS-level code no longer depends on `ui`.
#[cfg(target_os = "macos")]
pub type Id = *mut objc::runtime::Object;
