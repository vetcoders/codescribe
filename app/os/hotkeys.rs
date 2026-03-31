// hotkeys.rs
//
// Purpose: Captures global hotkeys on macOS using low-level CGEventTap
//
// Detects modifier-only keypresses:
// - Hold Ctrl (or configured combo): Start recording while held, stop when released
// - Double-tap Left Option: Toggle recording on/off (normal, AI formatting)
// - Double-tap Right Option: Toggle assistive hands-off (AI augmentation)
// - Double-tap Ctrl: Toggle recording on/off (raw, auto-paste)
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
// - None: hold-to-talk disabled
//
// ToggleTrigger options:
// - DoubleOption: Left Option (normal) + Right Option (assistive)
// - DoubleLeftOption: Left Option only (normal)
// - DoubleRightOption: Right Option only (assistive only)
// - DoubleCtrl: Ctrl only (raw hands-off)
// - None: Toggle mode completely disabled

use crate::config::{Config, HoldMods, ShortcutBinding, ToggleTrigger, UserSettings, WorkMode};
use crossbeam_channel::Sender;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering as AtomicOrdering};
use std::time::{Duration, Instant};

// --- Global HoldMods Configuration ---

/// Atomic storage for current HoldMods setting
/// Values: 0=Fn, 1=None, 2=Ctrl, 3=CtrlAlt, 4=CtrlShift, 5=CtrlCmd
static HOLD_MODS: AtomicU8 = AtomicU8::new(0);
const HOLD_MODS_FN: u8 = 0;
const HOLD_MODS_NONE: u8 = 1;
const HOLD_MODS_CTRL: u8 = 2;
const HOLD_MODS_CTRL_ALT: u8 = 3;
const HOLD_MODS_CTRL_SHIFT: u8 = 4;
const HOLD_MODS_CTRL_CMD: u8 = 5;

fn encode_hold_mods(mods: HoldMods) -> u8 {
    match mods {
        HoldMods::Fn => HOLD_MODS_FN,
        HoldMods::None => HOLD_MODS_NONE,
        HoldMods::Ctrl => HOLD_MODS_CTRL,
        HoldMods::CtrlAlt => HOLD_MODS_CTRL_ALT,
        HoldMods::CtrlShift => HOLD_MODS_CTRL_SHIFT,
        HoldMods::CtrlCmd => HOLD_MODS_CTRL_CMD,
    }
}

fn decode_hold_mods(value: u8) -> HoldMods {
    match value {
        HOLD_MODS_FN => HoldMods::Fn,
        HOLD_MODS_NONE => HoldMods::None,
        HOLD_MODS_CTRL => HoldMods::Ctrl,
        HOLD_MODS_CTRL_ALT => HoldMods::CtrlAlt,
        HOLD_MODS_CTRL_SHIFT => HoldMods::CtrlShift,
        HOLD_MODS_CTRL_CMD => HoldMods::CtrlCmd,
        _ => HoldMods::Fn,
    }
}

/// Set the hold modifier combination for hold-to-talk
pub fn set_hold_mods(mods: HoldMods) {
    HOLD_MODS.store(encode_hold_mods(mods), AtomicOrdering::SeqCst);
    tracing::info!("HoldMods set to {:?}", mods);
}

/// Get the current hold modifier combination
pub fn get_hold_mods() -> HoldMods {
    decode_hold_mods(HOLD_MODS.load(AtomicOrdering::SeqCst))
}

// --- Global Toggle Trigger Setting ---

/// Atomic storage for ToggleTrigger (0=DoubleOption, 1=DoubleLeftOption, 2=DoubleRightOption, 3=DoubleCtrl, 4=None)
static TOGGLE_TRIGGER: AtomicU8 = AtomicU8::new(0);
const TOGGLE_TRIGGER_DOUBLE_OPTION: u8 = 0;
const TOGGLE_TRIGGER_DOUBLE_LEFT_OPTION: u8 = 1;
const TOGGLE_TRIGGER_DOUBLE_RIGHT_OPTION: u8 = 2;
const TOGGLE_TRIGGER_DOUBLE_CTRL: u8 = 3;
const TOGGLE_TRIGGER_NONE: u8 = 4;

fn encode_toggle_trigger(trigger: ToggleTrigger) -> u8 {
    match trigger {
        ToggleTrigger::DoubleOption => TOGGLE_TRIGGER_DOUBLE_OPTION,
        ToggleTrigger::DoubleLeftOption => TOGGLE_TRIGGER_DOUBLE_LEFT_OPTION,
        ToggleTrigger::DoubleRightOption => TOGGLE_TRIGGER_DOUBLE_RIGHT_OPTION,
        ToggleTrigger::DoubleCtrl => TOGGLE_TRIGGER_DOUBLE_CTRL,
        ToggleTrigger::None => TOGGLE_TRIGGER_NONE,
    }
}

fn decode_toggle_trigger(value: u8) -> ToggleTrigger {
    match value {
        TOGGLE_TRIGGER_DOUBLE_OPTION => ToggleTrigger::DoubleOption,
        TOGGLE_TRIGGER_DOUBLE_LEFT_OPTION => ToggleTrigger::DoubleLeftOption,
        TOGGLE_TRIGGER_DOUBLE_RIGHT_OPTION => ToggleTrigger::DoubleRightOption,
        TOGGLE_TRIGGER_DOUBLE_CTRL => ToggleTrigger::DoubleCtrl,
        _ => ToggleTrigger::None,
    }
}

/// Set the toggle trigger mode (thread-safe)
pub fn set_toggle_trigger(trigger: ToggleTrigger) {
    TOGGLE_TRIGGER.store(encode_toggle_trigger(trigger), AtomicOrdering::SeqCst);
    tracing::info!("Toggle trigger set to: {:?}", trigger);
}

/// Get the current toggle trigger mode (thread-safe)
pub fn get_toggle_trigger() -> ToggleTrigger {
    decode_toggle_trigger(TOGGLE_TRIGGER.load(AtomicOrdering::SeqCst))
}

// --- Global Exclusive Mode Setting ---
// Exclusive mode controls whether Shift/Cmd can act as mode modifiers for hold gestures.
// When enabled, we ignore Shift/Cmd and keep hold mode as RAW.

/// Atomic storage for exclusive mode (Shift/Cmd mode modifiers disabled)
static EXCLUSIVE_MODE: AtomicBool = AtomicBool::new(true);

/// Set exclusive mode (thread-safe)
/// When true, Shift/Cmd modifiers are ignored for hold mode
/// When false, Shift/Cmd can act as mode modifiers (Chat/Selection)
pub fn set_exclusive_mode(enabled: bool) {
    EXCLUSIVE_MODE.store(enabled, AtomicOrdering::SeqCst);
    tracing::info!("Hotkey exclusive mode set to: {}", enabled);
}

pub fn get_exclusive_mode() -> bool {
    EXCLUSIVE_MODE.load(AtomicOrdering::SeqCst)
}

// --- Hold start delay setting ---

/// Atomic storage for hold-to-talk start delay (milliseconds)
static HOLD_START_DELAY_MS: AtomicU64 = AtomicU64::new(800);

/// Set hold start delay (ms) used by hotkey gesture detector.
pub fn set_hold_start_delay_ms(ms: u64) {
    HOLD_START_DELAY_MS.store(ms, AtomicOrdering::SeqCst);
    tracing::info!("Hold start delay set to: {}ms", ms);
}

/// Get current hold start delay (ms) for hotkey gesture detector.
pub fn get_hold_start_delay_ms() -> u64 {
    HOLD_START_DELAY_MS.load(AtomicOrdering::SeqCst)
}

// --- Double-tap interval setting ---

/// Atomic storage for double-tap interval (milliseconds)
static DOUBLE_TAP_INTERVAL_MS: AtomicU64 = AtomicU64::new(200);

/// Set the double-tap interval (ms). Clamped to safe bounds.
pub fn set_double_tap_interval_ms(ms: u64) {
    let clamped = ms.clamp(100, 450);
    DOUBLE_TAP_INTERVAL_MS.store(clamped, AtomicOrdering::SeqCst);
    tracing::info!("Double-tap interval set to: {}ms", clamped);
}

/// Get the current double-tap interval (ms).
pub fn get_double_tap_interval_ms() -> u64 {
    DOUBLE_TAP_INTERVAL_MS.load(AtomicOrdering::SeqCst)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotkeyRuntimeConfig {
    pub hold_mods: HoldMods,
    pub toggle_trigger: ToggleTrigger,
    pub hold_exclusive: bool,
    pub hold_start_delay_ms: u64,
    pub double_tap_interval_ms: u64,
}

impl From<&Config> for HotkeyRuntimeConfig {
    fn from(config: &Config) -> Self {
        Self {
            hold_mods: config.hold_mods,
            toggle_trigger: config.toggle_trigger,
            hold_exclusive: config.hold_exclusive,
            hold_start_delay_ms: config.hold_start_delay_ms,
            double_tap_interval_ms: config.double_tap_interval_ms,
        }
    }
}

pub fn get_hotkey_runtime_config() -> HotkeyRuntimeConfig {
    HotkeyRuntimeConfig {
        hold_mods: get_hold_mods(),
        toggle_trigger: get_toggle_trigger(),
        hold_exclusive: get_exclusive_mode(),
        hold_start_delay_ms: get_hold_start_delay_ms(),
        double_tap_interval_ms: get_double_tap_interval_ms(),
    }
}

pub fn apply_hotkey_runtime_config(config: HotkeyRuntimeConfig) {
    set_hold_mods(config.hold_mods);
    set_toggle_trigger(config.toggle_trigger);
    set_exclusive_mode(config.hold_exclusive);
    set_hold_start_delay_ms(config.hold_start_delay_ms);
    set_double_tap_interval_ms(config.double_tap_interval_ms);
}

pub fn apply_hotkey_config(config: &Config) {
    apply_hotkey_runtime_config(HotkeyRuntimeConfig::from(config));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModeHotkeyBindings {
    pub dictation: ShortcutBinding,
    pub formatting: ShortcutBinding,
    pub assistive: ShortcutBinding,
}

impl ModeHotkeyBindings {
    pub fn from_settings(settings: &UserSettings) -> Self {
        Self {
            dictation: settings.mode_binding_for(WorkMode::Dictation),
            formatting: settings.mode_binding_for(WorkMode::Formatting),
            assistive: settings.mode_binding_for(WorkMode::Assistive),
        }
    }

    pub fn from_config(config: &Config) -> Self {
        let dictation = if config.toggle_trigger == ToggleTrigger::DoubleCtrl {
            ShortcutBinding::DoubleCtrl
        } else {
            config.hold_mods.as_shortcut_binding()
        };
        let formatting = match config.toggle_trigger {
            ToggleTrigger::DoubleOption | ToggleTrigger::DoubleLeftOption => {
                ShortcutBinding::DoubleLeftOption
            }
            ToggleTrigger::DoubleCtrl | ToggleTrigger::DoubleRightOption | ToggleTrigger::None => {
                ShortcutBinding::Disabled
            }
        };
        let assistive = match config.toggle_trigger {
            ToggleTrigger::DoubleOption | ToggleTrigger::DoubleRightOption => {
                ShortcutBinding::DoubleRightOption
            }
            ToggleTrigger::DoubleCtrl | ToggleTrigger::DoubleLeftOption | ToggleTrigger::None => {
                ShortcutBinding::Disabled
            }
        };

        Self {
            dictation,
            formatting,
            assistive,
        }
    }
}

// --- Constants ---

/// Max press duration for a "tap" gesture (milliseconds)
const TAP_MAX_MS: u64 = 220;

// --- Types ---

/// Represents the action of a hold gesture
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldAction {
    Down,
    Up,
}

/// High-level hold intent derived from modifier state.
///
/// UX split:
/// - `Raw`: dictation → auto-paste (fast)
/// - `Chat`: voice chat to AI → response in overlay (no auto-paste)
/// - `Selection`: apply instruction to selected text → response in overlay (no auto-paste)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HoldMode {
    #[default]
    Raw,
    Chat,
    Selection,
}

/// Hotkey event emitted by the listener
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyEvent {
    /// Hold gesture detected (press/release configured modifier combo)
    Hold {
        action: HoldAction,
        mode: HoldMode,
        force_ai: bool,
    },
    /// Modifier change while hold is active (e.g., add/remove Shift/Cmd).
    HoldUpdate { mode: HoldMode, force_ai: bool },
    /// Normal toggle gesture (double-tap left Option)
    ToggleNormal,
    /// Raw toggle gesture (double-tap Ctrl)
    ToggleRaw,
    /// Assistive toggle gesture (double-tap right Option)
    ToggleAssistive,
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
            self.ctrl == required.ctrl
                && self.alt == required.alt
                && self.shift == required.shift
                && self.cmd == required.cmd
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HotkeyModifierSnapshot {
    pub ctrl: bool,
    pub option: bool,
    pub shift: bool,
    pub cmd: bool,
    pub fn_key: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyPhysicalKey {
    LeftOption,
    RightOption,
    LeftControl,
    RightControl,
    Fn,
    Other,
}

impl HotkeyPhysicalKey {
    fn is_option(self) -> bool {
        matches!(self, Self::LeftOption | Self::RightOption)
    }

    fn is_right_option(self) -> bool {
        matches!(self, Self::RightOption)
    }

    fn is_ctrl(self) -> bool {
        matches!(self, Self::LeftControl | Self::RightControl)
    }

    fn is_fn(self) -> bool {
        matches!(self, Self::Fn)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyDetectorInput {
    KeyDown {
        now: Instant,
        modifiers: HotkeyModifierSnapshot,
    },
    FlagsChanged {
        now: Instant,
        key: HotkeyPhysicalKey,
        modifiers: HotkeyModifierSnapshot,
    },
}

#[derive(Debug, Clone)]
pub struct HotkeyDetector {
    hold_active: bool,
    hold_active_ts: Option<Instant>,
    hold_mode: HoldMode,
    hold_force_ai: bool,
    hold_event_sent: bool,
    last_left_tap_ts: Option<Instant>,
    last_right_tap_ts: Option<Instant>,
    last_ctrl_tap_ts: Option<Instant>,
    ctrl_down: bool,
    ctrl_down_ts: Option<Instant>,
    option_down: bool,
    option_side: Option<bool>,
    key_pressed_during_modifier: bool,
}

impl Default for HotkeyDetector {
    fn default() -> Self {
        Self {
            hold_active: false,
            hold_active_ts: None,
            hold_mode: HoldMode::Raw,
            hold_force_ai: false,
            hold_event_sent: false,
            last_left_tap_ts: None,
            last_right_tap_ts: None,
            last_ctrl_tap_ts: None,
            ctrl_down: false,
            ctrl_down_ts: None,
            option_down: false,
            option_side: None,
            key_pressed_during_modifier: false,
        }
    }
}

impl HotkeyDetector {
    pub fn feed(
        &mut self,
        input: HotkeyDetectorInput,
        config: HotkeyRuntimeConfig,
    ) -> Option<HotkeyEvent> {
        match input {
            HotkeyDetectorInput::KeyDown { now, modifiers } => {
                self.handle_key_down(now, modifiers, config)
            }
            HotkeyDetectorInput::FlagsChanged {
                now,
                key,
                modifiers,
            } => self.handle_flags_changed(now, key, modifiers, config),
        }
    }

    pub fn is_combo_active(&self) -> bool {
        self.hold_active
    }

    fn handle_key_down(
        &mut self,
        now: Instant,
        modifiers: HotkeyModifierSnapshot,
        config: HotkeyRuntimeConfig,
    ) -> Option<HotkeyEvent> {
        let mut emitted = None;
        let base_held = hold_base_pressed(modifiers, config.hold_mods);
        if base_held && self.hold_active {
            let in_delay_window = self
                .hold_active_ts
                .map(|ts| {
                    elapsed_between(now, ts) < Duration::from_millis(config.hold_start_delay_ms)
                })
                .unwrap_or(false);

            if in_delay_window {
                let mode = self.hold_mode;
                let force_ai = self.hold_force_ai;
                self.hold_active = false;
                self.hold_active_ts = None;
                self.hold_force_ai = false;
                self.hold_event_sent = false;
                self.key_pressed_during_modifier = true;
                emitted = Some(HotkeyEvent::Hold {
                    action: HoldAction::Up,
                    mode,
                    force_ai,
                });
            }
        }

        if modifiers.ctrl && (self.ctrl_down || self.hold_active) {
            self.key_pressed_during_modifier = true;
            self.last_ctrl_tap_ts = None;
        }

        if modifiers.option && self.option_down {
            self.key_pressed_during_modifier = true;
            self.last_left_tap_ts = None;
            self.last_right_tap_ts = None;
        }

        emitted
    }

    fn handle_flags_changed(
        &mut self,
        now: Instant,
        key: HotkeyPhysicalKey,
        modifiers: HotkeyModifierSnapshot,
        config: HotkeyRuntimeConfig,
    ) -> Option<HotkeyEvent> {
        let raw_toggle_enabled = matches!(config.toggle_trigger, ToggleTrigger::DoubleCtrl);
        let combo_active = if raw_toggle_enabled && matches!(config.hold_mods, HoldMods::Ctrl) {
            false
        } else {
            check_hold_combo(modifiers, config.hold_mods)
        };
        let mode_now = compute_hold_mode(
            modifiers.shift,
            modifiers.cmd,
            config.hold_mods,
            config.hold_exclusive,
        );
        let force_ai_now = compute_hold_force_ai(
            modifiers.option,
            modifiers.shift,
            modifiers.cmd,
            config.hold_mods,
        );

        let mut emitted = None;
        if combo_active && !self.hold_active {
            self.hold_active = true;
            self.hold_active_ts = Some(now);
            self.hold_mode = mode_now;
            self.hold_force_ai = force_ai_now;
            self.hold_event_sent = true;
            emitted = Some(HotkeyEvent::Hold {
                action: HoldAction::Down,
                mode: self.hold_mode,
                force_ai: self.hold_force_ai,
            });
        } else if combo_active
            && self.hold_active
            && (mode_now != self.hold_mode || force_ai_now != self.hold_force_ai)
        {
            self.hold_mode = mode_now;
            self.hold_force_ai = force_ai_now;
            emitted = Some(HotkeyEvent::HoldUpdate {
                mode: self.hold_mode,
                force_ai: self.hold_force_ai,
            });
        } else if !combo_active && self.hold_active {
            self.hold_active = false;
            if self.hold_event_sent {
                emitted = Some(HotkeyEvent::Hold {
                    action: HoldAction::Up,
                    mode: self.hold_mode,
                    force_ai: self.hold_force_ai,
                });
            }
            self.hold_active_ts = None;
            self.hold_force_ai = false;
        }

        if raw_toggle_enabled {
            let mut toggle_event = None;
            if key.is_ctrl() && modifiers.ctrl && !self.ctrl_down {
                self.ctrl_down = true;
                self.ctrl_down_ts = Some(now);
            } else if key.is_ctrl() && !modifiers.ctrl && self.ctrl_down {
                self.ctrl_down = false;
                let held_for = self
                    .ctrl_down_ts
                    .take()
                    .map(|ts| elapsed_between(now, ts))
                    .unwrap_or_default();

                if held_for <= Duration::from_millis(TAP_MAX_MS)
                    && !modifiers.shift
                    && !modifiers.option
                    && !modifiers.cmd
                    && !self.key_pressed_during_modifier
                {
                    toggle_event = register_double_tap(
                        &mut self.last_ctrl_tap_ts,
                        now,
                        config.double_tap_interval_ms,
                        HotkeyEvent::ToggleRaw,
                    );
                } else {
                    self.last_ctrl_tap_ts = None;
                    self.key_pressed_during_modifier = false;
                }
            }

            if !modifiers.ctrl && !modifiers.option && !modifiers.cmd {
                self.key_pressed_during_modifier = false;
            }

            return emitted.or(toggle_event);
        }

        if matches!(config.toggle_trigger, ToggleTrigger::None) {
            if key.is_option() {
                if modifiers.option {
                    self.option_down = true;
                    self.option_side = Some(key.is_right_option());
                } else {
                    self.option_down = false;
                    self.option_side = None;
                }
            } else if !modifiers.option {
                self.option_down = false;
                self.option_side = None;
            }
            return emitted;
        }

        if key.is_option() && modifiers.option && !self.option_down {
            self.option_down = true;
            self.option_side = Some(key.is_right_option());
        } else if !modifiers.option && self.option_down {
            self.option_down = false;
            let released_right = key.is_right_option();
            let pressed_side = self.option_side.take();

            if !key.is_option() {
                self.last_left_tap_ts = None;
                self.last_right_tap_ts = None;
                self.key_pressed_during_modifier = false;
                return emitted;
            }

            if let Some(pressed_right) = pressed_side
                && pressed_right != released_right
            {
                self.last_left_tap_ts = None;
                self.last_right_tap_ts = None;
                return emitted;
            }

            let hold_mods_block_toggle = match config.hold_mods {
                HoldMods::CtrlAlt => modifiers.ctrl || self.hold_active,
                _ => modifiers.ctrl || modifiers.cmd || self.hold_active,
            };

            if self.key_pressed_during_modifier {
                self.key_pressed_during_modifier = false;
                return emitted;
            }

            if !hold_mods_block_toggle {
                let normal_toggle_enabled = matches!(
                    config.toggle_trigger,
                    ToggleTrigger::DoubleOption | ToggleTrigger::DoubleLeftOption
                );
                let assistive_toggle_enabled = matches!(
                    config.toggle_trigger,
                    ToggleTrigger::DoubleOption | ToggleTrigger::DoubleRightOption
                );

                let toggle_event = if released_right {
                    self.last_left_tap_ts = None;
                    if assistive_toggle_enabled {
                        register_double_tap(
                            &mut self.last_right_tap_ts,
                            now,
                            config.double_tap_interval_ms,
                            HotkeyEvent::ToggleAssistive,
                        )
                    } else {
                        None
                    }
                } else if normal_toggle_enabled {
                    self.last_right_tap_ts = None;
                    register_double_tap(
                        &mut self.last_left_tap_ts,
                        now,
                        config.double_tap_interval_ms,
                        HotkeyEvent::ToggleNormal,
                    )
                } else {
                    None
                };

                emitted = emitted.or(toggle_event);
            }
        }

        if !modifiers.ctrl && !modifiers.option && !modifiers.cmd && !modifiers.fn_key {
            self.key_pressed_during_modifier = false;
        }

        emitted
    }
}

fn elapsed_between(now: Instant, previous: Instant) -> Duration {
    now.checked_duration_since(previous).unwrap_or_default()
}

fn register_double_tap(
    last_tap: &mut Option<Instant>,
    now: Instant,
    interval_ms: u64,
    event: HotkeyEvent,
) -> Option<HotkeyEvent> {
    if let Some(previous) = *last_tap
        && elapsed_between(now, previous) <= Duration::from_millis(interval_ms)
    {
        *last_tap = None;
        return Some(event);
    }

    *last_tap = Some(now);
    None
}

fn hold_base_pressed(modifiers: HotkeyModifierSnapshot, hold_mods: HoldMods) -> bool {
    match hold_mods {
        HoldMods::Fn => modifiers.fn_key,
        HoldMods::None => false,
        HoldMods::Ctrl => modifiers.ctrl,
        HoldMods::CtrlAlt => modifiers.ctrl,
        HoldMods::CtrlShift => modifiers.ctrl && modifiers.shift,
        HoldMods::CtrlCmd => modifiers.ctrl && modifiers.cmd,
    }
}

fn check_hold_combo(modifiers: HotkeyModifierSnapshot, hold_mods: HoldMods) -> bool {
    if modifiers.option && !matches!(hold_mods, HoldMods::CtrlAlt | HoldMods::Fn) {
        return false;
    }

    match hold_mods {
        HoldMods::Fn => modifiers.fn_key,
        HoldMods::None => false,
        HoldMods::Ctrl => modifiers.ctrl,
        HoldMods::CtrlAlt => modifiers.ctrl,
        HoldMods::CtrlShift => modifiers.ctrl && modifiers.shift,
        HoldMods::CtrlCmd => modifiers.ctrl && modifiers.cmd,
    }
}

fn compute_hold_mode(
    shift: bool,
    cmd: bool,
    hold_mods: HoldMods,
    hold_exclusive: bool,
) -> HoldMode {
    if hold_exclusive {
        return HoldMode::Raw;
    }

    match hold_mods {
        HoldMods::None => HoldMode::Raw,
        HoldMods::Ctrl => HoldMode::Raw,
        HoldMods::CtrlShift | HoldMods::CtrlCmd => HoldMode::Raw,
        HoldMods::CtrlAlt => {
            if cmd {
                HoldMode::Selection
            } else if shift {
                HoldMode::Chat
            } else {
                HoldMode::Raw
            }
        }
        HoldMods::Fn => {
            if shift {
                HoldMode::Chat
            } else if cmd {
                HoldMode::Selection
            } else {
                HoldMode::Raw
            }
        }
    }
}

fn compute_hold_force_ai(option: bool, shift: bool, cmd: bool, hold_mods: HoldMods) -> bool {
    match hold_mods {
        HoldMods::CtrlAlt => option && !shift && !cmd,
        _ => false,
    }
}

// --- macOS CGEventTap Implementation using raw bindings ---

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::ffi::c_void;
    use std::ptr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
    use std::sync::mpsc;
    use std::thread::{self, JoinHandle};

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
    const K_CG_EVENT_FLAG_MASK_SECONDARY_FN: CGEventFlags = 0x00800000;

    // CGEventField for keycode
    const K_CG_KEYBOARD_EVENT_KEYCODE: CGEventField = 9;

    // macOS virtual keycodes for Option keys
    const K_VK_OPTION: i64 = 58; // Left Option
    const K_VK_RIGHT_OPTION: i64 = 61; // Right Option
    // macOS virtual keycodes for Control keys
    const K_VK_CONTROL: i64 = 59; // Left Control
    const K_VK_RIGHT_CONTROL: i64 = 62; // Right Control
    const K_VK_FUNCTION: i64 = 63; // Fn (Globe)

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
        fn CFMachPortInvalidate(port: CFMachPortRef);

        fn CFRunLoopGetCurrent() -> CFRunLoopRef;
        fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: *const c_void);
        fn CFRunLoopSourceInvalidate(source: CFRunLoopSourceRef);
        fn CFRunLoopRun();
        fn CFRunLoopStop(rl: CFRunLoopRef);
        fn CFRunLoopWakeUp(rl: CFRunLoopRef);
        fn CFRelease(cf: *const c_void);

        static kCFRunLoopCommonModes: *const c_void;
    }

    struct HotkeyState {
        detector: HotkeyDetector,
        tx: Sender<HotkeyEvent>,
    }

    impl HotkeyState {
        fn new(tx: Sender<HotkeyEvent>) -> Self {
            Self {
                detector: HotkeyDetector::default(),
                tx,
            }
        }
    }

    static RUNNING: AtomicBool = AtomicBool::new(false);
    static ENABLED: AtomicBool = AtomicBool::new(true);

    struct RunningGuard;

    impl RunningGuard {
        fn acquire() -> Result<Self, String> {
            if RUNNING.swap(true, Ordering::SeqCst) {
                return Err("Hotkey listener already running".to_string());
            }
            Ok(Self)
        }
    }

    impl Drop for RunningGuard {
        fn drop(&mut self) {
            RUNNING.store(false, Ordering::SeqCst);
        }
    }

    #[derive(Default)]
    struct RuntimeControl {
        stop_requested: AtomicBool,
        tap: AtomicPtr<c_void>,
        source: AtomicPtr<c_void>,
        run_loop: AtomicPtr<c_void>,
    }

    impl RuntimeControl {
        fn is_stop_requested(&self) -> bool {
            self.stop_requested.load(Ordering::SeqCst)
        }

        fn request_stop(&self) {
            if self.stop_requested.swap(true, Ordering::SeqCst) {
                return;
            }

            let tap = self.tap.load(Ordering::SeqCst) as CFMachPortRef;
            if !tap.is_null() {
                unsafe {
                    CGEventTapEnable(tap, false);
                    CFMachPortInvalidate(tap);
                }
            }

            let source = self.source.load(Ordering::SeqCst) as CFRunLoopSourceRef;
            if !source.is_null() {
                unsafe {
                    CFRunLoopSourceInvalidate(source);
                }
            }

            let run_loop = self.run_loop.load(Ordering::SeqCst) as CFRunLoopRef;
            if !run_loop.is_null() {
                unsafe {
                    CFRunLoopStop(run_loop);
                    CFRunLoopWakeUp(run_loop);
                }
            }
        }
    }

    struct EventTapResources {
        state: Box<HotkeyState>,
        tap: Option<CFMachPortRef>,
        source: Option<CFRunLoopSourceRef>,
        run_loop: Option<CFRunLoopRef>,
        control: Arc<RuntimeControl>,
    }

    impl EventTapResources {
        fn new(tx: Sender<HotkeyEvent>, control: Arc<RuntimeControl>) -> Self {
            Self {
                state: Box::new(HotkeyState::new(tx)),
                tap: None,
                source: None,
                run_loop: None,
                control,
            }
        }

        fn user_info_ptr(&mut self) -> *mut c_void {
            (&mut *self.state as *mut HotkeyState).cast::<c_void>()
        }

        fn set_tap(&mut self, tap: CFMachPortRef) {
            self.tap = Some(tap);
            self.control
                .tap
                .store(tap.cast::<c_void>(), Ordering::SeqCst);
        }

        fn set_source(&mut self, source: CFRunLoopSourceRef) {
            self.source = Some(source);
            self.control
                .source
                .store(source.cast::<c_void>(), Ordering::SeqCst);
        }

        fn set_run_loop(&mut self, run_loop: CFRunLoopRef) {
            self.run_loop = Some(run_loop);
            self.control
                .run_loop
                .store(run_loop.cast::<c_void>(), Ordering::SeqCst);
        }
    }

    impl Drop for EventTapResources {
        fn drop(&mut self) {
            if let Some(tap) = self.tap {
                unsafe {
                    CGEventTapEnable(tap, false);
                    CFMachPortInvalidate(tap);
                }
            }

            if let Some(source) = self.source {
                unsafe {
                    CFRunLoopSourceInvalidate(source);
                }
            }

            if let Some(run_loop) = self.run_loop {
                unsafe {
                    CFRunLoopStop(run_loop);
                    CFRunLoopWakeUp(run_loop);
                }
            }

            if let Some(source) = self.source.take() {
                unsafe {
                    CFRelease(source as *const c_void);
                }
            }
            if let Some(tap) = self.tap.take() {
                unsafe {
                    CFRelease(tap as *const c_void);
                }
            }

            self.control.tap.store(ptr::null_mut(), Ordering::SeqCst);
            self.control.source.store(ptr::null_mut(), Ordering::SeqCst);
            self.control
                .run_loop
                .store(ptr::null_mut(), Ordering::SeqCst);
        }
    }

    pub struct HotkeyRuntime {
        control: Arc<RuntimeControl>,
        worker: Option<JoinHandle<()>>,
        running_guard: Option<RunningGuard>,
    }

    impl HotkeyRuntime {
        fn new(
            control: Arc<RuntimeControl>,
            worker: JoinHandle<()>,
            running_guard: RunningGuard,
        ) -> Self {
            Self {
                control,
                worker: Some(worker),
                running_guard: Some(running_guard),
            }
        }

        pub fn shutdown(&mut self) {
            if self.worker.is_none() && self.running_guard.is_none() {
                return;
            }

            self.control.request_stop();
            if let Some(worker) = self.worker.take()
                && worker.join().is_err()
            {
                tracing::warn!("Hotkey worker thread panicked during shutdown");
            }
            self.running_guard.take();
        }
    }

    impl Drop for HotkeyRuntime {
        fn drop(&mut self) {
            self.shutdown();
        }
    }

    fn modifiers_from_flags(flags: CGEventFlags) -> HotkeyModifierSnapshot {
        HotkeyModifierSnapshot {
            ctrl: (flags & K_CG_EVENT_FLAG_MASK_CONTROL) != 0,
            shift: (flags & K_CG_EVENT_FLAG_MASK_SHIFT) != 0,
            option: (flags & K_CG_EVENT_FLAG_MASK_ALTERNATE) != 0,
            cmd: (flags & K_CG_EVENT_FLAG_MASK_COMMAND) != 0,
            fn_key: (flags & K_CG_EVENT_FLAG_MASK_SECONDARY_FN) != 0,
        }
    }

    fn map_keycode(keycode: i64) -> HotkeyPhysicalKey {
        match keycode {
            K_VK_OPTION => HotkeyPhysicalKey::LeftOption,
            K_VK_RIGHT_OPTION => HotkeyPhysicalKey::RightOption,
            K_VK_CONTROL => HotkeyPhysicalKey::LeftControl,
            K_VK_RIGHT_CONTROL => HotkeyPhysicalKey::RightControl,
            K_VK_FUNCTION => HotkeyPhysicalKey::Fn,
            _ => HotkeyPhysicalKey::Other,
        }
    }

    /// CGEventTap callback - thin adapter from CoreGraphics events to HotkeyDetector input.
    extern "C" fn event_callback(
        _proxy: CGEventTapProxy,
        event_type: CGEventType,
        event: CGEventRef,
        user_info: *mut c_void,
    ) -> CGEventRef {
        // Skip processing if hotkeys are disabled
        if !ENABLED.load(Ordering::Relaxed) {
            return event;
        }

        let state_ptr = user_info.cast::<HotkeyState>();
        if state_ptr.is_null() {
            return event;
        }
        let state = unsafe { &mut *state_ptr };

        let flags = unsafe { CGEventGetFlags(event) };
        let modifiers = modifiers_from_flags(flags);
        let now = Instant::now();
        let runtime_config = get_hotkey_runtime_config();

        let (input, swallow_fn_event) = match event_type {
            K_CG_EVENT_KEY_DOWN => (HotkeyDetectorInput::KeyDown { now, modifiers }, false),
            K_CG_EVENT_FLAGS_CHANGED => {
                let keycode =
                    unsafe { CGEventGetIntegerValueField(event, K_CG_KEYBOARD_EVENT_KEYCODE) };
                let key = map_keycode(keycode);

                tracing::debug!(
                    "CGEventTap: flags=0x{:X} keycode={} (ctrl={}, shift={}, opt={}, cmd={}, fn={})",
                    flags,
                    keycode,
                    modifiers.ctrl,
                    modifiers.shift,
                    modifiers.option,
                    modifiers.cmd,
                    modifiers.fn_key
                );

                (
                    HotkeyDetectorInput::FlagsChanged {
                        now,
                        key,
                        modifiers,
                    },
                    matches!(runtime_config.hold_mods, HoldMods::Fn) && key.is_fn(),
                )
            }
            _ => return event,
        };

        if let Some(hotkey_event) = state.detector.feed(input, runtime_config) {
            let _ = state.tx.send(hotkey_event);
        }

        if swallow_fn_event {
            // Swallow Fn events to avoid the system emoji picker.
            return ptr::null_mut();
        }

        event
    }
    /// Start the hotkey listener on a background thread and return its runtime owner.
    pub fn start_listener(tx: Sender<HotkeyEvent>) -> Result<HotkeyRuntime, String> {
        let running_guard = RunningGuard::acquire()?;
        let control = Arc::new(RuntimeControl::default());
        let worker_control = Arc::clone(&control);

        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let worker = thread::spawn(move || {
            if let Err(e) = run_event_tap(tx, worker_control, ready_tx) {
                tracing::error!("CGEventTap error: {}", e);
            }
        });

        let mut runtime = HotkeyRuntime::new(control, worker, running_guard);

        // Wait for startup confirmation so we can surface permission errors.
        match ready_rx.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(())) => Ok(runtime),
            Ok(Err(e)) => {
                runtime.shutdown();
                Err(e)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                runtime.shutdown();
                Err(
                    "Timed out while starting CGEventTap (hotkeys). Check Accessibility permission."
                        .to_string(),
                )
            }
            Err(e) => {
                runtime.shutdown();
                Err(format!("Failed to start hotkeys: {}", e))
            }
        }
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
    fn run_event_tap(
        tx: Sender<HotkeyEvent>,
        control: Arc<RuntimeControl>,
        ready_tx: mpsc::Sender<Result<(), String>>,
    ) -> Result<(), String> {
        let mut resources = EventTapResources::new(tx, control);

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
                resources.user_info_ptr(),
            )
        };

        if tap.is_null() {
            let msg = "Failed to create CGEventTap - check Accessibility permission".to_string();
            let _ = ready_tx.send(Err(msg.clone()));
            return Err(msg);
        }
        resources.set_tap(tap);

        // Enable the tap
        unsafe {
            CGEventTapEnable(tap, true);
        }

        // Verify tap is actually enabled
        let is_enabled = unsafe { CGEventTapIsEnabled(tap) };
        if !is_enabled {
            tracing::error!("CGEventTap failed to enable! macOS may have denied it.");
            let msg = "CGEventTap not enabled - macOS denied access".to_string();
            let _ = ready_tx.send(Err(msg.clone()));
            return Err(msg);
        }
        tracing::debug!("CGEventTap verified as enabled");

        // Create run loop source
        let source = unsafe { CFMachPortCreateRunLoopSource(ptr::null(), tap, 0) };

        if source.is_null() {
            let msg = "Failed to create run loop source".to_string();
            let _ = ready_tx.send(Err(msg.clone()));
            return Err(msg);
        }
        resources.set_source(source);

        // Add to run loop
        let run_loop = unsafe { CFRunLoopGetCurrent() };
        resources.set_run_loop(run_loop);
        unsafe {
            CFRunLoopAddSource(run_loop, source, kCFRunLoopCommonModes);
        }

        let hold_mods = get_hold_mods();
        let toggle_trigger = get_toggle_trigger();
        tracing::info!(
            "CGEventTap started, monitoring {:?} hold and Option double-tap (left=normal, right=assistive, trigger={:?})",
            hold_mods,
            toggle_trigger
        );
        let _ = ready_tx.send(Ok(()));

        // Run until an explicit shutdown request stops this run loop.
        tracing::debug!("Entering CFRunLoopRun (blocks until stop)");
        if resources.control.is_stop_requested() {
            unsafe {
                CFRunLoopStop(run_loop);
                CFRunLoopWakeUp(run_loop);
            }
        } else {
            unsafe {
                CFRunLoopRun();
            }
        }

        tracing::info!("CGEventTap run loop exited");

        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::Mutex;

        static LIFECYCLE_TEST_LOCK: Mutex<()> = Mutex::new(());

        fn spawn_test_runtime() -> HotkeyRuntime {
            let running_guard = RunningGuard::acquire().expect("test runtime should acquire guard");
            let control = Arc::new(RuntimeControl::default());
            let worker_control = Arc::clone(&control);
            let worker = thread::spawn(move || {
                while !worker_control.is_stop_requested() {
                    thread::sleep(Duration::from_millis(5));
                }
            });
            HotkeyRuntime::new(control, worker, running_guard)
        }

        #[test]
        fn compute_hold_mode_respects_modifiers() {
            // Fn base with Shift/Cmd modifiers
            assert_eq!(
                compute_hold_mode(false, false, HoldMods::Fn, false),
                HoldMode::Raw
            );
            assert_eq!(
                compute_hold_mode(true, false, HoldMods::Fn, false),
                HoldMode::Chat
            );
            assert_eq!(
                compute_hold_mode(false, true, HoldMods::Fn, false),
                HoldMode::Selection
            );

            // Ctrl-only ignores Shift/Cmd modifiers
            assert_eq!(
                compute_hold_mode(true, false, HoldMods::Ctrl, false),
                HoldMode::Raw
            );
            assert_eq!(
                compute_hold_mode(false, true, HoldMods::Ctrl, false),
                HoldMode::Raw
            );

            // Ctrl+Option allows modifiers
            assert_eq!(
                compute_hold_mode(true, false, HoldMods::CtrlAlt, false),
                HoldMode::Chat
            );
            assert_eq!(
                compute_hold_mode(false, true, HoldMods::CtrlAlt, false),
                HoldMode::Selection
            );
            assert_eq!(
                compute_hold_mode(false, false, HoldMods::CtrlAlt, false),
                HoldMode::Raw
            );

            // Ctrl+Shift/Cmd are fixed to raw
            assert_eq!(
                compute_hold_mode(true, false, HoldMods::CtrlShift, false),
                HoldMode::Raw
            );
            assert_eq!(
                compute_hold_mode(false, true, HoldMods::CtrlCmd, false),
                HoldMode::Raw
            );
        }

        #[test]
        fn compute_hold_mode_exclusive_forces_raw() {
            assert_eq!(
                compute_hold_mode(true, true, HoldMods::Fn, true),
                HoldMode::Raw
            );
            assert_eq!(
                compute_hold_mode(true, true, HoldMods::CtrlAlt, true),
                HoldMode::Raw
            );
        }

        #[test]
        fn running_guard_blocks_double_start() {
            let _guard = LIFECYCLE_TEST_LOCK
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            RUNNING.store(false, Ordering::SeqCst);

            let first = RunningGuard::acquire().expect("first start must succeed");
            assert!(RunningGuard::acquire().is_err());
            drop(first);

            let second = RunningGuard::acquire().expect("second start after drop must succeed");
            drop(second);
            assert!(!RUNNING.load(Ordering::SeqCst));
        }

        #[test]
        fn runtime_shutdown_is_idempotent() {
            let _guard = LIFECYCLE_TEST_LOCK
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            RUNNING.store(false, Ordering::SeqCst);

            let mut runtime = spawn_test_runtime();
            runtime.shutdown();
            runtime.shutdown();

            assert!(!RUNNING.load(Ordering::SeqCst));
        }

        #[test]
        fn runtime_drop_stops_worker_without_panic() {
            let _guard = LIFECYCLE_TEST_LOCK
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            RUNNING.store(false, Ordering::SeqCst);

            {
                let _runtime = spawn_test_runtime();
                assert!(RUNNING.load(Ordering::SeqCst));
            }

            assert!(!RUNNING.load(Ordering::SeqCst));
        }
    }
}

// --- Fallback for non-macOS ---

#[cfg(not(target_os = "macos"))]
mod macos {
    use super::*;

    pub struct HotkeyRuntime;

    impl HotkeyRuntime {
        pub fn shutdown(&mut self) {}
    }

    pub fn start_listener(_tx: Sender<HotkeyEvent>) -> Result<HotkeyRuntime, String> {
        tracing::warn!("Hotkey listener not supported on this platform");
        Ok(HotkeyRuntime)
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

/// Manages global hotkey runtime ownership.
///
/// Owns the macOS event tap worker thread and tears it down on `shutdown()`/`Drop`.
/// Runtime starts in `new`; there is no separate `start`/`process` lifecycle.
pub struct HotkeyManager {
    /// Kept for future use (e.g., manual event injection)
    _tx: Sender<HotkeyEvent>,
    runtime: Option<macos::HotkeyRuntime>,
}

impl HotkeyManager {
    /// Create a new HotkeyManager
    ///
    /// IMPORTANT: On macOS, starts a background thread for CGEventTap.
    /// Requires Accessibility permission.
    pub fn new(tx: Sender<HotkeyEvent>) -> Result<Self, String> {
        let runtime = macos::start_listener(tx.clone())?;

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
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    static HOTKEY_ATOMICS_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn test_config(hold_mods: HoldMods, toggle_trigger: ToggleTrigger) -> HotkeyRuntimeConfig {
        HotkeyRuntimeConfig {
            hold_mods,
            toggle_trigger,
            hold_exclusive: false,
            hold_start_delay_ms: 800,
            double_tap_interval_ms: 200,
        }
    }

    fn mods(
        ctrl: bool,
        option: bool,
        shift: bool,
        cmd: bool,
        fn_key: bool,
    ) -> HotkeyModifierSnapshot {
        HotkeyModifierSnapshot {
            ctrl,
            option,
            shift,
            cmd,
            fn_key,
        }
    }

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

        let current_with_shift = ModifierFlags {
            ctrl: true,
            alt: false,
            shift: true,
            cmd: false,
        };
        assert!(!current_with_shift.matches(&required, true));

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
            alt: true,
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
        let _guard = HOTKEY_ATOMICS_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        set_toggle_trigger(ToggleTrigger::DoubleOption);
        assert_eq!(get_toggle_trigger(), ToggleTrigger::DoubleOption);
        set_toggle_trigger(ToggleTrigger::DoubleLeftOption);
        assert_eq!(get_toggle_trigger(), ToggleTrigger::DoubleLeftOption);
        set_toggle_trigger(ToggleTrigger::DoubleRightOption);
        assert_eq!(get_toggle_trigger(), ToggleTrigger::DoubleRightOption);
        set_toggle_trigger(ToggleTrigger::DoubleCtrl);
        assert_eq!(get_toggle_trigger(), ToggleTrigger::DoubleCtrl);
        set_toggle_trigger(ToggleTrigger::None);
        assert_eq!(get_toggle_trigger(), ToggleTrigger::None);
        set_toggle_trigger(ToggleTrigger::DoubleOption);
    }

    #[test]
    fn test_hold_mods_get_set() {
        let _guard = HOTKEY_ATOMICS_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        set_hold_mods(HoldMods::Fn);
        assert_eq!(get_hold_mods(), HoldMods::Fn);
        set_hold_mods(HoldMods::None);
        assert_eq!(get_hold_mods(), HoldMods::None);
        set_hold_mods(HoldMods::Ctrl);
        assert_eq!(get_hold_mods(), HoldMods::Ctrl);
        set_hold_mods(HoldMods::CtrlAlt);
        assert_eq!(get_hold_mods(), HoldMods::CtrlAlt);
        set_hold_mods(HoldMods::CtrlShift);
        assert_eq!(get_hold_mods(), HoldMods::CtrlShift);
        set_hold_mods(HoldMods::CtrlCmd);
        assert_eq!(get_hold_mods(), HoldMods::CtrlCmd);
        set_hold_mods(HoldMods::Fn);
    }

    #[test]
    fn test_double_tap_interval_get_set() {
        let _guard = HOTKEY_ATOMICS_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        set_double_tap_interval_ms(200);
        assert_eq!(get_double_tap_interval_ms(), 200);
        set_double_tap_interval_ms(50);
        assert_eq!(get_double_tap_interval_ms(), 100);
        set_double_tap_interval_ms(999);
        assert_eq!(get_double_tap_interval_ms(), 450);
    }

    #[test]
    fn test_apply_hotkey_runtime_config_updates_all_atomics() {
        let _guard = HOTKEY_ATOMICS_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let runtime = HotkeyRuntimeConfig {
            hold_mods: HoldMods::CtrlAlt,
            toggle_trigger: ToggleTrigger::DoubleCtrl,
            hold_exclusive: true,
            hold_start_delay_ms: 1234,
            double_tap_interval_ms: 260,
        };
        apply_hotkey_runtime_config(runtime);

        assert_eq!(get_hold_mods(), runtime.hold_mods);
        assert_eq!(get_toggle_trigger(), runtime.toggle_trigger);
        assert_eq!(get_exclusive_mode(), runtime.hold_exclusive);
        assert_eq!(get_hold_start_delay_ms(), runtime.hold_start_delay_ms);
        assert_eq!(
            get_double_tap_interval_ms(),
            runtime.double_tap_interval_ms.clamp(100, 450)
        );
    }

    #[test]
    fn detector_option_double_tap_window_table() {
        let table = [(200_u64, true), (201_u64, false)];

        for (gap_ms, expect_toggle) in table {
            let mut detector = HotkeyDetector::default();
            let config = test_config(HoldMods::Fn, ToggleTrigger::DoubleOption);
            let base = Instant::now();

            assert_eq!(
                detector.feed(
                    HotkeyDetectorInput::FlagsChanged {
                        now: base,
                        key: HotkeyPhysicalKey::LeftOption,
                        modifiers: mods(false, true, false, false, false),
                    },
                    config,
                ),
                None
            );
            assert_eq!(
                detector.feed(
                    HotkeyDetectorInput::FlagsChanged {
                        now: base + Duration::from_millis(1),
                        key: HotkeyPhysicalKey::LeftOption,
                        modifiers: mods(false, false, false, false, false),
                    },
                    config,
                ),
                None
            );
            assert_eq!(
                detector.feed(
                    HotkeyDetectorInput::FlagsChanged {
                        now: base + Duration::from_millis(gap_ms),
                        key: HotkeyPhysicalKey::LeftOption,
                        modifiers: mods(false, true, false, false, false),
                    },
                    config,
                ),
                None
            );

            let second_release = detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(gap_ms + 1),
                    key: HotkeyPhysicalKey::LeftOption,
                    modifiers: mods(false, false, false, false, false),
                },
                config,
            );
            assert_eq!(
                second_release,
                if expect_toggle {
                    Some(HotkeyEvent::ToggleNormal)
                } else {
                    None
                }
            );
        }
    }

    #[test]
    fn detector_cancels_hold_on_keydown_during_delay() {
        let mut detector = HotkeyDetector::default();
        let mut config = test_config(HoldMods::Ctrl, ToggleTrigger::None);
        config.hold_start_delay_ms = 800;
        let base = Instant::now();

        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base,
                    key: HotkeyPhysicalKey::LeftControl,
                    modifiers: mods(true, false, false, false, false),
                },
                config,
            ),
            Some(HotkeyEvent::Hold {
                action: HoldAction::Down,
                mode: HoldMode::Raw,
                force_ai: false,
            })
        );

        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::KeyDown {
                    now: base + Duration::from_millis(200),
                    modifiers: mods(true, false, false, false, false),
                },
                config,
            ),
            Some(HotkeyEvent::Hold {
                action: HoldAction::Up,
                mode: HoldMode::Raw,
                force_ai: false,
            })
        );

        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(260),
                    key: HotkeyPhysicalKey::LeftControl,
                    modifiers: mods(false, false, false, false, false),
                },
                config,
            ),
            None
        );
        assert!(!detector.is_combo_active());
    }

    #[test]
    fn detector_resets_combo_flags_after_option_combo() {
        let mut detector = HotkeyDetector::default();
        let config = test_config(HoldMods::Fn, ToggleTrigger::DoubleOption);
        let base = Instant::now();

        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base,
                    key: HotkeyPhysicalKey::LeftOption,
                    modifiers: mods(false, true, false, false, false),
                },
                config,
            ),
            None
        );
        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(1),
                    key: HotkeyPhysicalKey::LeftOption,
                    modifiers: mods(false, false, false, false, false),
                },
                config,
            ),
            None
        );

        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(40),
                    key: HotkeyPhysicalKey::LeftOption,
                    modifiers: mods(false, true, false, false, false),
                },
                config,
            ),
            None
        );
        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::KeyDown {
                    now: base + Duration::from_millis(45),
                    modifiers: mods(false, true, false, false, false),
                },
                config,
            ),
            None
        );
        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(50),
                    key: HotkeyPhysicalKey::LeftOption,
                    modifiers: mods(false, false, false, false, false),
                },
                config,
            ),
            None
        );

        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(120),
                    key: HotkeyPhysicalKey::LeftOption,
                    modifiers: mods(false, true, false, false, false),
                },
                config,
            ),
            None
        );
        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(121),
                    key: HotkeyPhysicalKey::LeftOption,
                    modifiers: mods(false, false, false, false, false),
                },
                config,
            ),
            None
        );
        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(170),
                    key: HotkeyPhysicalKey::LeftOption,
                    modifiers: mods(false, true, false, false, false),
                },
                config,
            ),
            None
        );
        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(171),
                    key: HotkeyPhysicalKey::LeftOption,
                    modifiers: mods(false, false, false, false, false),
                },
                config,
            ),
            Some(HotkeyEvent::ToggleNormal)
        );
    }

    #[test]
    fn detector_raw_toggle_double_ctrl_and_combo_reset() {
        let mut detector = HotkeyDetector::default();
        let config = test_config(HoldMods::Ctrl, ToggleTrigger::DoubleCtrl);
        let base = Instant::now();

        let first_event = detector.feed(
            HotkeyDetectorInput::FlagsChanged {
                now: base,
                key: HotkeyPhysicalKey::LeftControl,
                modifiers: mods(true, false, false, false, false),
            },
            config,
        );
        assert!(
            matches!(
                first_event,
                None | Some(HotkeyEvent::Hold {
                    action: HoldAction::Down,
                    mode: HoldMode::Raw,
                    force_ai: false
                })
            ),
            "unexpected first ctrl event: {:?}",
            first_event
        );
        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::KeyDown {
                    now: base + Duration::from_millis(10),
                    modifiers: mods(true, false, false, false, false),
                },
                config,
            ),
            None
        );
        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(20),
                    key: HotkeyPhysicalKey::LeftControl,
                    modifiers: mods(false, false, false, false, false),
                },
                config,
            ),
            None
        );

        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(100),
                    key: HotkeyPhysicalKey::LeftControl,
                    modifiers: mods(true, false, false, false, false),
                },
                config,
            ),
            None
        );
        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(110),
                    key: HotkeyPhysicalKey::LeftControl,
                    modifiers: mods(false, false, false, false, false),
                },
                config,
            ),
            None
        );
        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(170),
                    key: HotkeyPhysicalKey::LeftControl,
                    modifiers: mods(true, false, false, false, false),
                },
                config,
            ),
            None
        );
        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(180),
                    key: HotkeyPhysicalKey::LeftControl,
                    modifiers: mods(false, false, false, false, false),
                },
                config,
            ),
            Some(HotkeyEvent::ToggleRaw)
        );
    }
}
