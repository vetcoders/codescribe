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

#[derive(Default)]
struct GlobalHotkeyService {
    tx: Option<Sender<HotkeyEvent>>,
    manager: Option<HotkeyManager>,
}

fn global_hotkey_service() -> &'static Mutex<GlobalHotkeyService> {
    static GLOBAL_HOTKEY_SERVICE: OnceLock<Mutex<GlobalHotkeyService>> = OnceLock::new();
    GLOBAL_HOTKEY_SERVICE.get_or_init(|| Mutex::new(GlobalHotkeyService::default()))
}

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

/// Recreate the process-global hotkey runtime after permissions or settings change.
pub fn refresh_global_hotkey_manager() -> Result<(), String> {
    let mut guard = global_hotkey_service()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    replace_global_hotkey_manager(&mut guard)
}

pub fn shutdown_global_hotkey_manager() {
    let mut guard = global_hotkey_service()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(manager) = guard.manager.as_mut() {
        manager.shutdown();
    }
    guard.manager = None;
}

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
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_global_hotkey_manager_active_returns_bool_safely() {
        // Smoke: getter must not panic on a fresh test runtime. The actual
        // value depends on whether prior tests have spun up the global hotkey
        // service (process-global Mutex), so we just assert the call returns
        // a bool without crashing. This guards the dedup path in
        // `app/ui/onboarding/permission_flow.rs::reconcile_permission_runtime_after_grant`
        // which calls this helper before deciding to refresh the manager.
        let active: bool = is_global_hotkey_manager_active();
        let _ = active;
    }
}
