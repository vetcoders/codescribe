//! Process-global ownership of the hotkey runtime.
//!
//! One [`HotkeyManager`] lives behind a global mutex and owns the platform event
//! tap. The sender handed to [`install_global_hotkey_manager`] is retained even
//! when startup fails, so [`refresh_global_hotkey_manager`] can re-arm the tap
//! once a TCC grant lands without an app restart.

use super::detector::HotkeyEvent;
use super::platform;
use crossbeam_channel::Sender;
use std::sync::{Mutex, OnceLock};

// --- Public API ---

/// Enable hotkey processing (thread-safe, global)
///
/// When enabled, modifier key events will be captured and sent to the event channel.
pub fn enable_hotkeys() {
    platform::enable();
}

/// Disable hotkey processing (thread-safe, global)
///
/// When disabled, modifier key events will be ignored and no events will be sent.
/// The CGEventTap remains running but skips processing.
pub fn disable_hotkeys() {
    platform::disable();
}

/// Check if hotkeys are currently enabled (thread-safe, global)
pub fn are_hotkeys_enabled() -> bool {
    platform::is_enabled()
}

/// Process-global hotkey state: the retained sender plus the live manager.
#[derive(Default)]
struct GlobalHotkeyService {
    /// Retained across failed starts so a later re-arm can reuse it.
    tx: Option<Sender<HotkeyEvent>>,
    /// `None` until a tap is successfully created (or after shutdown).
    manager: Option<HotkeyManager>,
}

/// Lazily initialize and borrow the process-global hotkey service.
fn global_hotkey_service() -> &'static Mutex<GlobalHotkeyService> {
    static GLOBAL_HOTKEY_SERVICE: OnceLock<Mutex<GlobalHotkeyService>> = OnceLock::new();
    GLOBAL_HOTKEY_SERVICE.get_or_init(|| Mutex::new(GlobalHotkeyService::default()))
}

/// Tear down any live manager and build a fresh one from the retained sender.
///
/// Errors when no sender has been installed yet, or when the platform tap still
/// cannot be created (permission absent).
fn replace_global_hotkey_manager(guard: &mut GlobalHotkeyService) -> Result<(), String> {
    let Some(tx) = guard.tx.clone() else {
        return Err("Hotkey runtime not initialized".to_string());
    };

    if let Some(manager) = guard.manager.as_mut() {
        manager.shutdown();
    }
    guard.manager = None;
    guard.manager = Some(HotkeyManager::new(tx)?);
    Ok(())
}

/// Install the process-global hotkey runtime owner.
///
/// The sender is retained even when startup fails so a later live reinit can retry
/// once permissions become available.
pub fn install_global_hotkey_manager(tx: Sender<HotkeyEvent>) -> Result<(), String> {
    let mut guard = global_hotkey_service()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    guard.tx = Some(tx);
    replace_global_hotkey_manager(&mut guard)
}

/// Recreate the process-global hotkey runtime after a permission or settings
/// change, reusing the sender retained by `install_global_hotkey_manager`.
///
/// This is the runtime re-arm path for the "TCC fresh-grant" case: the
/// CGEventTap reads Accessibility / Input Monitoring only at creation, so a
/// first-run grant leaves hotkeys dead until the tap is rebuilt (or the app
/// restarts). Returns `Err` when no sender is installed yet (i.e. `start()` was
/// never called) or when the tap still cannot be created (permission absent).
pub fn refresh_global_hotkey_manager() -> Result<(), String> {
    let mut guard = global_hotkey_service()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    replace_global_hotkey_manager(&mut guard)
}

/// Stop the global hotkey runtime and drop the manager, keeping the sender.
pub fn shutdown_global_hotkey_manager() {
    let mut guard = global_hotkey_service()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(manager) = guard.manager.as_mut() {
        manager.shutdown();
    }
    guard.manager = None;
}

/// Is a hotkey manager currently installed?
///
/// Used by the bridge to dedup re-arm requests after a permission grant.
pub fn is_global_hotkey_manager_active() -> bool {
    global_hotkey_service()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .manager
        .is_some()
}

/// Manages global hotkey runtime ownership.
///
/// Owns the macOS event tap worker thread and tears it down on `shutdown()`/`Drop`.
/// Runtime starts in `new`; there is no separate `start`/`process` lifecycle.
pub struct HotkeyManager {
    /// Kept for future use (e.g., manual event injection)
    _tx: Sender<HotkeyEvent>,
    /// Platform event-tap worker; `None` once shut down.
    runtime: Option<platform::HotkeyRuntime>,
}

impl HotkeyManager {
    /// Create a new HotkeyManager
    ///
    /// IMPORTANT: On macOS, starts a background thread for CGEventTap.
    /// Requires Accessibility permission.
    pub fn new(tx: Sender<HotkeyEvent>) -> Result<Self, String> {
        let runtime = platform::start_listener(tx.clone())?;

        Ok(Self {
            _tx: tx,
            runtime: Some(runtime),
        })
    }

    /// Stop global hotkeys and wait for runtime teardown.
    ///
    /// Safe to call multiple times.
    pub fn shutdown(&mut self) {
        if let Some(runtime) = self.runtime.as_mut() {
            runtime.shutdown();
        }
        self.runtime = None;
    }
}

impl Drop for HotkeyManager {
    /// Guarantees the event tap is torn down even on an early return or unwind.
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Smoke coverage for the process-global getters.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_global_hotkey_manager_active_returns_bool_safely() {
        // Smoke: getter must not panic on a fresh test runtime. The actual
        // value depends on whether prior tests have spun up the global hotkey
        // service (process-global Mutex), so we just assert the call returns
        // a bool without crashing. This guards the dedup path in
        // `bridge/src/hotkeys.rs::CodescribeHotkeys::rearm_after_permission_grant`
        // which calls this helper before deciding to refresh the manager.
        let active: bool = is_global_hotkey_manager_active();
        let _ = active;
    }
}
