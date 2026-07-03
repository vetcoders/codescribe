// hotkeys.rs
//
// Purpose: Captures global hotkeys on macOS using low-level CGEventTap.
//
// The module keeps the historical `crate::os::hotkeys::*` public surface while
// splitting the implementation by responsibility: runtime config, pure gesture
// detection, platform event taps, and process-global runtime ownership.

mod config;
mod detector;
mod manager;
mod platform;

pub use config::{
    HotkeyRuntimeConfig, ModeHotkeyBindings, apply_hotkey_config, apply_hotkey_runtime_config,
    get_double_tap_interval_ms, get_exclusive_mode, get_hold_start_delay_ms,
    get_hotkey_runtime_config, get_mode_hotkey_bindings, set_double_tap_interval_ms,
    set_exclusive_mode, set_hold_start_delay_ms, set_mode_hotkey_bindings,
};
pub use detector::{
    DoubleTapBlockReason, DoubleTapGesture, HoldAction, HoldMode, HotkeyDetector,
    HotkeyDetectorInput, HotkeyEvent, HotkeyModifierSnapshot, HotkeyPhysicalKey, ModifierFlags,
};
pub use manager::{
    HotkeyManager, are_hotkeys_enabled, disable_hotkeys, enable_hotkeys,
    install_global_hotkey_manager, is_global_hotkey_manager_active, shutdown_global_hotkey_manager,
};
