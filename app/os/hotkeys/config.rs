//! Process-global runtime mirror of the hotkey contract.
//!
//! Every hotkey setting is held in an atomic rather than behind a lock, so the
//! gesture detector can read the live configuration without blocking on
//! whoever is writing it. [`apply_hotkey_config`] is the single write entry
//! point from [`Config`]; the detector only ever reads.
//!
//! The mode bindings, channel modifier and Fn tap share one `AtomicU16`,
//! so their key ownership is never observed as a torn mix during reload.
//!
//! Canonical gesture semantics live in `docs/HOTKEYS_CONTRACT.md`, which the
//! defaults here mirror.

use super::detector::DoubleTapOutcome;
use crate::config::{
    ChannelModifier, Config, DeferredInsertShortcut, HoldArmModifier, ModeBinding, ShortcutBinding,
    UserSettings, WorkMode,
};
use std::sync::OnceLock;
use std::sync::atomic::{
    AtomicBool, AtomicU8, AtomicU16, AtomicU32, AtomicU64, Ordering as AtomicOrdering,
};
use std::time::{Duration, Instant};

/// Packed slot: no gesture armed for that mode.
const BIND_DISABLED: u16 = 0;
/// Packed slot: hold the Fn (globe) key.
const BIND_HOLD_FN: u16 = 1;
/// Packed slot: hold Control alone.
const BIND_HOLD_CTRL: u16 = 2;
/// Packed slot: hold Control+Option.
const BIND_HOLD_CTRL_ALT: u16 = 3;
/// Packed slot: hold Control+Shift.
const BIND_HOLD_CTRL_SHIFT: u16 = 4;
/// Packed slot: hold Control+Command.
const BIND_HOLD_CTRL_CMD: u16 = 5;
/// Packed slot: double-tap Control.
const BIND_DOUBLE_CTRL: u16 = 6;
/// Packed slot: double-tap left Option.
const BIND_DOUBLE_LEFT_OPTION: u16 = 7;
/// Packed slot: double-tap right Option.
const BIND_DOUBLE_RIGHT_OPTION: u16 = 8;

/// Boot packing: Fn hold → dictation; Formatting and the agent stay off until
/// the user opts into a gesture (the Option double-taps are opt-in).
const DEFAULT_MODE_BINDINGS_ENCODED: u16 =
    BIND_HOLD_FN | (BIND_DISABLED << 4) | (BIND_DISABLED << 8);

// --- Global Mode Binding Contract ---
// Runtime source of truth: mode -> binding mapping, not legacy hold/toggle fields.
/// Three 4-bit bindings, a 2-bit channel modifier and the Fn-tap flag.
static HOTKEY_GESTURES: AtomicU16 = AtomicU16::new(DEFAULT_MODE_BINDINGS_ENCODED);
const MODE_BINDINGS_MASK: u16 = 0x0FFF;
const CHANNEL_MODIFIER_MASK: u16 = 0x3000;
const FN_TAP_MASK: u16 = 0x4000;

/// A compare-exchange loop: `fetch_update` is deprecated on current stable and
/// its `try_update` replacement is still unstable at the 1.94 MSRV.
fn update_gesture_bits(mask: u16, value: u16) {
    let mut current = HOTKEY_GESTURES.load(AtomicOrdering::SeqCst);
    while let Err(actual) = HOTKEY_GESTURES.compare_exchange_weak(
        current,
        (current & !mask) | value,
        AtomicOrdering::SeqCst,
        AtomicOrdering::SeqCst,
    ) {
        current = actual;
    }
}

/// Pack one binding into its 4-bit slot.
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

/// Unpack a 4-bit slot value, treating anything unrecognised as
/// [`ShortcutBinding::Disabled`] so a corrupt atomic can never arm a gesture.
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

/// Pack all three mode bindings into one `u16`: dictation in bits 0-3,
/// formatting in 4-7, assistive in 8-11.
fn encode_mode_hotkey_bindings(bindings: ModeHotkeyBindings) -> u16 {
    encode_shortcut_binding(bindings.dictation)
        | (encode_shortcut_binding(bindings.formatting) << 4)
        | (encode_shortcut_binding(bindings.assistive) << 8)
}

/// Inverse of `encode_mode_hotkey_bindings`.
fn decode_mode_hotkey_bindings(raw: u16) -> ModeHotkeyBindings {
    ModeHotkeyBindings {
        dictation: decode_shortcut_binding(raw & 0x0F),
        formatting: decode_shortcut_binding((raw >> 4) & 0x0F),
        assistive: decode_shortcut_binding((raw >> 8) & 0x0F),
    }
}

/// Publish a new mode-to-gesture mapping for the gesture detector.
///
/// This mapping is the runtime source of truth for which gesture drives which
/// [`WorkMode`]; the legacy hold/toggle fields are not consulted.
pub fn set_mode_hotkey_bindings(bindings: ModeHotkeyBindings) {
    update_gesture_bits(MODE_BINDINGS_MASK, encode_mode_hotkey_bindings(bindings));
    tracing::info!(
        "Mode bindings set: dictation={:?}, formatting={:?}, assistive={:?}",
        bindings.dictation,
        bindings.formatting,
        bindings.assistive
    );
}

/// Current mode-to-gesture mapping as seen by the gesture detector.
pub fn get_mode_hotkey_bindings() -> ModeHotkeyBindings {
    decode_mode_hotkey_bindings(HOTKEY_GESTURES.load(AtomicOrdering::SeqCst))
}

// --- Global Exclusive Mode Setting ---
// Exclusive mode controls whether Shift/Cmd can act as mode modifiers for hold gestures.
// When enabled, we ignore Shift/Cmd and keep hold mode as RAW.

/// Atomic storage for exclusive mode (Shift/Cmd mode modifiers disabled)
// Default FALSE so the configured arm modifier (default Shift → Chat; Cmd alt)
// works out of the box (HOTKEYS_CONTRACT.md §"Mode modifiers", W10-B). Matches
// Config::default's `hold_exclusive: false`. Exclusive (Fn-hold is raw-only) is
// opt-in via HOLD_EXCLUSIVE=1.
static EXCLUSIVE_MODE: AtomicBool = AtomicBool::new(false);

/// Atomic storage for assistive-arm modifier: 0 = Shift (default), 1 = Cmd.
static HOLD_ARM_MODIFIER: AtomicU8 = AtomicU8::new(0);

/// Pack the assistive-arm modifier for `HOLD_ARM_MODIFIER`.
fn encode_hold_arm_modifier(arm: HoldArmModifier) -> u8 {
    match arm {
        HoldArmModifier::Shift => 0,
        HoldArmModifier::Cmd => 1,
    }
}

/// Unpack the assistive-arm modifier, defaulting to
/// [`HoldArmModifier::Shift`] for any unknown value.
fn decode_hold_arm_modifier(value: u8) -> HoldArmModifier {
    match value {
        1 => HoldArmModifier::Cmd,
        _ => HoldArmModifier::Shift,
    }
}

/// Choose which modifier arms the assistive lane during a hold gesture.
pub fn set_hold_arm_modifier(arm: HoldArmModifier) {
    HOLD_ARM_MODIFIER.store(encode_hold_arm_modifier(arm), AtomicOrdering::SeqCst);
    tracing::info!("Hold arm modifier set to: {}", arm.as_str());
}

/// Current modifier that arms the assistive lane during a hold gesture.
pub fn get_hold_arm_modifier() -> HoldArmModifier {
    decode_hold_arm_modifier(HOLD_ARM_MODIFIER.load(AtomicOrdering::SeqCst))
}

/// Set exclusive mode (thread-safe)
/// When true, Shift/Cmd modifiers are ignored for hold mode
/// When false, Shift/Cmd can act as mode modifiers (Chat/Selection)
pub fn set_exclusive_mode(enabled: bool) {
    EXCLUSIVE_MODE.store(enabled, AtomicOrdering::SeqCst);
    tracing::info!("Hotkey exclusive mode set to: {}", enabled);
}

/// Whether Shift/Cmd are ignored as mode modifiers (see [`set_exclusive_mode`]).
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

/// Packed deferred-insert chord: disabled (safe boot default).
const DEFERRED_INSERT_DISABLED: u8 = 0;
/// Packed deferred-insert chord: ⌘⌥V.
const DEFERRED_INSERT_COMMAND_OPTION_V: u8 = 1;
/// Packed deferred-insert chord: ⌘⇧V.
const DEFERRED_INSERT_COMMAND_SHIFT_V: u8 = 2;
/// Packed deferred-insert chord: ⌘⌃V.
const DEFERRED_INSERT_COMMAND_CONTROL_V: u8 = 3;

// Boot default matches `DeferredInsertShortcut::default()` (Disabled, P1-04):
// between tap start and config apply the chord must not be armed.
/// Live deferred-insert chord; stays Disabled until config apply arms a real shortcut.
static DEFERRED_INSERT_SHORTCUT: AtomicU8 = AtomicU8::new(DEFERRED_INSERT_DISABLED);

/// Pack the deferred-insert chord for `DEFERRED_INSERT_SHORTCUT`.
fn encode_deferred_insert_shortcut(shortcut: DeferredInsertShortcut) -> u8 {
    match shortcut {
        DeferredInsertShortcut::Disabled => DEFERRED_INSERT_DISABLED,
        DeferredInsertShortcut::CommandOptionV => DEFERRED_INSERT_COMMAND_OPTION_V,
        DeferredInsertShortcut::CommandShiftV => DEFERRED_INSERT_COMMAND_SHIFT_V,
        DeferredInsertShortcut::CommandControlV => DEFERRED_INSERT_COMMAND_CONTROL_V,
    }
}

/// Unpack the deferred-insert chord, defaulting to
/// [`DeferredInsertShortcut::Disabled`] for any unknown value.
fn decode_deferred_insert_shortcut(value: u8) -> DeferredInsertShortcut {
    match value {
        DEFERRED_INSERT_COMMAND_OPTION_V => DeferredInsertShortcut::CommandOptionV,
        DEFERRED_INSERT_COMMAND_SHIFT_V => DeferredInsertShortcut::CommandShiftV,
        DEFERRED_INSERT_COMMAND_CONTROL_V => DeferredInsertShortcut::CommandControlV,
        _ => DeferredInsertShortcut::Disabled,
    }
}

// Boot default matches `WidgetShortcut::default()` (⌃⇧B).
/// Live widget chord, packed by `encode_widget_shortcut`.
static WIDGET_SHORTCUT: AtomicU32 = AtomicU32::new(WIDGET_SHORTCUT_DEFAULT);
/// `ctrl | shift` modifier bits plus the letter B in the key byte.
const WIDGET_SHORTCUT_DEFAULT: u32 = (1 << 8) | (1 << 11) | b'B' as u32;

/// Pack a widget chord: modifier bits 8..=11 (ctrl, option, cmd, shift) and
/// the key in the low byte (ASCII letter, or 0 for Space).
fn encode_widget_shortcut(shortcut: crate::config::WidgetShortcut) -> u32 {
    use crate::config::WidgetShortcutKey;
    let key = match shortcut.key {
        WidgetShortcutKey::Space => 0,
        WidgetShortcutKey::Letter(letter) => letter as u32,
    };
    (shortcut.ctrl as u32) << 8
        | (shortcut.option as u32) << 9
        | (shortcut.cmd as u32) << 10
        | (shortcut.shift as u32) << 11
        | key
}

/// Unpack a widget chord; an unknown key byte falls back to the default.
fn decode_widget_shortcut(value: u32) -> crate::config::WidgetShortcut {
    use crate::config::{WidgetShortcut, WidgetShortcutKey};
    let key = match (value & 0xFF) as u8 {
        0 => WidgetShortcutKey::Space,
        letter @ b'A'..=b'Z' => WidgetShortcutKey::Letter(letter),
        _ => return WidgetShortcut::default(),
    };
    WidgetShortcut {
        ctrl: value & (1 << 8) != 0,
        option: value & (1 << 9) != 0,
        cmd: value & (1 << 10) != 0,
        shift: value & (1 << 11) != 0,
        key,
    }
}

/// Arm the chord that shows or hides the collapsed widget.
pub fn set_widget_shortcut(shortcut: crate::config::WidgetShortcut) {
    WIDGET_SHORTCUT.store(encode_widget_shortcut(shortcut), AtomicOrdering::SeqCst);
    tracing::info!(label = %shortcut.label(), "Widget shortcut set");
}

/// Currently armed widget chord.
pub fn get_widget_shortcut() -> crate::config::WidgetShortcut {
    decode_widget_shortcut(WIDGET_SHORTCUT.load(AtomicOrdering::SeqCst))
}

/// Arm (or disable) the chord that triggers deferred insert.
pub fn set_deferred_insert_shortcut(shortcut: DeferredInsertShortcut) {
    DEFERRED_INSERT_SHORTCUT.store(
        encode_deferred_insert_shortcut(shortcut),
        AtomicOrdering::SeqCst,
    );
    tracing::info!(label = shortcut.label(), "Deferred insert shortcut set");
}

/// Currently armed deferred-insert chord.
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

// --- Option tap timing ---
//
// Both are read by the detector on every Option release, so a Settings change
// applies to the next tap without restarting the event tap.

/// Bounds of the longest press that still counts as an Option tap.
pub const TAP_MAX_MS_RANGE: std::ops::RangeInclusive<u64> = 120..=300;
/// Bounds of the quiet time after typing during which Option taps are ignored.
pub const TYPING_QUIET_MS_RANGE: std::ops::RangeInclusive<u64> = 0..=1000;

/// Longest Option press (ms) that still counts as a tap.
static TAP_MAX_MS: AtomicU64 = AtomicU64::new(220);
/// Time (ms) after any typed key during which an Option tap counts for nothing.
static TYPING_QUIET_MS: AtomicU64 = AtomicU64::new(300);

/// Set the longest Option press that still counts as a tap. Clamped.
pub fn set_tap_max_ms(ms: u64) {
    let clamped = ms.clamp(*TAP_MAX_MS_RANGE.start(), *TAP_MAX_MS_RANGE.end());
    TAP_MAX_MS.store(clamped, AtomicOrdering::SeqCst);
    tracing::info!("Option tap max set to: {}ms", clamped);
}

/// Longest Option press (ms) that still counts as a tap.
pub fn get_tap_max_ms() -> u64 {
    TAP_MAX_MS.load(AtomicOrdering::SeqCst)
}

/// Set the quiet time after typing during which Option taps are ignored. Clamped.
pub fn set_typing_quiet_ms(ms: u64) {
    let clamped = ms.clamp(*TYPING_QUIET_MS_RANGE.start(), *TYPING_QUIET_MS_RANGE.end());
    TYPING_QUIET_MS.store(clamped, AtomicOrdering::SeqCst);
    tracing::info!("Option typing quiet time set to: {}ms", clamped);
}

/// Quiet time (ms) after typing during which Option taps are ignored.
pub fn get_typing_quiet_ms() -> u64 {
    TYPING_QUIET_MS.load(AtomicOrdering::SeqCst)
}

// --- Agent channel, Fn tap, middle mouse ---
//
// Channel digits default to Ctrl so a passed-through chord does not type into
// the frontmost app. Fn tap and middle-mouse-as-Fn stay off until chosen.

/// Middle mouse button follows the Fn press/release path.
static MIDDLE_MOUSE_ACTS_AS_FN: AtomicBool = AtomicBool::new(false);
/// A short middle click opens the overlay's agent chooser.
static MIDDLE_MOUSE_AGENT_GESTURE: AtomicBool = AtomicBool::new(false);

fn encode_channel_modifier(modifier: ChannelModifier) -> u8 {
    match modifier {
        ChannelModifier::Ctrl => 0,
        ChannelModifier::Fn => 1,
        ChannelModifier::Off => 2,
    }
}

fn decode_channel_modifier(value: u8) -> ChannelModifier {
    match value {
        1 => ChannelModifier::Fn,
        2 => ChannelModifier::Off,
        _ => ChannelModifier::Ctrl,
    }
}

/// Publish the modifier that opens an agent channel with a digit.
pub fn set_channel_modifier(modifier: ChannelModifier) {
    update_gesture_bits(
        CHANNEL_MODIFIER_MASK,
        u16::from(encode_channel_modifier(modifier)) << 12,
    );
    tracing::info!("Agent channel modifier set to: {}", modifier.as_str());
}

/// Modifier currently required to open an agent channel.
pub fn get_channel_modifier() -> ChannelModifier {
    decode_channel_modifier(
        ((HOTKEY_GESTURES.load(AtomicOrdering::SeqCst) & CHANNEL_MODIFIER_MASK) >> 12) as u8,
    )
}

/// Enable or disable the middle mouse button as an Fn press/release.
pub fn set_middle_mouse_acts_as_fn(enabled: bool) {
    MIDDLE_MOUSE_ACTS_AS_FN.store(enabled, AtomicOrdering::SeqCst);
    tracing::info!("Middle mouse acts as Fn: {}", enabled);
}

/// Whether mouse button 2 is fed to the detector as Fn.
pub fn get_middle_mouse_acts_as_fn() -> bool {
    MIDDLE_MOUSE_ACTS_AS_FN.load(AtomicOrdering::SeqCst)
}

/// Enable or disable the middle-click agent chooser.
pub fn set_middle_mouse_agent_gesture(enabled: bool) {
    MIDDLE_MOUSE_AGENT_GESTURE.store(enabled, AtomicOrdering::SeqCst);
    tracing::info!("Middle mouse agent gesture: {}", enabled);
}

/// Whether a short middle click opens the agent chooser.
pub fn get_middle_mouse_agent_gesture() -> bool {
    MIDDLE_MOUSE_AGENT_GESTURE.load(AtomicOrdering::SeqCst)
}

// --- Agent chooser capture ---
//
// While the overlay's agent chooser is open, the tap takes the scroll wheel,
// the left click and Escape away from other apps. The overlay renews the lease
// on every chooser input, so an overlay that stops answering cannot keep the
// mouse longer than one lease.

/// How long one renewal holds the capture.
pub const AGENT_GESTURE_CAPTURE_LEASE: Duration = Duration::from_secs(30);
/// Lease end in milliseconds since [`capture_epoch`]; 0 means released.
static AGENT_GESTURE_CAPTURE_UNTIL_MS: AtomicU64 = AtomicU64::new(0);

/// Process-local zero for the lease clock.
fn capture_epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

fn capture_millis(at: Instant) -> u64 {
    u64::try_from(at.saturating_duration_since(capture_epoch()).as_millis()).unwrap_or(u64::MAX)
}

/// Take (or renew) the chooser capture, or release it.
pub fn set_agent_gesture_capture(active: bool) {
    let until = if active {
        capture_millis(Instant::now() + AGENT_GESTURE_CAPTURE_LEASE).max(1)
    } else {
        0
    };
    AGENT_GESTURE_CAPTURE_UNTIL_MS.store(until, AtomicOrdering::SeqCst);
}

/// Whether the chooser holds the capture at `now`.
pub fn agent_gesture_capture_active(now: Instant) -> bool {
    let until = AGENT_GESTURE_CAPTURE_UNTIL_MS.load(AtomicOrdering::SeqCst);
    until != 0 && capture_millis(now) < until
}

// ── Composer caret ownership ──────────────────────────────────────────────────
//
// Codescribe's own message fields (the Agent composer and the Intercom bus
// composer) carry their own microphone. While one of them holds the focused
// caret, a long Fn is not a request for global dictation into that field: the
// recording controller refuses the hold start and the field shows where its
// microphone is. Swift is the only writer; it reports first-responder plus
// key-window truth and clears the flag when the field resigns or leaves.

static COMPOSER_CARET_OWNED: AtomicBool = AtomicBool::new(false);

/// Report whether a Codescribe composer currently owns the focused caret.
pub fn set_composer_caret_owned(owned: bool) {
    COMPOSER_CARET_OWNED.store(owned, AtomicOrdering::SeqCst);
}

/// Whether a Codescribe composer owns the focused caret right now.
pub fn composer_caret_owned() -> bool {
    COMPOSER_CARET_OWNED.load(AtomicOrdering::SeqCst)
}

// --- Gesture tester ---
//
// Settings > Shortcuts has a row where the user tries the Option double-tap.
// While its lease is held a recognised Option pair is reported but starts
// nothing; like the chooser capture, a Settings window that stops renewing
// loses the lease on its own.

/// How long one renewal of the gesture tester holds.
pub const GESTURE_TEST_LEASE: Duration = Duration::from_secs(10);
/// Tester lease end in milliseconds since [`capture_epoch`]; 0 means off.
static GESTURE_TEST_UNTIL_MS: AtomicU64 = AtomicU64::new(0);
/// Latest Option double-tap outcome: sequence in the high 24 bits, the
/// [`DoubleTapOutcome::code`] in the low 8; 0 means none yet.
static LAST_DOUBLE_TAP_OUTCOME: AtomicU32 = AtomicU32::new(0);

/// Take (or renew) the gesture tester lease, or release it.
pub fn set_gesture_test_active(active: bool) {
    let until = if active {
        capture_millis(Instant::now() + GESTURE_TEST_LEASE).max(1)
    } else {
        0
    };
    GESTURE_TEST_UNTIL_MS.store(until, AtomicOrdering::SeqCst);
}

/// Whether the gesture tester holds its lease at `now`.
pub fn gesture_test_active(now: Instant) -> bool {
    let until = GESTURE_TEST_UNTIL_MS.load(AtomicOrdering::SeqCst);
    until != 0 && capture_millis(now) < until
}

/// Publish the outcome of one Option double-tap attempt.
pub fn record_double_tap_outcome(outcome: DoubleTapOutcome) {
    let code = u32::from(outcome.code());
    let mut current = LAST_DOUBLE_TAP_OUTCOME.load(AtomicOrdering::SeqCst);
    loop {
        let sequence = ((current >> 8) + 1) & 0x00FF_FFFF;
        let next = (sequence.max(1) << 8) | code;
        match LAST_DOUBLE_TAP_OUTCOME.compare_exchange_weak(
            current,
            next,
            AtomicOrdering::SeqCst,
            AtomicOrdering::SeqCst,
        ) {
            Ok(_) => return,
            Err(actual) => current = actual,
        }
    }
}

/// The latest Option double-tap outcome and its sequence number, which
/// changes on every attempt so a repeat of the same outcome is still news.
pub fn last_double_tap_outcome() -> Option<(u32, DoubleTapOutcome)> {
    let raw = LAST_DOUBLE_TAP_OUTCOME.load(AtomicOrdering::SeqCst);
    let outcome = DoubleTapOutcome::from_code((raw & 0xFF) as u8)?;
    Some((raw >> 8, outcome))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// The whole hotkey contract as one comparable value.
///
/// Snapshotting every atomic into a single struct is what lets
/// [`apply_hotkey_runtime_config`] diff current against incoming state and
/// skip the write — and its logging — when nothing actually changed.
pub struct HotkeyRuntimeConfig {
    pub mode_bindings: ModeHotkeyBindings,
    pub hold_exclusive: bool,
    pub hold_arm_modifier: HoldArmModifier,
    pub hold_start_delay_ms: u64,
    pub double_tap_interval_ms: u64,
    /// Longest Option press that still counts as a tap.
    pub tap_max_ms: u64,
    /// After a typed key, Option taps count for nothing for this long.
    pub typing_quiet_ms: u64,
    pub deferred_insert_shortcut: DeferredInsertShortcut,
    /// Digit chord modifier. Command cannot be stored here.
    pub channel_modifier: ChannelModifier,
    /// The user's chord that shows or hides the collapsed widget.
    pub widget_shortcut: crate::config::WidgetShortcut,
    /// Quick Fn press below [`Self::hold_start_delay_ms`] toggles dictation.
    pub fn_tap_toggles_dictation: bool,
    /// Mouse button 2 is delivered as an Fn edge.
    pub middle_mouse_acts_as_fn: bool,
    /// A short middle click opens the agent chooser; with
    /// [`Self::middle_mouse_acts_as_fn`] a hold past the delay still dictates.
    pub middle_mouse_agent_gesture: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Which gesture drives each [`WorkMode`].
pub struct ModeHotkeyBindings {
    pub dictation: ShortcutBinding,
    pub formatting: ShortcutBinding,
    pub assistive: ShortcutBinding,
}

impl ModeHotkeyBindings {
    /// Read the mapping out of already-loaded user settings.
    pub fn from_settings(settings: &UserSettings) -> Self {
        Self {
            dictation: settings.mode_binding_for(WorkMode::Dictation),
            formatting: settings.mode_binding_for(WorkMode::Formatting),
            assistive: settings.mode_binding_for(WorkMode::Assistive),
        }
    }

    fn from_bindings(bindings: &[ModeBinding]) -> Self {
        let binding_for = |mode| {
            bindings
                .iter()
                .find(|entry| entry.mode == mode)
                .map_or(ShortcutBinding::Disabled, |entry| entry.binding)
        };
        Self {
            dictation: binding_for(WorkMode::Dictation),
            formatting: binding_for(WorkMode::Formatting),
            assistive: binding_for(WorkMode::Assistive),
        }
    }
}

impl From<&Config> for HotkeyRuntimeConfig {
    /// Build a runtime snapshot entirely from one loader capture.
    fn from(config: &Config) -> Self {
        Self {
            mode_bindings: ModeHotkeyBindings::from_bindings(&config.mode_bindings),
            hold_exclusive: config.hold_exclusive,
            hold_arm_modifier: config.hold_arm_modifier,
            hold_start_delay_ms: config.hold_start_delay_ms,
            double_tap_interval_ms: config.double_tap_interval_ms,
            tap_max_ms: config.tap_max_ms,
            typing_quiet_ms: config.typing_quiet_ms,
            deferred_insert_shortcut: config.deferred_insert_shortcut,
            channel_modifier: config.channel_modifier,
            widget_shortcut: config.widget_shortcut,
            fn_tap_toggles_dictation: config.fn_tap_toggles_dictation,
            middle_mouse_acts_as_fn: config.middle_mouse_acts_as_fn,
            middle_mouse_agent_gesture: config.middle_mouse_agent_gesture,
        }
    }
}

/// Snapshot every hotkey atomic into one value.
pub fn get_hotkey_runtime_config() -> HotkeyRuntimeConfig {
    let gestures = HOTKEY_GESTURES.load(AtomicOrdering::SeqCst);
    HotkeyRuntimeConfig {
        mode_bindings: decode_mode_hotkey_bindings(gestures),
        hold_exclusive: get_exclusive_mode(),
        hold_arm_modifier: get_hold_arm_modifier(),
        hold_start_delay_ms: get_hold_start_delay_ms(),
        double_tap_interval_ms: get_double_tap_interval_ms(),
        tap_max_ms: get_tap_max_ms(),
        typing_quiet_ms: get_typing_quiet_ms(),
        deferred_insert_shortcut: get_deferred_insert_shortcut(),
        channel_modifier: decode_channel_modifier(((gestures & CHANNEL_MODIFIER_MASK) >> 12) as u8),
        widget_shortcut: get_widget_shortcut(),
        fn_tap_toggles_dictation: gestures & FN_TAP_MASK != 0,
        middle_mouse_acts_as_fn: get_middle_mouse_acts_as_fn(),
        middle_mouse_agent_gesture: get_middle_mouse_agent_gesture(),
    }
}

/// Apply a full hotkey snapshot, writing only when something actually changed.
///
/// The double-tap interval is normalised before the diff because its setter
/// clamps on store — without that, an out-of-range settings value would read
/// as "changed" on every apply and flood the log.
pub fn apply_hotkey_runtime_config(config: HotkeyRuntimeConfig) {
    // Normalize before diffing: the double-tap setter clamps on store, so an
    // out-of-range settings value must not read as "changed" on every apply.
    let normalized = HotkeyRuntimeConfig {
        double_tap_interval_ms: config.double_tap_interval_ms.clamp(100, 450),
        tap_max_ms: config
            .tap_max_ms
            .clamp(*TAP_MAX_MS_RANGE.start(), *TAP_MAX_MS_RANGE.end()),
        typing_quiet_ms: config
            .typing_quiet_ms
            .clamp(*TYPING_QUIET_MS_RANGE.start(), *TYPING_QUIET_MS_RANGE.end()),
        ..config
    };
    // Every settings write funnels through this apply (update_config /
    // update_config_many), including non-hotkey keys and per-tick slider drags
    // (~60 Hz). Unchanged hotkey state must be a silent no-op — the per-setter
    // INFO logs were flooding codescribe.log with identical binding blocks.
    if get_hotkey_runtime_config() == normalized {
        return;
    }
    let gestures = encode_mode_hotkey_bindings(normalized.mode_bindings)
        | (u16::from(encode_channel_modifier(normalized.channel_modifier)) << 12)
        | if normalized.fn_tap_toggles_dictation {
            FN_TAP_MASK
        } else {
            0
        };
    HOTKEY_GESTURES.store(gestures, AtomicOrdering::SeqCst);
    tracing::info!(
        bindings = ?normalized.mode_bindings,
        channel_modifier = normalized.channel_modifier.as_str(),
        fn_tap = normalized.fn_tap_toggles_dictation,
        "Hotkey gestures set"
    );
    set_exclusive_mode(normalized.hold_exclusive);
    set_hold_arm_modifier(normalized.hold_arm_modifier);
    set_hold_start_delay_ms(normalized.hold_start_delay_ms);
    set_double_tap_interval_ms(normalized.double_tap_interval_ms);
    set_tap_max_ms(normalized.tap_max_ms);
    set_typing_quiet_ms(normalized.typing_quiet_ms);
    set_deferred_insert_shortcut(normalized.deferred_insert_shortcut);
    set_widget_shortcut(normalized.widget_shortcut);
    set_middle_mouse_acts_as_fn(normalized.middle_mouse_acts_as_fn);
    set_middle_mouse_agent_gesture(normalized.middle_mouse_agent_gesture);
}

/// Apply the hotkey slice of a [`Config`] to the runtime atomics.
pub fn apply_hotkey_config(config: &Config) {
    apply_hotkey_runtime_config(HotkeyRuntimeConfig::from(config));
}

/// Unit coverage for the process-global hotkey atomics (bindings, clamp, apply).
#[cfg(test)]
mod tests {
    use super::super::detector::DoubleTapBlockReason;
    use super::*;
    use std::sync::Mutex;

    /// Serializes tests that mutate process-global hotkey atomics.
    static HOTKEY_ATOMICS_TEST_LOCK: Mutex<()> = Mutex::new(());

    /// Round-trips a full mode-binding pack through set/get.
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

    /// Proves double-tap setter clamps to the safe [100, 450] ms window.
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

    /// The live double-tap interval atomic is what the runtime snapshot reports,
    /// so a Settings change reaches the detector's next event.
    #[test]
    #[serial_test::serial]
    fn test_double_tap_interval_is_reflected_in_runtime_snapshot() {
        let _guard = HOTKEY_ATOMICS_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let before = get_double_tap_interval_ms();
        set_double_tap_interval_ms(350);
        assert_eq!(get_hotkey_runtime_config().double_tap_interval_ms, 350);
        set_double_tap_interval_ms(200);
        assert_eq!(get_hotkey_runtime_config().double_tap_interval_ms, 200);
        set_double_tap_interval_ms(before);
    }

    /// Tap-max and typing-quiet setters clamp and reach the snapshot.
    #[test]
    #[serial_test::serial]
    fn test_tap_max_and_typing_quiet_clamp_and_reach_snapshot() {
        let _guard = HOTKEY_ATOMICS_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let (tap_before, quiet_before) = (get_tap_max_ms(), get_typing_quiet_ms());
        set_tap_max_ms(50);
        assert_eq!(get_tap_max_ms(), 120);
        set_tap_max_ms(9_999);
        assert_eq!(get_tap_max_ms(), 300);
        set_tap_max_ms(250);
        set_typing_quiet_ms(9_999);
        assert_eq!(get_typing_quiet_ms(), 1000);
        set_typing_quiet_ms(450);
        let snapshot = get_hotkey_runtime_config();
        assert_eq!(snapshot.tap_max_ms, 250);
        assert_eq!(snapshot.typing_quiet_ms, 450);
        set_tap_max_ms(tap_before);
        set_typing_quiet_ms(quiet_before);
    }

    /// The gesture tester lease is held until released, and expires on its own.
    #[test]
    #[serial_test::serial]
    fn test_gesture_test_lease_and_outcome_slot() {
        let now = Instant::now();
        set_gesture_test_active(false);
        assert!(!gesture_test_active(now));
        set_gesture_test_active(true);
        assert!(gesture_test_active(Instant::now()));
        assert!(
            !gesture_test_active(Instant::now() + Duration::from_secs(3600)),
            "the lease must lapse if the tester never releases it"
        );
        set_gesture_test_active(false);
        assert!(!gesture_test_active(Instant::now()));

        record_double_tap_outcome(DoubleTapOutcome::Recognized);
        let (first, outcome) = last_double_tap_outcome().expect("outcome recorded");
        assert_eq!(outcome, DoubleTapOutcome::Recognized);
        record_double_tap_outcome(DoubleTapOutcome::Recognized);
        let (second, _) = last_double_tap_outcome().expect("outcome recorded");
        assert_ne!(first, second, "a repeated outcome is still news");
        record_double_tap_outcome(DoubleTapOutcome::Blocked(DoubleTapBlockReason::Mouse));
        assert_eq!(
            last_double_tap_outcome().map(|(_, o)| o),
            Some(DoubleTapOutcome::Blocked(DoubleTapBlockReason::Mouse))
        );
    }

    /// Apply writes every hotkey atomic when the snapshot differs from live state.
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
            hold_arm_modifier: HoldArmModifier::Cmd,
            hold_start_delay_ms: 1234,
            double_tap_interval_ms: 260,
            tap_max_ms: 220,
            typing_quiet_ms: 300,
            deferred_insert_shortcut: DeferredInsertShortcut::CommandShiftV,
            channel_modifier: ChannelModifier::Fn,
            widget_shortcut: crate::config::WidgetShortcut::default(),
            fn_tap_toggles_dictation: true,
            middle_mouse_acts_as_fn: true,
            middle_mouse_agent_gesture: true,
        };
        apply_hotkey_runtime_config(runtime);

        assert_eq!(get_mode_hotkey_bindings(), runtime.mode_bindings);
        assert_eq!(get_exclusive_mode(), runtime.hold_exclusive);
        assert_eq!(get_hold_arm_modifier(), HoldArmModifier::Cmd);
        assert_eq!(get_hold_start_delay_ms(), runtime.hold_start_delay_ms);
        assert_eq!(
            get_double_tap_interval_ms(),
            runtime.double_tap_interval_ms.clamp(100, 450)
        );
        assert_eq!(
            get_deferred_insert_shortcut(),
            runtime.deferred_insert_shortcut
        );
        assert_eq!(get_channel_modifier(), ChannelModifier::Fn);
        assert!(get_hotkey_runtime_config().fn_tap_toggles_dictation);
        assert!(get_middle_mouse_acts_as_fn());
        assert!(get_middle_mouse_agent_gesture());
    }

    /// Second apply of an out-of-range interval is a no-op after the first clamp.
    #[test]
    fn test_apply_hotkey_runtime_config_is_idempotent_after_clamp() {
        let _guard = HOTKEY_ATOMICS_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        // Out-of-range double-tap: first apply clamps on store; a second apply
        // of the same raw config must be a no-op diff, not a perpetual "changed".
        let runtime = HotkeyRuntimeConfig {
            mode_bindings: ModeHotkeyBindings {
                dictation: ShortcutBinding::HoldFn,
                formatting: ShortcutBinding::DoubleLeftOption,
                assistive: ShortcutBinding::DoubleRightOption,
            },
            hold_exclusive: false,
            hold_arm_modifier: HoldArmModifier::Shift,
            hold_start_delay_ms: 800,
            double_tap_interval_ms: 999,
            tap_max_ms: 220,
            typing_quiet_ms: 300,
            deferred_insert_shortcut: DeferredInsertShortcut::CommandOptionV,
            channel_modifier: ChannelModifier::Ctrl,
            widget_shortcut: crate::config::WidgetShortcut::default(),
            fn_tap_toggles_dictation: false,
            middle_mouse_acts_as_fn: false,
            middle_mouse_agent_gesture: false,
        };
        apply_hotkey_runtime_config(runtime);
        let after_first = get_hotkey_runtime_config();
        assert_eq!(after_first.double_tap_interval_ms, 450);

        apply_hotkey_runtime_config(runtime);
        assert_eq!(get_hotkey_runtime_config(), after_first);
    }

    #[test]
    fn runtime_uses_captured_bindings() {
        let config = Config {
            mode_bindings: vec![ModeBinding {
                mode: WorkMode::Dictation,
                binding: ShortcutBinding::HoldCtrl,
            }],
            channel_modifier: ChannelModifier::Fn,
            ..Config::default()
        };
        let runtime = HotkeyRuntimeConfig::from(&config);
        assert_eq!(runtime.mode_bindings.dictation, ShortcutBinding::HoldCtrl);
        assert_eq!(runtime.mode_bindings.formatting, ShortcutBinding::Disabled);
        assert_eq!(runtime.mode_bindings.assistive, ShortcutBinding::Disabled);
        assert_eq!(runtime.channel_modifier, ChannelModifier::Fn);
    }

    #[test]
    #[serial_test::serial]
    fn live_reload_never_tears_key_ownership() {
        let _guard = HOTKEY_ATOMICS_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let previous = get_hotkey_runtime_config();
        let fn_recording = HotkeyRuntimeConfig {
            mode_bindings: ModeHotkeyBindings {
                dictation: ShortcutBinding::HoldFn,
                formatting: ShortcutBinding::Disabled,
                assistive: ShortcutBinding::Disabled,
            },
            channel_modifier: ChannelModifier::Ctrl,
            fn_tap_toggles_dictation: true,
            ..previous
        };
        let ctrl_recording = HotkeyRuntimeConfig {
            mode_bindings: ModeHotkeyBindings {
                dictation: ShortcutBinding::HoldCtrl,
                ..fn_recording.mode_bindings
            },
            channel_modifier: ChannelModifier::Fn,
            fn_tap_toggles_dictation: false,
            ..fn_recording
        };
        apply_hotkey_runtime_config(fn_recording);
        std::thread::scope(|scope| {
            let writer = scope.spawn(|| {
                for _ in 0..10_000 {
                    apply_hotkey_runtime_config(ctrl_recording);
                    apply_hotkey_runtime_config(fn_recording);
                }
            });
            for _ in 0..20_000 {
                let snapshot = get_hotkey_runtime_config();
                assert!(snapshot == fn_recording || snapshot == ctrl_recording);
            }
            writer.join().unwrap();
        });
        apply_hotkey_runtime_config(previous);
    }

    /// `ctrl` and `fn` parse; `cmd` is rejected. Defaults stay ctrl, tap off, middle off.
    #[test]
    fn channel_modifier_parse_and_input_surface_defaults() {
        assert_eq!("ctrl".parse::<ChannelModifier>(), Ok(ChannelModifier::Ctrl));
        assert_eq!("fn".parse::<ChannelModifier>(), Ok(ChannelModifier::Fn));
        assert!("cmd".parse::<ChannelModifier>().is_err());
        assert!("command".parse::<ChannelModifier>().is_err());
        assert_eq!(ChannelModifier::default(), ChannelModifier::Ctrl);

        let config = Config::default();
        assert_eq!(config.channel_modifier, ChannelModifier::Ctrl);
        assert!(!config.fn_tap_toggles_dictation);
        assert!(!config.middle_mouse_acts_as_fn);
        assert!(!config.middle_mouse_agent_gesture);

        let runtime = HotkeyRuntimeConfig {
            mode_bindings: ModeHotkeyBindings {
                dictation: ShortcutBinding::HoldFn,
                formatting: ShortcutBinding::DoubleLeftOption,
                assistive: ShortcutBinding::DoubleRightOption,
            },
            hold_exclusive: false,
            hold_arm_modifier: HoldArmModifier::Shift,
            hold_start_delay_ms: 800,
            double_tap_interval_ms: 200,
            tap_max_ms: 220,
            typing_quiet_ms: 300,
            deferred_insert_shortcut: DeferredInsertShortcut::default(),
            channel_modifier: config.channel_modifier,
            widget_shortcut: crate::config::WidgetShortcut::default(),
            fn_tap_toggles_dictation: config.fn_tap_toggles_dictation,
            middle_mouse_acts_as_fn: config.middle_mouse_acts_as_fn,
            middle_mouse_agent_gesture: config.middle_mouse_agent_gesture,
        };
        assert_eq!(runtime.channel_modifier, ChannelModifier::Ctrl);
        assert!(!runtime.fn_tap_toggles_dictation);
        assert!(!runtime.middle_mouse_acts_as_fn);
        assert!(!runtime.middle_mouse_agent_gesture);
    }

    /// The chooser capture holds for one lease, and a release ends it at once.
    #[test]
    fn agent_gesture_capture_is_a_lease_and_releases_at_once() {
        let _guard = HOTKEY_ATOMICS_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        set_agent_gesture_capture(true);
        let now = Instant::now();
        assert!(agent_gesture_capture_active(now));
        assert!(!agent_gesture_capture_active(
            now + AGENT_GESTURE_CAPTURE_LEASE + Duration::from_millis(5)
        ));

        set_agent_gesture_capture(false);
        assert!(!agent_gesture_capture_active(Instant::now()));
    }

    /// Composer caret ownership is a plain fact the UI sets and clears.
    #[test]
    fn composer_caret_ownership_follows_the_last_report() {
        let _guard = HOTKEY_ATOMICS_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        set_composer_caret_owned(true);
        assert!(composer_caret_owned());
        set_composer_caret_owned(false);
        assert!(!composer_caret_owned());
    }
}
