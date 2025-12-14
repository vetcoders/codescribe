// hotkeys.rs
//
// Purpose: Captures global hotkeys on macOS using low-level CGEventTap
//
// Detects modifier-only keypresses:
// - Hold Ctrl: Start recording while held, stop when released
// - Double-tap Option: Toggle recording on/off
//
// Design: Uses CGEventTap to monitor modifier flag changes only.
// We specifically avoid calling TSMGetInputSourceProperty which caused
// rdev to crash on macOS 26.2 (Sequoia). We only read CGEventFlags,
// not keyboard layout or key translation.

use crossbeam_channel::Sender;
use std::time::{Duration, Instant};

// --- Constants ---

/// Double-tap interval for toggle detection (milliseconds)
const DOUBLE_TAP_INTERVAL_MS: u64 = 450;

/// Minimum hold duration to distinguish from accidental tap (milliseconds)
const MIN_HOLD_DURATION_MS: u64 = 150;

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
    /// Hold gesture detected (press/release Ctrl key)
    /// The boolean indicates "assistive mode" (Shift was held during the gesture)
    Hold { action: HoldAction, assistive: bool },
    /// Toggle gesture detected (double-tap Option within threshold)
    Toggle,
}

/// Modifier flags for hold gesture detection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModifierFlags {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub cmd: bool,
}

#[allow(dead_code)]
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

    // CGEventType values
    const K_CG_EVENT_FLAGS_CHANGED: CGEventType = 12;

    // CGEventFlags masks
    const K_CG_EVENT_FLAG_MASK_CONTROL: CGEventFlags = 0x00040000;
    const K_CG_EVENT_FLAG_MASK_SHIFT: CGEventFlags = 0x00020000;
    const K_CG_EVENT_FLAG_MASK_ALTERNATE: CGEventFlags = 0x00080000; // Option key
    const K_CG_EVENT_FLAG_MASK_COMMAND: CGEventFlags = 0x00100000;

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
    extern "C" {
        fn CGEventTapCreate(
            tap: u32,
            place: u32,
            options: u32,
            events_of_interest: u64,
            callback: CGEventTapCallBack,
            user_info: *mut c_void,
        ) -> CFMachPortRef;

        fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
        fn CGEventGetFlags(event: CGEventRef) -> CGEventFlags;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
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
        /// Ctrl is currently held
        ctrl_down: bool,
        /// When Ctrl was pressed (for hold duration check)
        ctrl_down_ts: Option<Instant>,
        /// Shift was held when Ctrl was pressed (assistive mode)
        assistive_mode: bool,
        /// Hold event already sent (prevent duplicates)
        hold_event_sent: bool,
        /// Last Option tap timestamp (for double-tap detection)
        last_option_tap_ts: Option<Instant>,
        /// Option is currently held
        option_down: bool,
        /// Event sender
        tx: Sender<HotkeyEvent>,
    }

    impl HotkeyState {
        fn new(tx: Sender<HotkeyEvent>) -> Self {
            Self {
                ctrl_down: false,
                ctrl_down_ts: None,
                assistive_mode: false,
                hold_event_sent: false,
                last_option_tap_ts: None,
                option_down: false,
                tx,
            }
        }
    }

    // Global state pointer for callback (must be static for C callback)
    static mut GLOBAL_STATE: Option<*mut HotkeyState> = None;
    static RUNNING: AtomicBool = AtomicBool::new(false);
    static ENABLED: AtomicBool = AtomicBool::new(true);

    /// CGEventTap callback - processes modifier key events
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

        // Only process flags changed events
        if event_type != K_CG_EVENT_FLAGS_CHANGED {
            return event;
        }

        let flags = unsafe { CGEventGetFlags(event) };

        let state = unsafe {
            match GLOBAL_STATE {
                Some(ptr) => &mut *ptr,
                None => return event,
            }
        };

        // Check current modifier states
        let ctrl_now = (flags & K_CG_EVENT_FLAG_MASK_CONTROL) != 0;
        let shift_now = (flags & K_CG_EVENT_FLAG_MASK_SHIFT) != 0;
        let option_now = (flags & K_CG_EVENT_FLAG_MASK_ALTERNATE) != 0;
        let cmd_now = (flags & K_CG_EVENT_FLAG_MASK_COMMAND) != 0;

        // Detect Ctrl press/release for hold gesture
        if ctrl_now && !state.ctrl_down {
            // Ctrl just pressed
            state.ctrl_down = true;
            state.ctrl_down_ts = Some(Instant::now());
            state.assistive_mode = shift_now;
            state.hold_event_sent = false;

            tracing::debug!("Ctrl pressed - sending Hold Down event");
            // Send Hold Down immediately for responsiveness
            let _ = state.tx.send(HotkeyEvent::Hold {
                action: HoldAction::Down,
                assistive: state.assistive_mode,
            });
            state.hold_event_sent = true;
        } else if !ctrl_now && state.ctrl_down {
            // Ctrl just released
            state.ctrl_down = false;
            tracing::debug!("Ctrl released");

            // Only send Up if we sent Down and held long enough
            if state.hold_event_sent {
                if let Some(ts) = state.ctrl_down_ts {
                    let elapsed = ts.elapsed();
                    if elapsed >= Duration::from_millis(MIN_HOLD_DURATION_MS) {
                        tracing::debug!("Ctrl held for {:?} - sending Hold Up event", elapsed);
                        let _ = state.tx.send(HotkeyEvent::Hold {
                            action: HoldAction::Up,
                            assistive: state.assistive_mode,
                        });
                    } else {
                        tracing::debug!("Ctrl held for {:?} - too short, ignoring", elapsed);
                    }
                }
            }
            state.ctrl_down_ts = None;
        }

        // Detect Option double-tap for toggle gesture
        if option_now && !state.option_down {
            // Option just pressed
            state.option_down = true;
            tracing::debug!("Option pressed");
        } else if !option_now && state.option_down {
            // Option just released
            state.option_down = false;
            tracing::debug!("Option released");

            // Don't trigger toggle if other modifiers were held
            if !ctrl_now && !cmd_now {
                let now = Instant::now();

                if let Some(last_tap) = state.last_option_tap_ts {
                    let interval = now.duration_since(last_tap);
                    if interval <= Duration::from_millis(DOUBLE_TAP_INTERVAL_MS) {
                        // Double-tap detected!
                        tracing::debug!(
                            "Option double-tap detected ({:?}) - sending Toggle",
                            interval
                        );
                        let _ = state.tx.send(HotkeyEvent::Toggle);
                        state.last_option_tap_ts = None;
                    } else {
                        // Too slow, start new sequence
                        tracing::debug!("Option tap too slow ({:?}), resetting", interval);
                        state.last_option_tap_ts = Some(now);
                    }
                } else {
                    // First tap
                    tracing::debug!("Option first tap");
                    state.last_option_tap_ts = Some(now);
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

        // Event mask for flags changed events only
        let event_mask: u64 = 1 << K_CG_EVENT_FLAGS_CHANGED;

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

        tracing::info!("CGEventTap started, monitoring Ctrl hold and Option double-tap");

        // Run the loop (blocking)
        unsafe {
            CFRunLoopRun();
        }

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
    #[allow(dead_code)]
    tx: Sender<HotkeyEvent>,
}

impl HotkeyManager {
    /// Create a new HotkeyManager
    ///
    /// IMPORTANT: On macOS, starts a background thread for CGEventTap.
    /// Requires Accessibility permission.
    pub fn new(tx: Sender<HotkeyEvent>) -> Result<Self, String> {
        // Start the listener
        macos::start_listener(tx.clone())?;

        Ok(Self { tx })
    }

    /// Process pending hotkey events
    ///
    /// Note: With CGEventTap implementation, events are sent directly to the channel.
    /// This method is kept for API compatibility but does nothing.
    pub fn process_events(&self) {
        // Events are processed in the background thread
        // This is a no-op for API compatibility
    }

    /// Enable hotkey processing (thread-safe)
    ///
    /// When enabled, modifier key events will be captured and sent to the event channel.
    pub fn enable(&self) {
        macos::enable();
    }

    /// Disable hotkey processing (thread-safe)
    ///
    /// When disabled, modifier key events will be ignored and no events will be sent.
    /// The CGEventTap remains running but skips processing.
    pub fn disable(&self) {
        macos::disable();
    }

    /// Check if hotkeys are currently enabled (thread-safe)
    pub fn is_enabled(&self) -> bool {
        macos::is_enabled()
    }
}

// --- Legacy API (for compatibility) ---

/// Start the global hotkey listener (legacy API - now just returns success)
///
/// The actual hotkey handling is now done through HotkeyManager integrated
/// with CGEventTap.
#[allow(dead_code)]
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
}
