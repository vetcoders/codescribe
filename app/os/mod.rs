#[cfg(target_os = "macos")]
pub mod clipboard;
#[cfg(target_os = "macos")]
pub mod hotkeys;
#[cfg(target_os = "macos")]
pub mod notifications;
#[cfg(target_os = "macos")]
pub mod onboarding;
#[cfg(target_os = "macos")]
pub mod permissions;
#[cfg(target_os = "macos")]
pub mod selection;
#[cfg(target_os = "macos")]
pub mod shortcut_registry;
#[cfg(target_os = "macos")]
pub mod thermal;
#[cfg(target_os = "macos")]
pub mod tray_status;

/// Objective-C object pointer alias (compatible with the `objc` crate's `msg_send!`).
/// Migrated out of `app/ui/shared/helpers` so OS-level code no longer depends on `ui`.
#[cfg(target_os = "macos")]
pub type Id = *mut objc::runtime::Object;
