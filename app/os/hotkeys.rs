// hotkeys.rs
//
// Purpose: Captures global hotkeys on macOS using low-level CGEventTap
//
// Detects modifier-only keypresses:
// - Hold Ctrl (or configured combo): Start recording while held, stop when released
// - Double-tap Left Option: Toggle recording on/off (normal, AI formatting)
// - Double-tap Right Option: Toggle assistive hands-off (AI augmentation)
//
// Design: Uses CGEventTap to monitor modifier flag changes only.
// We specifically avoid calling TSMGetInputSourceProperty which caused
// rdev to crash on macOS 26.2 (Sequoia). We only read CGEventFlags,
// not keyboard layout or key translation.
//
// HoldMods options:
// - Ctrl: Ctrl key only (default)
// - CtrlAlt: Ctrl+Option together
// - CtrlShift: Ctrl+Shift together
// - CtrlCmd: Ctrl+Command together
//
// ToggleTrigger options:
// - DoubleOption: Left Option (normal) + Right Option (assistive)
// - DoubleRightOption: Right Option only (assistive only)
// - None: Toggle mode completely disabled

use crate::config::{HoldMods, ToggleTrigger};
use crossbeam_channel::Sender;
use std::sync::atomic::{AtomicU8, Ordering as AtomicOrdering};
use std::time::{Duration, Instant};

// --- Global HoldMods Configuration ---

/// Atomic storage for current HoldMods setting
/// Values: 0=Ctrl, 1=CtrlAlt, 2=CtrlShift, 3=CtrlCmd
static HOLD_MODS: AtomicU8 = AtomicU8::new(0);

/// Set the hold modifier combination for hold-to-talk
pub fn set_hold_mods(mods: HoldMods) {
    let value = match mods {
        HoldMods::Ctrl => 0,
        HoldMods::CtrlAlt => 1,
        HoldMods::CtrlShift => 2,
        HoldMods::CtrlCmd => 3,
    };
    HOLD_MODS.store(value, AtomicOrdering::SeqCst);
    tracing::info!("HoldMods set to {:?}", mods);
}

/// Get the current hold modifier combination
pub fn get_hold_mods() -> HoldMods {
    match HOLD_MODS.load(AtomicOrdering::SeqCst) {
        0 => HoldMods::Ctrl,
        1 => HoldMods::CtrlAlt,
        2 => HoldMods::CtrlShift,
        3 => HoldMods::CtrlCmd,
        _ => HoldMods::Ctrl, // fallback
    }
}

// --- Global Toggle Trigger Setting ---

/// Atomic storage for ToggleTrigger (0=DoubleOption, 1=DoubleRightOption, 2=None)
static TOGGLE_TRIGGER: AtomicU8 = AtomicU8::new(0);

/// Set the toggle trigger mode (thread-safe)
pub fn set_toggle_trigger(trigger: ToggleTrigger) {
    let value = match trigger {
        ToggleTrigger::DoubleOption => 0,
        ToggleTrigger::DoubleRightOption => 1,
        ToggleTrigger::None => 2,
    };
    TOGGLE_TRIGGER.store(value, AtomicOrdering::SeqCst);
    tracing::info!("Toggle trigger set to: {:?}", trigger);
}

/// Get the current toggle trigger mode (thread-safe)
pub fn get_toggle_trigger() -> ToggleTrigger {
    match TOGGLE_TRIGGER.load(AtomicOrdering::SeqCst) {
        0 => ToggleTrigger::DoubleOption,
        1 => ToggleTrigger::DoubleRightOption,
        _ => ToggleTrigger::None,
    }
}

// --- Global Exclusive Mode Setting ---
// Note: Exclusive mode is now implicitly handled by HoldMods configuration.
// When HoldMods::Ctrl is set, Option key is excluded from hold combo.
// This function is kept for API compatibility with existing code.

use std::sync::atomic::AtomicBool;

/// Atomic storage for exclusive mode (Ctrl and Option mutually exclusive)
static EXCLUSIVE_MODE: AtomicBool = AtomicBool::new(true);

/// Set exclusive mode (thread-safe)
/// When true, Ctrl and Option are mutually exclusive (default behavior)
/// When false, they can be pressed together (legacy behavior)
pub fn set_exclusive_mode(enabled: bool) {
    EXCLUSIVE_MODE.store(enabled, AtomicOrdering::SeqCst);
    tracing::info!("Hotkey exclusive mode set to: {}", enabled);
}

// --- Constants ---

/// Double-tap interval for toggle detection (milliseconds)
const DOUBLE_TAP_INTERVAL_MS: u64 = 450;

// --- Types ---

/// Represents the action of a hold gesture
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldAction {
    Down,
    Up,
}

/// Hotkey event emitted by the listener
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyEvent {
    /// Hold gesture detected (press/release configured modifier combo)
    /// The boolean indicates "assistive mode" (Shift was held during the gesture)
    Hold { action: HoldAction, assistive: bool },
    /// Normal toggle gesture (double-tap left Option)
    ToggleNormal,
    /// Assistive toggle gesture (double-tap right Option)
    ToggleAssistive,
    /// Conversation mode gesture (Ctrl+Option hold) - full-duplex Moshi
    Conversation { action: HoldAction },
}

/// Modifier flags for hold gesture detection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModifierFlags {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub cmd: bool,
}

impl ModifierFlags {
    pub fn new() -> Self {
        Self {
            ctrl: false,
            alt: false,
            shift: false,
            cmd: false,
        }
    }

    pub fn ctrl_only() -> Self {
        Self {
            ctrl: true,
            alt: false,
            shift: false,
            cmd: false,
        }
    }

    /// Check if the current flags match the required flags
    pub fn matches(&self, required: &ModifierFlags, exclusive: bool) -> bool {
        if exclusive {
            self.ctrl == required.ctrl && self.alt == required.alt && self.cmd == required.cmd
        } else {
            (!required.ctrl || self.ctrl)
                && (!required.alt || self.alt)
                && (!required.shift || self.shift)
                && (!required.cmd || self.cmd)
        }
    }

    pub fn is_assistive(&self) -> bool {
        self.shift
    }
}

impl Default for ModifierFlags {
    fn default() -> Self {
        Self::new()
    }
}

// --- macOS CGEventTap Implementation using raw bindings ---

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;

    // CGEvent types and flags
    type CGEventRef = *mut c_void;
    type CGEventTapProxy = *mut c_void;
    type CFMachPortRef = *mut c_void;
    type CFRunLoopSourceRef = *mut c_void;
    type CFRunLoopRef = *mut c_void;

    type CGEventType = u32;
    type CGEventFlags = u64;
    type CGEventField = u32;

    // CGEventType values
    const K_CG_EVENT_KEY_DOWN: CGEventType = 10;
    const K_CG_EVENT_FLAGS_CHANGED: CGEventType = 12;

    // CGEventFlags masks
    const K_CG_EVENT_FLAG_MASK_CONTROL: CGEventFlags = 0x00040000;
    const K_CG_EVENT_FLAG_MASK_SHIFT: CGEventFlags = 0x00020000;
    const K_CG_EVENT_FLAG_MASK_ALTERNATE: CGEventFlags = 0x00080000; // Option key
    const K_CG_EVENT_FLAG_MASK_COMMAND: CGEventFlags = 0x00100000;

    // CGEventField for keycode
    const K_CG_KEYBOARD_EVENT_KEYCODE: CGEventField = 9;

    // macOS virtual keycodes for Option keys
    const K_VK_OPTION: i64 = 58; // Left Option
    const K_VK_RIGHT_OPTION: i64 = 61; // Right Option

    // CGEventTap constants
    const K_CG_SESSION_EVENT_TAP: u32 = 1;
    const K_CG_HEAD_INSERT_EVENT_TAP: u32 = 0;
    const K_CG_EVENT_TAP_OPTION_LISTEN_ONLY: u32 = 1;

    // Callback type
    type CGEventTapCallBack = extern "C" fn(
        proxy: CGEventTapProxy,
        event_type: CGEventType,
        event: CGEventRef,
        user_info: *mut c_void,
    ) -> CGEventRef;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventTapCreate(
            tap: u32,
            place: u32,
            options: u32,
            events_of_interest: u64,
            callback: CGEventTapCallBack,
            user_info: *mut c_void,
        ) -> CFMachPortRef;

        fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
        fn CGEventTapIsEnabled(tap: CFMachPortRef) -> bool;
        fn CGEventGetFlags(event: CGEventRef) -> CGEventFlags;
        fn CGEventGetIntegerValueField(event: CGEventRef, field: CGEventField) -> i64;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFMachPortCreateRunLoopSource(
            allocator: *const c_void,
            port: CFMachPortRef,
            order: i64,
        ) -> CFRunLoopSourceRef;

        fn CFRunLoopGetCurrent() -> CFRunLoopRef;
        fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: *const c_void);
        fn CFRunLoopRun();

        static kCFRunLoopCommonModes: *const c_void;
    }

    /// State for tracking modifier keypresses
    struct HotkeyState {
        /// Hold combo is currently active (all required modifiers held)
        hold_active: bool,
        /// When hold combo was activated (for hold duration check)
        hold_active_ts: Option<Instant>,
        /// Assistive mode detected during this hold
        assistive_mode: bool,
        /// Hold event already sent (prevent duplicates)
        hold_event_sent: bool,
        /// Last left Option tap timestamp
        last_left_tap_ts: Option<Instant>,
        /// Last right Option tap timestamp
        last_right_tap_ts: Option<Instant>,
        /// Option is currently held
        option_down: bool,
        /// Whether the currently held Option is the RIGHT Option key
        right_option_held: bool,
        /// A non-modifier key was pressed while modifier(s) held - invalidates gesture
        key_pressed_during_modifier: bool,
        /// Event sender
        tx: Sender<HotkeyEvent>,
    }

    impl HotkeyState {
        fn new(tx: Sender<HotkeyEvent>) -> Self {
            Self {
                hold_active: false,
                hold_active_ts: None,
                assistive_mode: false,
                hold_event_sent: false,
                last_left_tap_ts: None,
                last_right_tap_ts: None,
                option_down: false,
                right_option_held: false,
                key_pressed_during_modifier: false,
                tx,
            }
        }
    }

    fn register_option_tap(
        last_tap: &mut Option<Instant>,
        event: HotkeyEvent,
        tx: &Sender<HotkeyEvent>,
    ) {
        let now = Instant::now();
        if let Some(previous) = *last_tap
            && now.duration_since(previous) <= Duration::from_millis(DOUBLE_TAP_INTERVAL_MS)
        {
            let _ = tx.send(event);
            *last_tap = None;
            return;
        }

        *last_tap = Some(now);
    }

    /// Check if the configured hold combo is currently pressed
    /// Returns (combo_active, is_assistive)
    fn check_hold_combo(
        ctrl: bool,
        shift: bool,
        option: bool,
        cmd: bool,
        hold_mods: HoldMods,
    ) -> (bool, bool) {
        let combo_active = match hold_mods {
            // Ctrl alone triggers (Option must NOT be pressed for exclusivity with toggle)
            HoldMods::Ctrl => ctrl && !option && !cmd,
            // Ctrl+Alt (Option) together trigger
            HoldMods::CtrlAlt => ctrl && option && !cmd,
            // Ctrl+Shift together trigger (shift is part of combo, not assistive)
            HoldMods::CtrlShift => ctrl && shift && !option && !cmd,
            // Ctrl+Command together trigger
            HoldMods::CtrlCmd => ctrl && cmd && !option,
        };

        // Assistive mode: Shift held DURING the gesture (except for CtrlShift mode where shift is required)
        let is_assistive = match hold_mods {
            HoldMods::CtrlShift => false, // Shift is part of the combo, not assistive
            _ => shift,
        };

        (combo_active, is_assistive)
    }

    // Global state pointer for callback (must be static for C callback)
    static mut GLOBAL_STATE: Option<*mut HotkeyState> = None;
    static RUNNING: AtomicBool = AtomicBool::new(false);
    static ENABLED: AtomicBool = AtomicBool::new(true);

    /// CGEventTap callback - processes modifier key events and key presses
    extern "C" fn event_callback(
        _proxy: CGEventTapProxy,
        event_type: CGEventType,
        event: CGEventRef,
        _user_info: *mut c_void,
    ) -> CGEventRef {
        // Skip processing if hotkeys are disabled
        if !ENABLED.load(Ordering::Relaxed) {
            return event;
        }

        let state = unsafe {
            match GLOBAL_STATE {
                Some(ptr) => &mut *ptr,
                None => return event,
            }
        };

        // Handle KEY_DOWN: cancel pending gestures if non-modifier key pressed
        if event_type == K_CG_EVENT_KEY_DOWN {
            let flags = unsafe { CGEventGetFlags(event) };
            let ctrl_held = (flags & K_CG_EVENT_FLAG_MASK_CONTROL) != 0;
            let option_held = (flags & K_CG_EVENT_FLAG_MASK_ALTERNATE) != 0;

            // If Ctrl is held and hold gesture is in the delay window (~800ms)
            // → cancel the hold gesture by sending Hold Up (Ctrl+K, Ctrl+C, etc.)
            // After delay, recording has started - don't cancel on key presses
            const HOLD_DELAY_MS: u64 = 850; // Slightly longer than controller's 800ms
            if ctrl_held && state.hold_active {
                let in_delay_window = state
                    .hold_active_ts
                    .map(|ts| ts.elapsed() < Duration::from_millis(HOLD_DELAY_MS))
                    .unwrap_or(false);

                if in_delay_window {
                    tracing::info!(
                        "Key pressed during Ctrl hold delay - canceling (Ctrl+key combo detected)"
                    );
                    // Send Hold Up to cancel the pending hold in controller
                    let _ = state.tx.send(HotkeyEvent::Hold {
                        action: HoldAction::Up,
                        assistive: state.assistive_mode,
                    });
                    state.hold_active = false;
                    state.hold_active_ts = None;
                    state.hold_event_sent = false;
                    state.key_pressed_during_modifier = true;
                } else {
                    tracing::debug!("Key pressed during recording - allowed (past delay window)");
                }
            }

            // If Option is held → invalidate tap sequence (Option+Arrow, etc.)
            // This is NOT a tap - it's a modifier combo, so discard the sequence
            if option_held && state.option_down {
                tracing::debug!("Key pressed while Option held - this is a combo, not a tap");
                state.key_pressed_during_modifier = true;
                // Discard any pending tap sequence - do NOT send Toggle
                state.last_left_tap_ts = None;
                state.last_right_tap_ts = None;
            }

            return event;
        }

        // Only process flags changed events from here
        if event_type != K_CG_EVENT_FLAGS_CHANGED {
            return event;
        }

        let flags = unsafe { CGEventGetFlags(event) };
        let keycode = unsafe { CGEventGetIntegerValueField(event, K_CG_KEYBOARD_EVENT_KEYCODE) };

        // Debug: log every modifier event
        tracing::debug!(
            "CGEventTap: flags=0x{:X} keycode={} (ctrl={}, shift={}, opt={}, cmd={})",
            flags,
            keycode,
            (flags & K_CG_EVENT_FLAG_MASK_CONTROL) != 0,
            (flags & K_CG_EVENT_FLAG_MASK_SHIFT) != 0,
            (flags & K_CG_EVENT_FLAG_MASK_ALTERNATE) != 0,
            (flags & K_CG_EVENT_FLAG_MASK_COMMAND) != 0
        );

        // Check current modifier states
        let ctrl_now = (flags & K_CG_EVENT_FLAG_MASK_CONTROL) != 0;
        let shift_now = (flags & K_CG_EVENT_FLAG_MASK_SHIFT) != 0;
        let option_now = (flags & K_CG_EVENT_FLAG_MASK_ALTERNATE) != 0;
        let cmd_now = (flags & K_CG_EVENT_FLAG_MASK_COMMAND) != 0;

        // Reset key_pressed flag when all modifiers released
        if !ctrl_now && !option_now && !cmd_now {
            state.key_pressed_during_modifier = false;
        }

        // Determine if this is specifically the right Option key
        let is_right_option = keycode == K_VK_RIGHT_OPTION;
        let is_option_key = keycode == K_VK_OPTION || keycode == K_VK_RIGHT_OPTION;

        // Get current settings
        let hold_mods = get_hold_mods();
        let toggle_trigger = get_toggle_trigger();

        // Check if hold combo is active
        let (combo_active, is_assistive) =
            check_hold_combo(ctrl_now, shift_now, option_now, cmd_now, hold_mods);

        // Detect hold combo activation/deactivation
        if combo_active && !state.hold_active {
            // Hold combo just activated
            state.hold_active = true;
            state.hold_active_ts = Some(Instant::now());
            state.assistive_mode = is_assistive;
            state.hold_event_sent = false;

            tracing::debug!(
                "Hold combo activated ({:?}, assistive={}) - sending event",
                hold_mods,
                is_assistive
            );
            // Send appropriate event based on hold mode
            // CtrlAlt (Ctrl+Option) = Conversation mode (Moshi full-duplex)
            if hold_mods == HoldMods::CtrlAlt {
                let _ = state.tx.send(HotkeyEvent::Conversation {
                    action: HoldAction::Down,
                });
            } else {
                let _ = state.tx.send(HotkeyEvent::Hold {
                    action: HoldAction::Down,
                    assistive: state.assistive_mode,
                });
            }
            state.hold_event_sent = true;
        } else if combo_active && state.hold_active && is_assistive && !state.assistive_mode {
            // Shift was added while combo active - upgrade to assistive mode
            state.assistive_mode = true;
            tracing::info!("Upgraded to assistive mode (Shift added during hold)");
        } else if !combo_active && state.hold_active {
            // Hold combo just deactivated
            state.hold_active = false;

            // ALWAYS send Up event so controller can cancel pending actions
            // Controller will decide what to do based on state
            if state.hold_event_sent {
                if let Some(ts) = state.hold_active_ts {
                    let elapsed = ts.elapsed();
                    tracing::debug!("Hold combo released after {:?}", elapsed);
                }
                // Send appropriate event based on hold mode
                if hold_mods == HoldMods::CtrlAlt {
                    let _ = state.tx.send(HotkeyEvent::Conversation {
                        action: HoldAction::Up,
                    });
                } else {
                    let _ = state.tx.send(HotkeyEvent::Hold {
                        action: HoldAction::Up,
                        assistive: state.assistive_mode,
                    });
                }
            }
            state.hold_active_ts = None;
        }

        let normal_toggle_enabled = matches!(toggle_trigger, ToggleTrigger::DoubleOption);
        let assistive_toggle_enabled = matches!(
            toggle_trigger,
            ToggleTrigger::DoubleOption | ToggleTrigger::DoubleRightOption
        );

        // Skip Option processing if toggle is disabled
        if matches!(toggle_trigger, ToggleTrigger::None) {
            // Still track option_down state but don't process double-tap
            if option_now && !state.option_down {
                state.option_down = true;
            } else if !option_now && state.option_down {
                state.option_down = false;
            }
            return event;
        }

        // Detect Option double-tap for toggle gesture (left/right)
        if option_now && !state.option_down {
            // Option just pressed
            state.option_down = true;
            state.right_option_held = is_right_option;
            tracing::debug!(
                "Option pressed (right={}, keycode={})",
                is_right_option,
                keycode
            );
        } else if !option_now && state.option_down {
            // Option just released
            let was_right_option = state.right_option_held;
            state.option_down = false;
            state.right_option_held = false;

            tracing::debug!(
                "Option released (right={}, was_right={}, keycode={})",
                is_right_option,
                was_right_option,
                keycode
            );

            // Don't trigger toggle if:
            // - hold combo was/is active or other modifiers held
            // - a key was pressed while Option was held (Option+Arrow is a combo, not a tap)
            let hold_mods_block_toggle = match hold_mods {
                HoldMods::CtrlAlt => false, // Option is part of hold combo, don't block
                _ => ctrl_now || cmd_now || state.hold_active,
            };

            // Skip if this was a combo (Option+Arrow, etc.) not a pure tap
            if state.key_pressed_during_modifier {
                tracing::debug!("Option released after combo - not a tap, skipping");
                state.key_pressed_during_modifier = false;
                return event;
            }

            if !hold_mods_block_toggle {
                let current_tap_is_right = was_right_option || (is_option_key && is_right_option);

                if current_tap_is_right {
                    state.last_left_tap_ts = None;
                    if assistive_toggle_enabled {
                        register_option_tap(
                            &mut state.last_right_tap_ts,
                            HotkeyEvent::ToggleAssistive,
                            &state.tx,
                        );
                    }
                } else if normal_toggle_enabled {
                    state.last_right_tap_ts = None;
                    register_option_tap(
                        &mut state.last_left_tap_ts,
                        HotkeyEvent::ToggleNormal,
                        &state.tx,
                    );
                }
            }
        }

        event
    }

    /// Start the hotkey listener on a background thread
    pub fn start_listener(tx: Sender<HotkeyEvent>) -> Result<(), String> {
        if RUNNING.swap(true, Ordering::SeqCst) {
            return Err("Hotkey listener already running".to_string());
        }

        thread::spawn(move || {
            if let Err(e) = run_event_tap(tx) {
                tracing::error!("CGEventTap error: {}", e);
            }
            RUNNING.store(false, Ordering::SeqCst);
        });

        // Give the thread a moment to start
        thread::sleep(Duration::from_millis(100));

        Ok(())
    }

    /// Enable hotkey processing (thread-safe)
    pub fn enable() {
        ENABLED.store(true, Ordering::SeqCst);
        tracing::info!("Hotkeys enabled");
    }

    /// Disable hotkey processing (thread-safe)
    pub fn disable() {
        ENABLED.store(false, Ordering::SeqCst);
        tracing::info!("Hotkeys disabled");
    }

    /// Check if hotkeys are currently enabled (thread-safe)
    pub fn is_enabled() -> bool {
        ENABLED.load(Ordering::SeqCst)
    }

    /// Run the CGEventTap on the current thread (blocking)
    fn run_event_tap(tx: Sender<HotkeyEvent>) -> Result<(), String> {
        // Create state on heap and store global pointer
        let state = Box::new(HotkeyState::new(tx));
        let state_ptr = Box::into_raw(state);

        unsafe {
            GLOBAL_STATE = Some(state_ptr);
        }

        // Event mask: flags changed + key down (to detect Ctrl+K style combos)
        let event_mask: u64 = (1 << K_CG_EVENT_FLAGS_CHANGED) | (1 << K_CG_EVENT_KEY_DOWN);

        // Create the event tap
        let tap = unsafe {
            CGEventTapCreate(
                K_CG_SESSION_EVENT_TAP,
                K_CG_HEAD_INSERT_EVENT_TAP,
                K_CG_EVENT_TAP_OPTION_LISTEN_ONLY,
                event_mask,
                event_callback,
                std::ptr::null_mut(),
            )
        };

        if tap.is_null() {
            // Clean up state
            unsafe {
                let _ = Box::from_raw(state_ptr);
                GLOBAL_STATE = None;
            }
            return Err("Failed to create CGEventTap - check Accessibility permission".to_string());
        }

        // Enable the tap
        unsafe {
            CGEventTapEnable(tap, true);
        }

        // Verify tap is actually enabled
        let is_enabled = unsafe { CGEventTapIsEnabled(tap) };
        if !is_enabled {
            tracing::error!("CGEventTap failed to enable! macOS may have denied it.");
            unsafe {
                let _ = Box::from_raw(state_ptr);
                GLOBAL_STATE = None;
            }
            return Err("CGEventTap not enabled - macOS denied access".to_string());
        }
        tracing::debug!("CGEventTap verified as enabled");

        // Create run loop source
        let source = unsafe { CFMachPortCreateRunLoopSource(std::ptr::null(), tap, 0) };

        if source.is_null() {
            unsafe {
                let _ = Box::from_raw(state_ptr);
                GLOBAL_STATE = None;
            }
            return Err("Failed to create run loop source".to_string());
        }

        // Add to run loop
        unsafe {
            let run_loop = CFRunLoopGetCurrent();
            CFRunLoopAddSource(run_loop, source, kCFRunLoopCommonModes);
        }

        let hold_mods = get_hold_mods();
        let toggle_trigger = get_toggle_trigger();
        tracing::info!(
            "CGEventTap started, monitoring {:?} hold and Option double-tap (left=normal, right=assistive, trigger={:?})",
            hold_mods,
            toggle_trigger
        );

        // Run the loop (blocking - should never return)
        tracing::debug!("Entering CFRunLoopRun (should block forever)...");
        unsafe {
            CFRunLoopRun();
        }

        // If we get here, something went wrong
        tracing::error!("CFRunLoopRun returned unexpectedly! Event tap may have died.");

        // Clean up (won't reach here normally)
        unsafe {
            let _ = Box::from_raw(state_ptr);
            GLOBAL_STATE = None;
        }

        Ok(())
    }
}

// --- Fallback for non-macOS ---

#[cfg(not(target_os = "macos"))]
mod macos {
    use super::*;

    pub fn start_listener(_tx: Sender<HotkeyEvent>) -> Result<(), String> {
        tracing::warn!("Hotkey listener not supported on this platform");
        Ok(())
    }

    pub fn enable() {
        tracing::warn!("Hotkey enable not supported on this platform");
    }

    pub fn disable() {
        tracing::warn!("Hotkey disable not supported on this platform");
    }

    pub fn is_enabled() -> bool {
        false
    }
}

// --- Public API ---

/// Enable hotkey processing (thread-safe, global)
///
/// When enabled, modifier key events will be captured and sent to the event channel.
pub fn enable_hotkeys() {
    macos::enable();
}

/// Disable hotkey processing (thread-safe, global)
///
/// When disabled, modifier key events will be ignored and no events will be sent.
/// The CGEventTap remains running but skips processing.
pub fn disable_hotkeys() {
    macos::disable();
}

/// Check if hotkeys are currently enabled (thread-safe, global)
pub fn are_hotkeys_enabled() -> bool {
    macos::is_enabled()
}

/// Manages global hotkey registration and event handling
pub struct HotkeyManager {
    /// Kept for future use (e.g., manual event injection)
    _tx: Sender<HotkeyEvent>,
}

impl HotkeyManager {
    /// Create a new HotkeyManager
    ///
    /// IMPORTANT: On macOS, starts a background thread for CGEventTap.
    /// Requires Accessibility permission.
    pub fn new(tx: Sender<HotkeyEvent>) -> Result<Self, String> {
        // Start the listener
        macos::start_listener(tx.clone())?;

        Ok(Self { _tx: tx })
    }

    /// Process pending hotkey events
    ///
    /// Note: With CGEventTap implementation, events are sent directly to the channel.
    /// This method is kept for API compatibility but does nothing.
    pub fn process_events(&self) {
        // Events are processed in the background thread
        // This is a no-op for API compatibility
    }
}

// --- Legacy API (for compatibility) ---

/// Start the global hotkey listener (legacy API - now just returns success)
///
/// The actual hotkey handling is now done through HotkeyManager integrated
/// with CGEventTap.
pub fn start(
    _tx: Sender<HotkeyEvent>,
    _required_modifiers: ModifierFlags,
    _exclusive_mode: bool,
) -> Result<(), String> {
    // This is now a no-op - hotkeys are integrated with HotkeyManager
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_modifier_flags_ctrl_only() {
        let flags = ModifierFlags::ctrl_only();
        assert!(flags.ctrl);
        assert!(!flags.alt);
        assert!(!flags.shift);
        assert!(!flags.cmd);
    }

    #[test]
    fn test_matches_exclusive_mode() {
        let required = ModifierFlags::ctrl_only();
        let current = ModifierFlags {
            ctrl: true,
            alt: false,
            shift: false,
            cmd: false,
        };
        assert!(current.matches(&required, true));

        // With Shift (assistive mode) - should still match in exclusive mode
        let current_with_shift = ModifierFlags {
            ctrl: true,
            alt: false,
            shift: true,
            cmd: false,
        };
        assert!(current_with_shift.matches(&required, true));

        // Extra modifier (Alt) - should NOT match in exclusive mode
        let current_with_extra = ModifierFlags {
            ctrl: true,
            alt: true,
            shift: false,
            cmd: false,
        };
        assert!(!current_with_extra.matches(&required, true));
    }

    #[test]
    fn test_matches_non_exclusive_mode() {
        let required = ModifierFlags::ctrl_only();
        let current = ModifierFlags {
            ctrl: true,
            alt: true, // Extra modifier allowed in non-exclusive mode
            shift: false,
            cmd: false,
        };
        assert!(current.matches(&required, false));
    }

    #[test]
    fn test_is_assistive() {
        let flags = ModifierFlags {
            ctrl: true,
            alt: true,
            shift: true,
            cmd: false,
        };
        assert!(flags.is_assistive());

        let flags_no_shift = ModifierFlags {
            ctrl: true,
            alt: true,
            shift: false,
            cmd: false,
        };
        assert!(!flags_no_shift.is_assistive());
    }

    #[test]
    fn test_toggle_trigger_get_set() {
        // Test default
        set_toggle_trigger(ToggleTrigger::DoubleOption);
        assert_eq!(get_toggle_trigger(), ToggleTrigger::DoubleOption);

        // Test DoubleRightOption
        set_toggle_trigger(ToggleTrigger::DoubleRightOption);
        assert_eq!(get_toggle_trigger(), ToggleTrigger::DoubleRightOption);

        // Test None (disabled)
        set_toggle_trigger(ToggleTrigger::None);
        assert_eq!(get_toggle_trigger(), ToggleTrigger::None);

        // Reset to default
        set_toggle_trigger(ToggleTrigger::DoubleOption);
    }

    #[test]
    fn test_hold_mods_get_set() {
        // Test default
        set_hold_mods(HoldMods::Ctrl);
        assert_eq!(get_hold_mods(), HoldMods::Ctrl);

        // Test CtrlAlt
        set_hold_mods(HoldMods::CtrlAlt);
        assert_eq!(get_hold_mods(), HoldMods::CtrlAlt);

        // Test CtrlShift
        set_hold_mods(HoldMods::CtrlShift);
        assert_eq!(get_hold_mods(), HoldMods::CtrlShift);

        // Test CtrlCmd
        set_hold_mods(HoldMods::CtrlCmd);
        assert_eq!(get_hold_mods(), HoldMods::CtrlCmd);

        // Reset to default
        set_hold_mods(HoldMods::Ctrl);
    }
}
