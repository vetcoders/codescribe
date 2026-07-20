use crate::config::{Config, DeferredInsertShortcut, ShortcutBinding, UserSettings, WorkMode};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, AtomicU64, Ordering as AtomicOrdering};

const BIND_DISABLED: u16 = 0;
const BIND_HOLD_FN: u16 = 1;
const BIND_HOLD_CTRL: u16 = 2;
const BIND_HOLD_CTRL_ALT: u16 = 3;
const BIND_HOLD_CTRL_SHIFT: u16 = 4;
const BIND_HOLD_CTRL_CMD: u16 = 5;
const BIND_DOUBLE_CTRL: u16 = 6;
const BIND_DOUBLE_LEFT_OPTION: u16 = 7;
const BIND_DOUBLE_RIGHT_OPTION: u16 = 8;

const DEFAULT_MODE_BINDINGS_ENCODED: u16 =
    BIND_HOLD_FN | (BIND_DOUBLE_LEFT_OPTION << 4) | (BIND_DOUBLE_RIGHT_OPTION << 8);

// --- Global Mode Binding Contract ---
// Runtime source of truth: mode -> binding mapping, not legacy hold/toggle fields.
static MODE_HOTKEY_BINDINGS: AtomicU16 = AtomicU16::new(DEFAULT_MODE_BINDINGS_ENCODED);

fn encode_shortcut_binding(binding: ShortcutBinding) -> u16 {
    match binding {
        ShortcutBinding::Disabled => BIND_DISABLED,
        ShortcutBinding::HoldFn => BIND_HOLD_FN,
        ShortcutBinding::HoldCtrl => BIND_HOLD_CTRL,
        ShortcutBinding::HoldCtrlAlt => BIND_HOLD_CTRL_ALT,
        ShortcutBinding::HoldCtrlShift => BIND_HOLD_CTRL_SHIFT,
        ShortcutBinding::HoldCtrlCmd => BIND_HOLD_CTRL_CMD,
        ShortcutBinding::DoubleCtrl => BIND_DOUBLE_CTRL,
        ShortcutBinding::DoubleLeftOption => BIND_DOUBLE_LEFT_OPTION,
        ShortcutBinding::DoubleRightOption => BIND_DOUBLE_RIGHT_OPTION,
    }
}

fn decode_shortcut_binding(value: u16) -> ShortcutBinding {
    match value {
        BIND_DISABLED => ShortcutBinding::Disabled,
        BIND_HOLD_FN => ShortcutBinding::HoldFn,
        BIND_HOLD_CTRL => ShortcutBinding::HoldCtrl,
        BIND_HOLD_CTRL_ALT => ShortcutBinding::HoldCtrlAlt,
        BIND_HOLD_CTRL_SHIFT => ShortcutBinding::HoldCtrlShift,
        BIND_HOLD_CTRL_CMD => ShortcutBinding::HoldCtrlCmd,
        BIND_DOUBLE_CTRL => ShortcutBinding::DoubleCtrl,
        BIND_DOUBLE_LEFT_OPTION => ShortcutBinding::DoubleLeftOption,
        BIND_DOUBLE_RIGHT_OPTION => ShortcutBinding::DoubleRightOption,
        _ => ShortcutBinding::Disabled,
    }
}

fn encode_mode_hotkey_bindings(bindings: ModeHotkeyBindings) -> u16 {
    encode_shortcut_binding(bindings.dictation)
        | (encode_shortcut_binding(bindings.formatting) << 4)
        | (encode_shortcut_binding(bindings.assistive) << 8)
}

fn decode_mode_hotkey_bindings(raw: u16) -> ModeHotkeyBindings {
    ModeHotkeyBindings {
        dictation: decode_shortcut_binding(raw & 0x0F),
        formatting: decode_shortcut_binding((raw >> 4) & 0x0F),
        assistive: decode_shortcut_binding((raw >> 8) & 0x0F),
    }
}

pub fn set_mode_hotkey_bindings(bindings: ModeHotkeyBindings) {
    MODE_HOTKEY_BINDINGS.store(
        encode_mode_hotkey_bindings(bindings),
        AtomicOrdering::SeqCst,
    );
    tracing::info!(
        "Mode bindings set: dictation={:?}, formatting={:?}, assistive={:?}",
        bindings.dictation,
        bindings.formatting,
        bindings.assistive
    );
}

pub fn get_mode_hotkey_bindings() -> ModeHotkeyBindings {
    decode_mode_hotkey_bindings(MODE_HOTKEY_BINDINGS.load(AtomicOrdering::SeqCst))
}

// --- Global Exclusive Mode Setting ---
// Exclusive mode controls whether Shift/Cmd can act as mode modifiers for hold gestures.
// When enabled, we ignore Shift/Cmd and keep hold mode as RAW.

/// Atomic storage for exclusive mode (Shift/Cmd mode modifiers disabled)
// Default FALSE so the documented Fn+Shift→Chat / Fn+Cmd→Selection modifiers work
// out of the box (HOTKEYS_CONTRACT.md §"Mode modifiers"). Matches Config::default's
// `hold_exclusive: false`. Exclusive (Fn-hold is raw-only) is opt-in via HOLD_EXCLUSIVE=1.
static EXCLUSIVE_MODE: AtomicBool = AtomicBool::new(false);

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

const DEFERRED_INSERT_DISABLED: u8 = 0;
const DEFERRED_INSERT_COMMAND_OPTION_V: u8 = 1;
const DEFERRED_INSERT_COMMAND_SHIFT_V: u8 = 2;
const DEFERRED_INSERT_COMMAND_CONTROL_V: u8 = 3;

static DEFERRED_INSERT_SHORTCUT: AtomicU8 = AtomicU8::new(DEFERRED_INSERT_COMMAND_OPTION_V);

fn encode_deferred_insert_shortcut(shortcut: DeferredInsertShortcut) -> u8 {
    match shortcut {
        DeferredInsertShortcut::Disabled => DEFERRED_INSERT_DISABLED,
        DeferredInsertShortcut::CommandOptionV => DEFERRED_INSERT_COMMAND_OPTION_V,
        DeferredInsertShortcut::CommandShiftV => DEFERRED_INSERT_COMMAND_SHIFT_V,
        DeferredInsertShortcut::CommandControlV => DEFERRED_INSERT_COMMAND_CONTROL_V,
    }
}

fn decode_deferred_insert_shortcut(value: u8) -> DeferredInsertShortcut {
    match value {
        DEFERRED_INSERT_COMMAND_OPTION_V => DeferredInsertShortcut::CommandOptionV,
        DEFERRED_INSERT_COMMAND_SHIFT_V => DeferredInsertShortcut::CommandShiftV,
        DEFERRED_INSERT_COMMAND_CONTROL_V => DeferredInsertShortcut::CommandControlV,
        _ => DeferredInsertShortcut::Disabled,
    }
}

pub fn set_deferred_insert_shortcut(shortcut: DeferredInsertShortcut) {
    DEFERRED_INSERT_SHORTCUT.store(
        encode_deferred_insert_shortcut(shortcut),
        AtomicOrdering::SeqCst,
    );
    tracing::info!(label = shortcut.label(), "Deferred insert shortcut set");
}

pub fn get_deferred_insert_shortcut() -> DeferredInsertShortcut {
    decode_deferred_insert_shortcut(DEFERRED_INSERT_SHORTCUT.load(AtomicOrdering::SeqCst))
}

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
    pub mode_bindings: ModeHotkeyBindings,
    pub hold_exclusive: bool,
    pub hold_start_delay_ms: u64,
    pub double_tap_interval_ms: u64,
    pub deferred_insert_shortcut: DeferredInsertShortcut,
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

    pub fn load() -> Self {
        Self::from_settings(&UserSettings::load())
    }
}

impl From<&Config> for HotkeyRuntimeConfig {
    fn from(config: &Config) -> Self {
        Self {
            mode_bindings: ModeHotkeyBindings::load(),
            hold_exclusive: config.hold_exclusive,
            hold_start_delay_ms: config.hold_start_delay_ms,
            double_tap_interval_ms: config.double_tap_interval_ms,
            deferred_insert_shortcut: config.deferred_insert_shortcut,
        }
    }
}

pub fn get_hotkey_runtime_config() -> HotkeyRuntimeConfig {
    HotkeyRuntimeConfig {
        mode_bindings: get_mode_hotkey_bindings(),
        hold_exclusive: get_exclusive_mode(),
        hold_start_delay_ms: get_hold_start_delay_ms(),
        double_tap_interval_ms: get_double_tap_interval_ms(),
        deferred_insert_shortcut: get_deferred_insert_shortcut(),
    }
}

pub fn apply_hotkey_runtime_config(config: HotkeyRuntimeConfig) {
    set_mode_hotkey_bindings(config.mode_bindings);
    set_exclusive_mode(config.hold_exclusive);
    set_hold_start_delay_ms(config.hold_start_delay_ms);
    set_double_tap_interval_ms(config.double_tap_interval_ms);
    set_deferred_insert_shortcut(config.deferred_insert_shortcut);
}

pub fn apply_hotkey_config(config: &Config) {
    apply_hotkey_runtime_config(HotkeyRuntimeConfig::from(config));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static HOTKEY_ATOMICS_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_mode_hotkey_bindings_get_set() {
        let _guard = HOTKEY_ATOMICS_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let bindings = ModeHotkeyBindings {
            dictation: ShortcutBinding::HoldCtrlAlt,
            formatting: ShortcutBinding::DoubleLeftOption,
            assistive: ShortcutBinding::Disabled,
        };
        set_mode_hotkey_bindings(bindings);
        assert_eq!(get_mode_hotkey_bindings(), bindings);
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
            mode_bindings: ModeHotkeyBindings {
                dictation: ShortcutBinding::DoubleCtrl,
                formatting: ShortcutBinding::Disabled,
                assistive: ShortcutBinding::Disabled,
            },
            hold_exclusive: true,
            hold_start_delay_ms: 1234,
            double_tap_interval_ms: 260,
            deferred_insert_shortcut: DeferredInsertShortcut::CommandShiftV,
        };
        apply_hotkey_runtime_config(runtime);

        assert_eq!(get_mode_hotkey_bindings(), runtime.mode_bindings);
        assert_eq!(get_exclusive_mode(), runtime.hold_exclusive);
        assert_eq!(get_hold_start_delay_ms(), runtime.hold_start_delay_ms);
        assert_eq!(
            get_double_tap_interval_ms(),
            runtime.double_tap_interval_ms.clamp(100, 450)
        );
        assert_eq!(
            get_deferred_insert_shortcut(),
            runtime.deferred_insert_shortcut
        );
    }
}
