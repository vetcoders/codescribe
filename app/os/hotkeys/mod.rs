// hotkeys.rs
//
// Purpose: Captures global hotkeys on macOS using low-level CGEventTap.
//
// The module keeps the historical `crate::os::hotkeys::*` public surface while
// splitting the implementation by responsibility: runtime config, pure gesture
// detection, platform event taps, and process-global runtime ownership.

//! Global hotkey capture: the stable `crate::os::hotkeys::*` facade.
//!
//! This module holds no logic of its own — it is a re-export surface over four
//! submodules that split what used to live in one file. Consumers import from
//! here, not from the submodules, so the internal split can move without
//! breaking call sites.
//!
//! - `config` — runtime-tunable bindings and thresholds (hold delay, double-tap
//!   window, exclusive mode), readable and writable while the app runs.
//! - `detector` — the pure gesture state machine over key/modifier snapshots.
//! - `platform` — the macOS `CGEventTap` feeding that state machine.
//! - `manager` — process-global ownership of the tap, plus the enable/disable gate.
//!
//! The seam worth knowing: gesture recognition carries no platform types, so
//! hold and double-tap behaviour is testable without an event tap or an
//! accessibility grant. Each submodule documents itself in depth.

mod config;
mod detector;
mod manager;
mod platform;

pub use config::{
    HotkeyRuntimeConfig, ModeHotkeyBindings, apply_hotkey_config, apply_hotkey_runtime_config,
    get_deferred_insert_shortcut, get_double_tap_interval_ms, get_exclusive_mode,
    get_hold_arm_modifier, get_hold_start_delay_ms, get_hotkey_runtime_config,
    get_mode_hotkey_bindings, set_deferred_insert_shortcut, set_double_tap_interval_ms,
    set_exclusive_mode, set_hold_arm_modifier, set_hold_start_delay_ms, set_mode_hotkey_bindings,
};
pub use detector::{
    DoubleTapBlockReason, DoubleTapGesture, HoldAction, HoldMode, HotkeyDetector,
    HotkeyDetectorInput, HotkeyEvent, HotkeyModifierSnapshot, HotkeyPhysicalKey, ModifierFlags,
    arm_ignored_diagnostic_line, blocked_double_tap_diagnostic_line,
};
pub use manager::{
    HotkeyManager, are_hotkeys_enabled, disable_hotkeys, enable_hotkeys,
    install_global_hotkey_manager, is_global_hotkey_manager_active, refresh_global_hotkey_manager,
    shutdown_global_hotkey_manager,
};
