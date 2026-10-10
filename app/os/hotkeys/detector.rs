//! Pure gesture recognition for Codescribe hotkeys — the state machine behind
//! every trigger, with no platform API in sight.
//!
//! This module consumes already-decoded [`HotkeyDetectorInput`] values (key
//! down/up plus a modifier snapshot) and emits [`HotkeyEvent`]s. The
//! CoreGraphics event tap that produces those inputs lives in
//! [`super::platform`]. Keeping the two apart is what makes the gesture rules —
//! hold combos, double taps, arm modifiers, block diagnostics — unit-testable
//! without Accessibility permission or a live event tap; the tests at the
//! bottom of this file drive [`HotkeyDetector::feed`] with synthetic
//! `Instant`s and never touch macOS.
//!
//! The canonical routing contract for the emitted events is
//! `docs/HOTKEYS_CONTRACT.md`; this module implements the *detection* half of
//! it, while the destination of each event is decided by the controller.

use super::config::HotkeyRuntimeConfig;
use crate::config::{ChannelModifier, DeferredInsertShortcut, ShortcutBinding, WorkMode};
use std::time::{Duration, Instant};

// --- Constants ---

/// Max press duration for a "tap" gesture (milliseconds)
const TAP_MAX_MS: u64 = 220;

// --- Types ---

/// Represents the action of a hold gesture
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldAction {
    /// The hold combo just became active — start capturing.
    Down,
    /// The hold combo was released — stop capturing and deliver.
    Up,
}

/// High-level hold intent derived from modifier state.
///
/// Destination is latched at hold-down. Live detectors start a dictation hold
/// as `Raw` even when Shift/Command is already down; a later arm pulse emits
/// [`HotkeyEvent::AttachSelection`] instead of promoting to `Chat`.
///
/// - `Raw`: dictation → overlay + auto-paste (the live hold destination)
/// - `Chat` / `Selection`: leftover controller vocabulary for an assistive
///   *start* that a hold arm no longer produces. Replies still go to the
///   Agent window over `CsAgentDeliveryListener`, never the overlay.
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
    /// Front the existing Agent window without starting recording or sending.
    ShowAgent,
    /// Deliver the current in-memory deferred transcript at the active caret.
    InsertHere,
    /// Hold gesture detected (press/release configured modifier combo)
    Hold { action: HoldAction, mode: HoldMode },
    /// Modifier change while hold is active (legacy mid-hold mode flip).
    ///
    /// Destination is latched at hold-down. Live detectors emit
    /// [`HotkeyEvent::AttachSelection`] instead of upgrading Raw → Chat.
    HoldUpdate { mode: HoldMode },
    /// Rising edge of the configured arm modifier during an active hold.
    ///
    /// Captures the current OS selection as `{selection_N}` without changing
    /// destination, hiding the overlay, or fronting Agent.
    AttachSelection,
    /// Normal toggle gesture (double-tap left Option)
    ToggleNormal,
    /// Raw toggle gesture (double-tap Ctrl)
    ToggleRaw,
    /// Assistive toggle gesture (double-tap right Option)
    ToggleAssistive,
    /// Configured modifier + 0..9 toggles one agent channel. This is not a dictation take.
    ///
    /// The modifier is [`ChannelModifier`] on the hotkey runtime config (Ctrl by
    /// default, Fn optional). Command never opens a channel.
    AgentChannel { digit: u8 },
    /// A double-tap gesture was detected but could not be routed.
    DoubleTapBlocked {
        gesture: DoubleTapGesture,
        reason: DoubleTapBlockReason,
    },
}

/// Which physical double-tap the user performed.
///
/// Carried by [`HotkeyEvent::DoubleTapBlocked`] so the UI and the log line can
/// name the gesture the user actually made, independently of whichever mode it
/// failed to reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoubleTapGesture {
    LeftOption,
    RightOption,
}

impl DoubleTapGesture {
    /// Human-facing gesture name, as shown to the user in Diagnostics.
    pub fn label(self) -> &'static str {
        match self {
            Self::LeftOption => "Double-tap Left Option",
            Self::RightOption => "Double-tap Right Option",
        }
    }

    /// Stable token for log/Diagnostics lines (`blocked_double_tap gesture=…`).
    ///
    /// Kept separate from [`Self::label`] on purpose: the label is prose that
    /// may be reworded, the token is a grep-stable contract asserted by
    /// `blocked_double_tap_diagnostic_line_uses_stable_reason_tokens`.
    pub fn reason_token(self) -> &'static str {
        match self {
            Self::LeftOption => "left_option",
            Self::RightOption => "right_option",
        }
    }
}

/// Why a recognised double-tap could not be routed to a mode.
///
/// The detector still *sees* the gesture; it just has nowhere to send it. Both
/// variants are surfaced to the user rather than swallowed, so a mis-bound
/// shortcut looks like a refusal with a reason instead of a dead key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoubleTapBlockReason {
    /// No Codescribe mode is bound to this gesture in the current config.
    BindingDisabled,
    /// A hold combo or another modifier was active, so the tap was ambiguous.
    ModifierComboActive,
}

impl DoubleTapBlockReason {
    /// User-facing explanation, phrased to complete "…because …".
    pub fn message(self) -> &'static str {
        match self {
            Self::BindingDisabled => "that gesture is not assigned to a Codescribe mode",
            Self::ModifierComboActive => "another modifier or hold gesture is active",
        }
    }

    /// Stable token for log/Diagnostics lines (`blocked_double_tap reason=…`).
    pub fn reason_token(self) -> &'static str {
        match self {
            Self::BindingDisabled => "binding_disabled",
            Self::ModifierComboActive => "modifier_combo_active",
        }
    }
}

/// Stable INFO log line for a blocked double-tap (visibility only — no routing change).
pub fn blocked_double_tap_diagnostic_line(
    gesture: DoubleTapGesture,
    reason: DoubleTapBlockReason,
) -> String {
    format!(
        "blocked_double_tap gesture={} reason={}",
        gesture.reason_token(),
        reason.reason_token()
    )
}

/// Stable INFO log line when an arm attempt is ignored (visibility only).
pub fn arm_ignored_diagnostic_line(reason: &str) -> String {
    format!("arm_ignored reason={reason}")
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
    /// All modifiers released.
    pub fn new() -> Self {
        Self {
            ctrl: false,
            alt: false,
            shift: false,
            cmd: false,
        }
    }

    /// Check if the current flags match the required flags
    ///
    /// `exclusive` decides the comparison: exact equality (extra modifiers
    /// break the match) versus containment (every required modifier is held,
    /// extras tolerated). Exclusive matching is what stops `Ctrl+Shift` from
    /// silently satisfying a plain `Ctrl` binding.
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

    /// Whether these flags carry the assistive marker (Shift).
    pub fn is_assistive(&self) -> bool {
        self.shift
    }
}

impl Default for ModifierFlags {
    /// Empty modifier requirement: no ctrl/alt/shift/cmd demanded.
    fn default() -> Self {
        Self::new()
    }
}

/// Modifier state as observed at one event, straight from the platform layer.
///
/// Distinct from [`ModifierFlags`] on purpose: this is the *raw observation*
/// (including `fn_key`, which macOS reports as a secondary-Fn flag), whereas
/// `ModifierFlags` is the *requirement* a binding compares against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HotkeyModifierSnapshot {
    pub ctrl: bool,
    pub option: bool,
    pub shift: bool,
    pub cmd: bool,
    pub fn_key: bool,
}

/// The physical key an event came from, already mapped off macOS keycodes.
///
/// Left/right variants are kept apart because the whole double-tap routing
/// depends on the side: left Option and right Option drive different modes,
/// and a tap that starts on one side and ends on the other is discarded.
/// Everything the detector does not care about collapses into `Other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyPhysicalKey {
    LeftOption,
    RightOption,
    LeftControl,
    RightControl,
    Fn,
    Space,
    V,
    /// Top-row or keypad digit. `0` is the broadcast channel.
    Digit(u8),
    Other,
}

impl HotkeyPhysicalKey {
    /// Either Option key, regardless of side.
    fn is_option(self) -> bool {
        matches!(self, Self::LeftOption | Self::RightOption)
    }

    /// Right Option specifically — the side that routes to assistive mode.
    fn is_right_option(self) -> bool {
        matches!(self, Self::RightOption)
    }

    /// Either Control key, regardless of side.
    fn is_ctrl(self) -> bool {
        matches!(self, Self::LeftControl | Self::RightControl)
    }
}

/// One decoded keyboard event fed into [`HotkeyDetector::feed`].
///
/// `now` is passed in rather than read from the clock inside the detector —
/// that is what lets the tests drive double-tap windows and hold delays with
/// synthetic `Instant`s instead of sleeping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyDetectorInput {
    /// A non-modifier key went down (Space, V, or anything else).
    KeyDown {
        now: Instant,
        key: HotkeyPhysicalKey,
        modifiers: HotkeyModifierSnapshot,
    },
    /// A non-modifier key was released; clears the one-shot chord latches.
    KeyUp {
        key: HotkeyPhysicalKey,
        modifiers: HotkeyModifierSnapshot,
    },
    /// A modifier changed state — the event that carries every hold and
    /// double-tap gesture, since modifiers never produce key down/up.
    FlagsChanged {
        now: Instant,
        key: HotkeyPhysicalKey,
        modifiers: HotkeyModifierSnapshot,
    },
    /// Mouse button 2. Ignored unless `middle_mouse_acts_as_fn` is set, in
    /// which case press and release follow the Fn path.
    MiddleButton {
        now: Instant,
        pressed: bool,
        modifiers: HotkeyModifierSnapshot,
    },
}

/// The gesture state machine: fed one event at a time, emits at most one
/// [`HotkeyEvent`] per input.
///
/// All state is per-instance and time is supplied by the caller, so a detector
/// is cheap to construct in tests and carries no global or platform coupling.
/// The fields track what a single event cannot tell you: whether a hold is
/// already running, which Option side was pressed, when the last tap landed,
/// and whether a real key was struck during a modifier (which disqualifies the
/// modifier release from counting as a tap).
#[derive(Debug, Clone)]
pub struct HotkeyDetector {
    hold_active: bool,
    hold_active_ts: Option<Instant>,
    hold_mode: HoldMode,
    hold_event_sent: bool,
    last_left_tap_ts: Option<Instant>,
    last_right_tap_ts: Option<Instant>,
    last_ctrl_tap_ts: Option<Instant>,
    ctrl_down: bool,
    ctrl_down_ts: Option<Instant>,
    option_down: bool,
    option_side: Option<bool>,
    key_pressed_during_modifier: bool,
    show_agent_space_down: bool,
    insert_here_v_down: bool,
    /// Edge-trigger for `arm_ignored` diagnostics (visibility only).
    wrong_arm_logged: bool,
    /// Last sampled arm-modifier state while a hold is active, so a Shift
    /// (or Cmd) pulse can attach another `{selection_N}` without flipping mode.
    arm_modifier_down: bool,
    /// Digit currently held with the channel modifier, so key-repeat does not toggle twice.
    agent_channel_digit_down: Option<u8>,
    /// Fn (or middle-as-Fn) went down and the tap/hold choice is still open.
    fn_press_pending: bool,
    /// When the pending Fn press began.
    fn_press_started: Option<Instant>,
    /// Hold Down was emitted on release because the threshold poll had not
    /// run yet. The next [`HotkeyDetector::poll`] emits the matching Hold Up.
    hold_up_owed: bool,
}

impl Default for HotkeyDetector {
    /// Fresh detector with all hold, tap, and one-shot chord latches cleared.
    fn default() -> Self {
        Self {
            hold_active: false,
            hold_active_ts: None,
            hold_mode: HoldMode::Raw,
            hold_event_sent: false,
            last_left_tap_ts: None,
            last_right_tap_ts: None,
            last_ctrl_tap_ts: None,
            ctrl_down: false,
            ctrl_down_ts: None,
            option_down: false,
            option_side: None,
            key_pressed_during_modifier: false,
            show_agent_space_down: false,
            insert_here_v_down: false,
            wrong_arm_logged: false,
            arm_modifier_down: false,
            agent_channel_digit_down: None,
            fn_press_pending: false,
            fn_press_started: None,
            hold_up_owed: false,
        }
    }
}

/// macOS virtual keycode for a digit, top row or keypad.
///
/// The event tap already delivers every key. This only names the digits the
/// Fn channel chord cares about.
pub fn digit_from_virtual_keycode(keycode: i64) -> Option<u8> {
    match keycode {
        29 => Some(0),
        18 => Some(1),
        19 => Some(2),
        20 => Some(3),
        21 => Some(4),
        23 => Some(5),
        22 => Some(6),
        26 => Some(7),
        28 => Some(8),
        25 => Some(9),
        82 => Some(0),
        83 => Some(1),
        84 => Some(2),
        85 => Some(3),
        86 => Some(4),
        87 => Some(5),
        88 => Some(6),
        89 => Some(7),
        91 => Some(8),
        92 => Some(9),
        _ => None,
    }
}

impl HotkeyDetector {
    /// Advance the state machine by one input and return the gesture it
    /// completes, if any.
    ///
    /// `config` is passed per call rather than stored so a settings change
    /// takes effect on the very next event, without rebuilding the detector or
    /// losing an in-flight hold.
    pub fn feed(
        &mut self,
        input: HotkeyDetectorInput,
        config: HotkeyRuntimeConfig,
    ) -> Option<HotkeyEvent> {
        match input {
            HotkeyDetectorInput::KeyDown {
                now,
                key,
                modifiers,
            } => self.handle_key_down(now, key, modifiers, config),
            HotkeyDetectorInput::KeyUp { key, modifiers } => {
                if let HotkeyPhysicalKey::Digit(digit) = key
                    && self.agent_channel_digit_down == Some(digit)
                {
                    self.agent_channel_digit_down = None;
                }
                if key == HotkeyPhysicalKey::Space {
                    self.show_agent_space_down = false;
                }
                if key == HotkeyPhysicalKey::V {
                    self.insert_here_v_down = false;
                }
                if !modifiers.ctrl && !modifiers.option && !modifiers.cmd && !modifiers.fn_key {
                    self.key_pressed_during_modifier = false;
                }
                None
            }
            HotkeyDetectorInput::FlagsChanged {
                now,
                key,
                modifiers,
            } => self.handle_flags_changed(now, key, modifiers, config),
            HotkeyDetectorInput::MiddleButton {
                now,
                pressed,
                modifiers,
            } => {
                if !config.middle_mouse_acts_as_fn {
                    return None;
                }
                let mut modifiers = modifiers;
                modifiers.fn_key = pressed || modifiers.fn_key;
                self.handle_flags_changed(now, HotkeyPhysicalKey::Fn, modifiers, config)
            }
        }
    }

    /// Promote a pending Fn press once it has crossed the hold delay.
    ///
    /// The platform calls this on a short timer. A press that is still pending
    /// and younger than `hold_start_delay_ms` stays a possible tap. Crossing
    /// the delay emits the same Hold Down a Fn hold emits when tap-to-toggle
    /// is off. When a release already owed Hold Up, that event is returned
    /// instead.
    pub fn poll(&mut self, now: Instant, config: HotkeyRuntimeConfig) -> Option<HotkeyEvent> {
        if self.hold_up_owed {
            self.hold_up_owed = false;
            self.hold_active = false;
            self.hold_active_ts = None;
            self.arm_modifier_down = false;
            let mode = self.hold_mode;
            return Some(HotkeyEvent::Hold {
                action: HoldAction::Up,
                mode,
            });
        }
        if !config.fn_tap_toggles_dictation || !self.fn_press_pending {
            return None;
        }
        let started = self.fn_press_started?;
        if elapsed_between(now, started) < Duration::from_millis(config.hold_start_delay_ms) {
            return None;
        }
        self.fn_press_pending = false;
        self.fn_press_started = None;
        if config.mode_bindings.dictation != ShortcutBinding::HoldFn {
            return None;
        }
        self.hold_active = true;
        self.hold_active_ts = Some(started);
        self.hold_mode = HoldMode::Raw;
        self.hold_event_sent = true;
        self.arm_modifier_down = false;
        Some(HotkeyEvent::Hold {
            action: HoldAction::Down,
            mode: HoldMode::Raw,
        })
    }

    /// Handle a non-modifier key press.
    ///
    /// Two things happen here beyond the one-shot chords (Insert-Here and
    /// Show-Agent): a key struck inside the hold start-delay window *cancels*
    /// the pending hold (the user meant to type, not to dictate), and any key
    /// struck while a modifier is down marks that modifier as "used", which
    /// disqualifies its later release from registering as a tap.
    fn handle_key_down(
        &mut self,
        now: Instant,
        key: HotkeyPhysicalKey,
        modifiers: HotkeyModifierSnapshot,
        config: HotkeyRuntimeConfig,
    ) -> Option<HotkeyEvent> {
        if let HotkeyPhysicalKey::Digit(digit) = key
            && channel_chord_matches(config.channel_modifier, modifiers)
        {
            self.fn_press_pending = false;
            self.fn_press_started = None;
            if self.agent_channel_digit_down == Some(digit) {
                return None;
            }
            self.agent_channel_digit_down = Some(digit);
            self.key_pressed_during_modifier = true;
            return Some(HotkeyEvent::AgentChannel { digit });
        }
        if self.fn_press_pending {
            self.fn_press_pending = false;
            self.fn_press_started = None;
            self.key_pressed_during_modifier = true;
        }

        if key == HotkeyPhysicalKey::V
            && deferred_insert_modifiers_match(config.deferred_insert_shortcut, modifiers)
        {
            if self.insert_here_v_down {
                return None;
            }
            self.insert_here_v_down = true;
            return Some(HotkeyEvent::InsertHere);
        }

        if key == HotkeyPhysicalKey::Space
            && modifiers.cmd
            && modifiers.shift
            && !modifiers.ctrl
            && !modifiers.option
            && !modifiers.fn_key
        {
            if self.show_agent_space_down {
                return None;
            }
            self.show_agent_space_down = true;
            return Some(HotkeyEvent::ShowAgent);
        }

        let dictation_binding = config.mode_bindings.dictation;
        let assistive_binding = config.mode_bindings.assistive;
        let mut emitted = None;
        let base_held = hold_base_pressed(modifiers, dictation_binding)
            || assistive_hold_binding(assistive_binding)
                .is_some_and(|binding| hold_base_pressed(modifiers, binding));
        if base_held && self.hold_active {
            let in_delay_window = self
                .hold_active_ts
                .map(|ts| {
                    elapsed_between(now, ts) < Duration::from_millis(config.hold_start_delay_ms)
                })
                .unwrap_or(false);

            if in_delay_window {
                let mode = self.hold_mode;
                self.hold_active = false;
                self.hold_active_ts = None;
                self.hold_event_sent = false;
                self.arm_modifier_down = false;
                self.key_pressed_during_modifier = true;
                emitted = Some(HotkeyEvent::Hold {
                    action: HoldAction::Up,
                    mode,
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

    /// Handle a modifier state change — where every hold and double-tap is
    /// actually decided.
    ///
    /// Order matters and is deliberate: hold transitions are resolved first
    /// (so a hold always wins over a tap interpretation), then the double-tap
    /// paths run per binding. A gesture that is recognised but unroutable
    /// leaves through [`HotkeyEvent::DoubleTapBlocked`] rather than being
    /// dropped, so the user sees a reason instead of a dead key.
    fn handle_flags_changed(
        &mut self,
        now: Instant,
        key: HotkeyPhysicalKey,
        modifiers: HotkeyModifierSnapshot,
        config: HotkeyRuntimeConfig,
    ) -> Option<HotkeyEvent> {
        let dictation_binding = config.mode_bindings.dictation;
        let assistive_binding = config.mode_bindings.assistive;
        let raw_toggle_enabled = dictation_binding == ShortcutBinding::DoubleCtrl;
        let normal_toggle_enabled =
            config.mode_bindings.formatting == ShortcutBinding::DoubleLeftOption;
        let assistive_toggle_enabled =
            config.mode_bindings.assistive == ShortcutBinding::DoubleRightOption;
        let assistive_selection_combo_active = assistive_hold_binding(assistive_binding)
            .is_some_and(|binding| check_hold_combo(modifiers, binding));
        let dictation_combo_active = check_hold_combo(modifiers, dictation_binding);
        let combo_active = assistive_selection_combo_active || dictation_combo_active;
        let mode_now = if assistive_selection_combo_active {
            HoldMode::Selection
        } else {
            compute_hold_mode(
                modifiers.shift,
                modifiers.cmd,
                dictation_binding,
                config.hold_exclusive,
                config.hold_arm_modifier,
            )
        };

        // Visibility-only: rising edge when the non-configured arm modifier is
        // pressed while hold base is active (valentino-class silent arm).
        let base_for_arm = hold_base_pressed(modifiers, dictation_binding);
        let wrong_arm = match config.hold_arm_modifier {
            crate::config::HoldArmModifier::Shift => modifiers.cmd && !modifiers.shift,
            crate::config::HoldArmModifier::Cmd => modifiers.shift && !modifiers.cmd,
        };
        if base_for_arm && wrong_arm && !self.wrong_arm_logged {
            tracing::info!("{}", arm_ignored_diagnostic_line("wrong_arm_modifier"));
            self.wrong_arm_logged = true;
        } else if !wrong_arm {
            self.wrong_arm_logged = false;
        }

        let arm_now = arm_modifier_is_down(modifiers, config.hold_arm_modifier);

        let clean_fn_down = key == HotkeyPhysicalKey::Fn
            && modifiers.fn_key
            && !modifiers.ctrl
            && !modifiers.option
            && !modifiers.shift
            && !modifiers.cmd;
        if config.fn_tap_toggles_dictation
            && clean_fn_down
            && !self.hold_active
            && !self.fn_press_pending
            && !self.hold_up_owed
        {
            self.fn_press_pending = true;
            self.fn_press_started = Some(now);
        }

        let mut emitted = None;
        let still_a_clean_fn = modifiers.fn_key
            && !modifiers.ctrl
            && !modifiers.option
            && !modifiers.shift
            && !modifiers.cmd;
        let suppress_hold_down = self.fn_press_pending && still_a_clean_fn && !self.hold_active;
        if self.fn_press_pending && modifiers.fn_key && !still_a_clean_fn {
            self.fn_press_pending = false;
            self.fn_press_started = None;
        }
        if combo_active && !self.hold_active && !suppress_hold_down {
            self.hold_active = true;
            self.hold_active_ts = Some(now);
            self.hold_mode = mode_now;
            self.hold_event_sent = true;
            self.arm_modifier_down = arm_now;
            emitted = Some(HotkeyEvent::Hold {
                action: HoldAction::Down,
                mode: self.hold_mode,
            });
        } else if combo_active && self.hold_active {
            // Destination is latched at hold-down. A later Shift/Cmd pulse
            // attaches `{selection_N}`; it must not emit HoldUpdate Chat,
            // which fronts Agent and drops the live take.
            let arm_rising = arm_now && !self.arm_modifier_down;
            self.arm_modifier_down = arm_now;
            if arm_rising {
                emitted = Some(HotkeyEvent::AttachSelection);
            }
        } else if !combo_active && self.hold_active {
            self.hold_active = false;
            self.arm_modifier_down = false;
            if self.hold_event_sent {
                emitted = Some(HotkeyEvent::Hold {
                    action: HoldAction::Up,
                    mode: self.hold_mode,
                });
            }
            self.hold_active_ts = None;
        }

        if !modifiers.fn_key && self.fn_press_pending {
            let held_for = self
                .fn_press_started
                .take()
                .map(|ts| elapsed_between(now, ts))
                .unwrap_or_default();
            self.fn_press_pending = false;
            if config.fn_tap_toggles_dictation
                && held_for < Duration::from_millis(config.hold_start_delay_ms)
            {
                emitted = emitted.or(Some(HotkeyEvent::ToggleRaw));
            } else if config.mode_bindings.dictation == ShortcutBinding::HoldFn
                && held_for >= Duration::from_millis(config.hold_start_delay_ms)
            {
                self.hold_active = true;
                self.hold_active_ts = Some(now);
                self.hold_mode = HoldMode::Raw;
                self.hold_event_sent = true;
                self.hold_up_owed = true;
                emitted = emitted.or(Some(HotkeyEvent::Hold {
                    action: HoldAction::Down,
                    mode: HoldMode::Raw,
                }));
            }
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

        if !normal_toggle_enabled && !assistive_toggle_enabled {
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

            let hold_binding_blocks_toggle = match dictation_binding {
                ShortcutBinding::HoldCtrlAlt => modifiers.ctrl || self.hold_active,
                _ => modifiers.ctrl || modifiers.cmd || self.hold_active,
            };

            if self.key_pressed_during_modifier {
                self.key_pressed_during_modifier = false;
                return emitted;
            }

            let toggle_event = if hold_binding_blocks_toggle {
                register_blocked_option_double_tap(
                    self,
                    released_right,
                    now,
                    config.double_tap_interval_ms,
                    DoubleTapBlockReason::ModifierComboActive,
                )
            } else if released_right {
                self.last_left_tap_ts = None;
                if assistive_toggle_enabled {
                    register_double_tap(
                        &mut self.last_right_tap_ts,
                        now,
                        config.double_tap_interval_ms,
                        HotkeyEvent::ToggleAssistive,
                    )
                } else {
                    register_blocked_option_double_tap(
                        self,
                        released_right,
                        now,
                        config.double_tap_interval_ms,
                        DoubleTapBlockReason::BindingDisabled,
                    )
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
                register_blocked_option_double_tap(
                    self,
                    released_right,
                    now,
                    config.double_tap_interval_ms,
                    DoubleTapBlockReason::BindingDisabled,
                )
            };

            emitted = emitted.or(toggle_event);
        }

        if !modifiers.ctrl && !modifiers.option && !modifiers.cmd && !modifiers.fn_key {
            self.key_pressed_during_modifier = false;
        }

        emitted
    }
}

/// Whether the held modifiers exactly match the configured Insert-Here chord.
///
/// Matching is exclusive in both directions — every modifier the chord needs
/// must be down and every one it does not must be up — so `Cmd+Opt+V` cannot
/// be satisfied by `Cmd+Opt+Shift+V`, which belongs to the app underneath.
/// Digit opens a channel only with the configured modifier, never with Command,
/// and never with the modifier that was not selected.
pub(super) fn channel_chord_matches(
    modifier: ChannelModifier,
    modifiers: HotkeyModifierSnapshot,
) -> bool {
    if modifiers.cmd {
        return false;
    }
    match modifier {
        ChannelModifier::Ctrl => modifiers.ctrl && !modifiers.fn_key,
        ChannelModifier::Fn => modifiers.fn_key && !modifiers.ctrl,
    }
}

fn deferred_insert_modifiers_match(
    shortcut: DeferredInsertShortcut,
    modifiers: HotkeyModifierSnapshot,
) -> bool {
    match shortcut {
        DeferredInsertShortcut::Disabled => false,
        DeferredInsertShortcut::CommandOptionV => {
            modifiers.cmd
                && modifiers.option
                && !modifiers.ctrl
                && !modifiers.shift
                && !modifiers.fn_key
        }
        DeferredInsertShortcut::CommandShiftV => {
            modifiers.cmd
                && modifiers.shift
                && !modifiers.ctrl
                && !modifiers.option
                && !modifiers.fn_key
        }
        DeferredInsertShortcut::CommandControlV => {
            modifiers.cmd
                && modifiers.ctrl
                && !modifiers.option
                && !modifiers.shift
                && !modifiers.fn_key
        }
    }
}

/// Saturating `now - previous`.
///
/// Returns zero instead of panicking when the two `Instant`s arrive out of
/// order, which the tests do routinely and a monotonic-clock hiccup can do in
/// production.
fn elapsed_between(now: Instant, previous: Instant) -> Duration {
    now.checked_duration_since(previous).unwrap_or_default()
}

/// Record a tap and return `event` if it completed a double-tap.
fn register_double_tap(
    last_tap: &mut Option<Instant>,
    now: Instant,
    interval_ms: u64,
    event: HotkeyEvent,
) -> Option<HotkeyEvent> {
    if consume_double_tap(last_tap, now, interval_ms) {
        Some(event)
    } else {
        None
    }
}

/// The double-tap window itself: `true` when this tap closes a pair.
///
/// On success `last_tap` is cleared rather than replaced, so three taps read as
/// one double-tap plus a fresh first tap — never as two overlapping pairs.
fn consume_double_tap(last_tap: &mut Option<Instant>, now: Instant, interval_ms: u64) -> bool {
    if let Some(previous) = *last_tap
        && elapsed_between(now, previous) <= Duration::from_millis(interval_ms)
    {
        *last_tap = None;
        return true;
    }

    *last_tap = Some(now);
    false
}

/// Run an Option double-tap through the same window as a routable one, but emit
/// [`HotkeyEvent::DoubleTapBlocked`] instead of a mode switch.
///
/// The block still consumes the tap pair and clears the opposite side's
/// timestamp, so a blocked gesture cannot leave half-state that makes the next
/// single tap look like a double. An INFO line is logged on the same edge, once
/// per completed pair.
fn register_blocked_option_double_tap(
    detector: &mut HotkeyDetector,
    released_right: bool,
    now: Instant,
    interval_ms: u64,
    reason: DoubleTapBlockReason,
) -> Option<HotkeyEvent> {
    let (last_tap, gesture) = if released_right {
        detector.last_left_tap_ts = None;
        (
            &mut detector.last_right_tap_ts,
            DoubleTapGesture::RightOption,
        )
    } else {
        detector.last_right_tap_ts = None;
        (&mut detector.last_left_tap_ts, DoubleTapGesture::LeftOption)
    };

    if consume_double_tap(last_tap, now, interval_ms) {
        let line = blocked_double_tap_diagnostic_line(gesture, reason);
        tracing::info!("{line}");
        Some(HotkeyEvent::DoubleTapBlocked { gesture, reason })
    } else {
        None
    }
}

/// Whether the *base* modifiers of a hold binding are down, ignoring any arm
/// modifier layered on top.
///
/// Deliberately looser than [`check_hold_combo`]: this answers "is the user in
/// the neighbourhood of a hold", which is what the start-delay cancel and the
/// wrong-arm diagnostic need. Double-tap bindings have no base and return
/// `false`.
fn arm_modifier_is_down(
    modifiers: HotkeyModifierSnapshot,
    arm_modifier: crate::config::HoldArmModifier,
) -> bool {
    match arm_modifier {
        crate::config::HoldArmModifier::Shift => modifiers.shift,
        crate::config::HoldArmModifier::Cmd => modifiers.cmd,
    }
}

fn hold_base_pressed(
    modifiers: HotkeyModifierSnapshot,
    dictation_binding: ShortcutBinding,
) -> bool {
    match dictation_binding {
        ShortcutBinding::HoldFn => modifiers.fn_key,
        ShortcutBinding::HoldCtrl => modifiers.ctrl,
        ShortcutBinding::HoldCtrlAlt => modifiers.ctrl && modifiers.option,
        ShortcutBinding::HoldCtrlShift => modifiers.ctrl && modifiers.shift,
        ShortcutBinding::HoldCtrlCmd => modifiers.ctrl && modifiers.cmd,
        ShortcutBinding::Disabled
        | ShortcutBinding::DoubleCtrl
        | ShortcutBinding::DoubleLeftOption
        | ShortcutBinding::DoubleRightOption => false,
    }
}

/// Whether the hold combo is genuinely active — the strict test that actually
/// starts and stops a hold.
///
/// The Option guard up front is the difference from [`hold_base_pressed`]: for
/// every binding that does not itself involve Option, a held Option vetoes the
/// hold, so `Ctrl+Opt` cannot be mistaken for a plain `Ctrl` hold while the
/// user is reaching for an Option gesture.
fn check_hold_combo(modifiers: HotkeyModifierSnapshot, dictation_binding: ShortcutBinding) -> bool {
    if modifiers.option
        && !matches!(
            dictation_binding,
            ShortcutBinding::HoldCtrlAlt | ShortcutBinding::HoldFn
        )
    {
        return false;
    }

    match dictation_binding {
        ShortcutBinding::HoldFn => modifiers.fn_key,
        ShortcutBinding::HoldCtrl => modifiers.ctrl,
        ShortcutBinding::HoldCtrlAlt => modifiers.ctrl && modifiers.option,
        ShortcutBinding::HoldCtrlShift => modifiers.ctrl && modifiers.shift,
        ShortcutBinding::HoldCtrlCmd => modifiers.ctrl && modifiers.cmd,
        ShortcutBinding::Disabled
        | ShortcutBinding::DoubleCtrl
        | ShortcutBinding::DoubleLeftOption
        | ShortcutBinding::DoubleRightOption => false,
    }
}

/// The hold binding that arms Selection mode for a given assistive binding.
///
/// Currently returns `None` for every [`ShortcutBinding`] variant: assistive is
/// reached by double-tapping right Option, not by holding, so no binding maps
/// to an assistive *hold*. The function is kept as the single seam where a hold
/// route would be reintroduced — its two call sites in
/// [`HotkeyDetector::handle_flags_changed`] and
/// [`HotkeyDetector::handle_key_down`] already fold the `Some` case in, so the
/// wiring stays honest instead of being rediscovered later.
fn assistive_hold_binding(binding: ShortcutBinding) -> Option<ShortcutBinding> {
    match binding {
        ShortcutBinding::Disabled
        | ShortcutBinding::HoldFn
        | ShortcutBinding::HoldCtrl
        | ShortcutBinding::HoldCtrlAlt
        | ShortcutBinding::HoldCtrlShift
        | ShortcutBinding::HoldCtrlCmd
        | ShortcutBinding::DoubleCtrl
        | ShortcutBinding::DoubleLeftOption
        | ShortcutBinding::DoubleRightOption => None,
    }
}

/// Whether [`HotkeyDetector::feed`] can ever start `mode` from `binding`.
///
/// This is the routing of [`HotkeyDetector::handle_flags_changed`] written as
/// one predicate, so Settings validation and the bridge setter ask the
/// detector instead of keeping their own table:
///
/// - Dictation: every hold combo ([`check_hold_combo`]) and double-tap Ctrl.
/// - Formatting: double-tap left Option only.
/// - Assistive: double-tap right Option only; a hold reaches it only through
///   [`assistive_hold_binding`], which maps none today.
///
/// `Disabled` binds nothing and is always accepted. The reachable sets are
/// disjoint, so one gesture bound to two modes leaves exactly one of them
/// unreachable. Cross-mode precedence (double-tap Ctrl dictation silencing the
/// Option toggles) is pairwise and stays in `shortcut_registry`.
pub fn mode_binding_reachable(mode: WorkMode, binding: ShortcutBinding) -> bool {
    match binding {
        ShortcutBinding::Disabled => true,
        ShortcutBinding::HoldFn
        | ShortcutBinding::HoldCtrl
        | ShortcutBinding::HoldCtrlAlt
        | ShortcutBinding::HoldCtrlShift
        | ShortcutBinding::HoldCtrlCmd => match mode {
            WorkMode::Dictation => true,
            WorkMode::Formatting => false,
            WorkMode::Assistive => assistive_hold_binding(binding).is_some(),
        },
        ShortcutBinding::DoubleCtrl => mode == WorkMode::Dictation,
        ShortcutBinding::DoubleLeftOption => mode == WorkMode::Formatting,
        ShortcutBinding::DoubleRightOption => mode == WorkMode::Assistive,
    }
}

/// Hold destination at key-down. Arm modifiers no longer promote to `Chat`.
///
/// Shift/Command during an already-started hold attach `{selection_N}`.
/// Fn+Shift from idle stays dictation. The arguments are kept so exclusive /
/// binding / arm Settings still flow through this seam; they must not change
/// the latched destination.
fn compute_hold_mode(
    _shift: bool,
    _cmd: bool,
    _dictation_binding: ShortcutBinding,
    _hold_exclusive: bool,
    _arm_modifier: crate::config::HoldArmModifier,
) -> HoldMode {
    HoldMode::Raw
}

#[cfg(test)]
/// Table-driven and synthetic-time tests for hold, double-tap, and command chords.
mod tests {
    use super::super::config::ModeHotkeyBindings;
    use super::*;

    /// Build a HotkeyRuntimeConfig with fixed hold delay and double-tap window for tests.
    fn test_config(
        dictation: ShortcutBinding,
        formatting: ShortcutBinding,
        assistive: ShortcutBinding,
    ) -> HotkeyRuntimeConfig {
        HotkeyRuntimeConfig {
            mode_bindings: ModeHotkeyBindings {
                dictation,
                formatting,
                assistive,
            },
            hold_exclusive: false,
            hold_arm_modifier: crate::config::HoldArmModifier::Shift,
            hold_start_delay_ms: 800,
            double_tap_interval_ms: 200,
            deferred_insert_shortcut: DeferredInsertShortcut::CommandOptionV,
            channel_modifier: ChannelModifier::Ctrl,
            fn_tap_toggles_dictation: false,
            middle_mouse_acts_as_fn: false,
        }
    }

    /// Dictation on Fn hold, formatting on double-left Option, assistive on double-right.
    fn fn_hold_double_option_config() -> HotkeyRuntimeConfig {
        test_config(
            ShortcutBinding::HoldFn,
            ShortcutBinding::DoubleLeftOption,
            ShortcutBinding::DoubleRightOption,
        )
    }

    fn fn_hold_double_option_detector() -> (HotkeyDetector, HotkeyRuntimeConfig, Instant) {
        (
            HotkeyDetector::default(),
            fn_hold_double_option_config(),
            Instant::now(),
        )
    }

    /// Shorthand HotkeyModifierSnapshot constructor for compact test tables.
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
    fn fn_digit_toggles_an_agent_channel_once_per_press() {
        let mut config = fn_hold_double_option_config();
        config.channel_modifier = ChannelModifier::Fn;
        let now = Instant::now();
        let mut detector = HotkeyDetector::default();
        let down = |detector: &mut HotkeyDetector, key, modifiers| {
            detector.feed(
                HotkeyDetectorInput::KeyDown {
                    now,
                    key,
                    modifiers,
                },
                config,
            )
        };
        assert_eq!(
            down(
                &mut detector,
                HotkeyPhysicalKey::Digit(3),
                mods(false, false, false, false, true)
            ),
            Some(HotkeyEvent::AgentChannel { digit: 3 })
        );
        assert_eq!(
            down(
                &mut detector,
                HotkeyPhysicalKey::Digit(3),
                mods(false, false, false, false, true)
            ),
            None,
            "key repeat must not seal the channel"
        );
        detector.feed(
            HotkeyDetectorInput::KeyUp {
                key: HotkeyPhysicalKey::Digit(3),
                modifiers: mods(false, false, false, false, true),
            },
            config,
        );
        assert_eq!(
            down(
                &mut detector,
                HotkeyPhysicalKey::Digit(3),
                mods(false, false, false, false, true)
            ),
            Some(HotkeyEvent::AgentChannel { digit: 3 })
        );
        assert_eq!(
            down(
                &mut detector,
                HotkeyPhysicalKey::Digit(3),
                mods(false, false, false, false, false)
            ),
            None
        );
        assert_eq!(digit_from_virtual_keycode(20), Some(3));
        assert_eq!(digit_from_virtual_keycode(29), Some(0));
        assert_eq!(digit_from_virtual_keycode(85), Some(3));
        assert_eq!(digit_from_virtual_keycode(49), None);
    }

    #[test]
    /// ⌘⇧Space emits ShowAgent once per physical press; repeats and wrong mods are silent.
    fn detector_show_agent_command_table_emits_once_per_space_press() {
        let config = fn_hold_double_option_config();
        let base = Instant::now();
        let command_shift = mods(false, false, true, true, false);

        let cases = [
            (mods(false, false, false, true, false), None),
            (mods(false, false, true, false, false), None),
            (mods(true, false, true, true, false), None),
            (command_shift, Some(HotkeyEvent::ShowAgent)),
            // Auto-repeat is another key-down before key-up and must not summon twice.
            (command_shift, None),
        ];

        let mut detector = HotkeyDetector::default();
        for (index, (modifiers, expected)) in cases.into_iter().enumerate() {
            assert_eq!(
                detector.feed(
                    HotkeyDetectorInput::KeyDown {
                        now: base + Duration::from_millis(index as u64),
                        key: HotkeyPhysicalKey::Space,
                        modifiers,
                    },
                    config,
                ),
                expected,
                "unexpected command detection at table row {index}"
            );
        }

        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::KeyUp {
                    key: HotkeyPhysicalKey::Space,
                    modifiers: command_shift,
                },
                config,
            ),
            None
        );
        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::KeyDown {
                    now: base + Duration::from_millis(10),
                    key: HotkeyPhysicalKey::Space,
                    modifiers: command_shift,
                },
                config,
            ),
            Some(HotkeyEvent::ShowAgent),
            "a new physical Space press must emit exactly one new command"
        );
    }

    #[test]
    /// Configured deferred-insert chord fires InsertHere once; key-repeat is suppressed.
    fn detector_deferred_insert_command_uses_configured_chord_once_per_press() {
        let mut config = fn_hold_double_option_config();
        config.deferred_insert_shortcut = DeferredInsertShortcut::CommandShiftV;
        let mut detector = HotkeyDetector::default();
        let base = Instant::now();
        let command_shift = mods(false, false, true, true, false);

        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::KeyDown {
                    now: base,
                    key: HotkeyPhysicalKey::V,
                    modifiers: command_shift,
                },
                config,
            ),
            Some(HotkeyEvent::InsertHere)
        );
        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::KeyDown {
                    now: base + Duration::from_millis(1),
                    key: HotkeyPhysicalKey::V,
                    modifiers: command_shift,
                },
                config,
            ),
            None,
            "key repeat must not deliver twice"
        );
        detector.feed(
            HotkeyDetectorInput::KeyUp {
                key: HotkeyPhysicalKey::V,
                modifiers: command_shift,
            },
            config,
        );
        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::KeyDown {
                    now: base + Duration::from_millis(2),
                    key: HotkeyPhysicalKey::V,
                    modifiers: command_shift,
                },
                config,
            ),
            Some(HotkeyEvent::InsertHere)
        );

        config.deferred_insert_shortcut = DeferredInsertShortcut::Disabled;
        detector.feed(
            HotkeyDetectorInput::KeyUp {
                key: HotkeyPhysicalKey::V,
                modifiers: command_shift,
            },
            config,
        );
        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::KeyDown {
                    now: base + Duration::from_millis(3),
                    key: HotkeyPhysicalKey::V,
                    modifiers: command_shift,
                },
                config,
            ),
            None
        );
    }

    #[test]
    /// Arm modifiers never upgrade hold destination — attach is a later pulse.
    fn compute_hold_mode_respects_modifiers() {
        use crate::config::HoldArmModifier;
        let cases = [
            (
                false,
                false,
                ShortcutBinding::HoldFn,
                HoldArmModifier::Shift,
            ),
            (true, false, ShortcutBinding::HoldFn, HoldArmModifier::Shift),
            (false, true, ShortcutBinding::HoldFn, HoldArmModifier::Shift),
            (false, true, ShortcutBinding::HoldFn, HoldArmModifier::Cmd),
            (true, false, ShortcutBinding::HoldFn, HoldArmModifier::Cmd),
            (
                true,
                false,
                ShortcutBinding::HoldCtrl,
                HoldArmModifier::Shift,
            ),
            (false, true, ShortcutBinding::HoldCtrl, HoldArmModifier::Cmd),
            (
                true,
                false,
                ShortcutBinding::HoldCtrlAlt,
                HoldArmModifier::Shift,
            ),
            (
                false,
                true,
                ShortcutBinding::HoldCtrlAlt,
                HoldArmModifier::Shift,
            ),
            (
                false,
                false,
                ShortcutBinding::HoldCtrlAlt,
                HoldArmModifier::Shift,
            ),
            (
                true,
                false,
                ShortcutBinding::HoldCtrlShift,
                HoldArmModifier::Shift,
            ),
            (
                false,
                true,
                ShortcutBinding::HoldCtrlCmd,
                HoldArmModifier::Cmd,
            ),
        ];
        for (shift, cmd, binding, arm) in cases {
            assert_eq!(
                compute_hold_mode(shift, cmd, binding, false, arm),
                HoldMode::Raw,
                "arm must not promote {binding:?} (shift={shift}, cmd={cmd}, arm={arm:?})"
            );
        }
    }

    #[test]
    /// When hold_exclusive is set, non-raw modes collapse to Raw regardless of arm.
    fn compute_hold_mode_exclusive_forces_raw() {
        use crate::config::HoldArmModifier;
        assert_eq!(
            compute_hold_mode(
                true,
                true,
                ShortcutBinding::HoldFn,
                true,
                HoldArmModifier::Shift
            ),
            HoldMode::Raw
        );
        assert_eq!(
            compute_hold_mode(
                true,
                true,
                ShortcutBinding::HoldCtrlAlt,
                true,
                HoldArmModifier::Cmd
            ),
            HoldMode::Raw
        );
    }

    #[test]
    /// Fn hold produces one Hold(Down) after delay and matching Hold(Up) on release.
    fn detector_fn_hold_emits_down_and_up_for_one_physical_hold() {
        let (mut detector, config, base) = fn_hold_double_option_detector();

        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base,
                    key: HotkeyPhysicalKey::Fn,
                    modifiers: mods(false, false, false, false, true),
                },
                config,
            ),
            Some(HotkeyEvent::Hold {
                action: HoldAction::Down,
                mode: HoldMode::Raw,
            })
        );
        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_secs(1),
                    key: HotkeyPhysicalKey::Fn,
                    modifiers: mods(false, false, false, false, false),
                },
                config,
            ),
            Some(HotkeyEvent::Hold {
                action: HoldAction::Up,
                mode: HoldMode::Raw,
            })
        );
        assert!(!detector.hold_active);
    }

    #[test]
    /// Fn then Shift attaches selection; release stays Raw dictation.
    fn detector_fn_then_shift_attaches_selection_and_up_stays_raw() {
        let (mut detector, config, base) = fn_hold_double_option_detector();

        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base,
                    key: HotkeyPhysicalKey::Fn,
                    modifiers: mods(false, false, false, false, true),
                },
                config,
            ),
            Some(HotkeyEvent::Hold {
                action: HoldAction::Down,
                mode: HoldMode::Raw,
            })
        );
        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(10),
                    key: HotkeyPhysicalKey::Other,
                    modifiers: mods(false, false, true, false, true),
                },
                config,
            ),
            Some(HotkeyEvent::AttachSelection)
        );
        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(20),
                    key: HotkeyPhysicalKey::Fn,
                    modifiers: mods(false, false, false, false, false),
                },
                config,
            ),
            Some(HotkeyEvent::Hold {
                action: HoldAction::Up,
                mode: HoldMode::Raw,
            })
        );
    }

    #[test]
    /// Fn+Shift from idle is dictation, not Assistive / Chat.
    fn detector_fn_shift_from_idle_stays_dictation() {
        let (mut detector, config, base) = fn_hold_double_option_detector();

        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base,
                    key: HotkeyPhysicalKey::Fn,
                    modifiers: mods(false, false, true, false, true),
                },
                config,
            ),
            Some(HotkeyEvent::Hold {
                action: HoldAction::Down,
                mode: HoldMode::Raw,
            })
        );
        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(5),
                    key: HotkeyPhysicalKey::Other,
                    modifiers: mods(false, false, true, false, true),
                },
                config,
            ),
            None,
            "arm already down at start is not a rising-edge attach"
        );
        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(15),
                    key: HotkeyPhysicalKey::Fn,
                    modifiers: mods(false, false, false, false, false),
                },
                config,
            ),
            Some(HotkeyEvent::Hold {
                action: HoldAction::Up,
                mode: HoldMode::Raw,
            })
        );
    }

    #[test]
    /// Two Shift pulses during one Fn hold emit two AttachSelection events.
    fn detector_two_shift_pulses_emit_two_attach_selection() {
        let (mut detector, config, base) = fn_hold_double_option_detector();

        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base,
                    key: HotkeyPhysicalKey::Fn,
                    modifiers: mods(false, false, false, false, true),
                },
                config,
            ),
            Some(HotkeyEvent::Hold {
                action: HoldAction::Down,
                mode: HoldMode::Raw,
            })
        );
        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(10),
                    key: HotkeyPhysicalKey::Other,
                    modifiers: mods(false, false, true, false, true),
                },
                config,
            ),
            Some(HotkeyEvent::AttachSelection)
        );
        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(20),
                    key: HotkeyPhysicalKey::Other,
                    modifiers: mods(false, false, false, false, true),
                },
                config,
            ),
            None,
            "arm release is silent"
        );
        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(30),
                    key: HotkeyPhysicalKey::Other,
                    modifiers: mods(false, false, true, false, true),
                },
                config,
            ),
            Some(HotkeyEvent::AttachSelection)
        );
        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(40),
                    key: HotkeyPhysicalKey::Fn,
                    modifiers: mods(false, false, false, false, false),
                },
                config,
            ),
            Some(HotkeyEvent::Hold {
                action: HoldAction::Up,
                mode: HoldMode::Raw,
            })
        );
    }

    #[test]
    /// Exclusive match requires exact flag equality with the binding requirement.
    fn test_matches_exclusive_mode() {
        let required = ModifierFlags {
            ctrl: true,
            alt: false,
            shift: false,
            cmd: false,
        };
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
    /// Non-exclusive match allows extra modifiers beyond the required set.
    fn test_matches_non_exclusive_mode() {
        let required = ModifierFlags {
            ctrl: true,
            alt: false,
            shift: false,
            cmd: false,
        };
        let current = ModifierFlags {
            ctrl: true,
            alt: true,
            shift: false,
            cmd: false,
        };
        assert!(current.matches(&required, false));
    }

    #[test]
    /// Assistive marker is the Shift bit on ModifierFlags.
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
    /// Left Option double-tap fires only inside the configured interval window.
    fn detector_option_double_tap_window_table() {
        let table = [(200_u64, true), (201_u64, false)];

        for (gap_ms, expect_toggle) in table {
            let mut detector = HotkeyDetector::default();
            let config = fn_hold_double_option_config();
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
    /// Right Option double-tap routes to ToggleAssistive, not formatting toggle.
    fn detector_right_option_double_tap_emits_toggle_assistive() {
        let (mut detector, config, base) = fn_hold_double_option_detector();

        // First tap: press then release right Option.
        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base,
                    key: HotkeyPhysicalKey::RightOption,
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
                    key: HotkeyPhysicalKey::RightOption,
                    modifiers: mods(false, false, false, false, false),
                },
                config,
            ),
            None
        );

        // Second tap within the double-tap window: press then release again.
        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(100),
                    key: HotkeyPhysicalKey::RightOption,
                    modifiers: mods(false, true, false, false, false),
                },
                config,
            ),
            None
        );
        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(101),
                    key: HotkeyPhysicalKey::RightOption,
                    modifiers: mods(false, false, false, false, false),
                },
                config,
            ),
            Some(HotkeyEvent::ToggleAssistive)
        );
    }

    #[test]
    /// Disabled binding still surfaces DoubleTapBlocked with BindingDisabled reason.
    fn detector_reports_disabled_option_double_tap() {
        let mut detector = HotkeyDetector::default();
        let config = test_config(
            ShortcutBinding::HoldFn,
            ShortcutBinding::Disabled,
            ShortcutBinding::DoubleRightOption,
        );
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
                    now: base + Duration::from_millis(100),
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
                    now: base + Duration::from_millis(101),
                    key: HotkeyPhysicalKey::LeftOption,
                    modifiers: mods(false, false, false, false, false),
                },
                config,
            ),
            Some(HotkeyEvent::DoubleTapBlocked {
                gesture: DoubleTapGesture::LeftOption,
                reason: DoubleTapBlockReason::BindingDisabled,
            })
        );
    }

    #[test]
    /// Diagnostic lines use stable gesture= and reason= tokens for log scraping.
    fn blocked_double_tap_diagnostic_line_uses_stable_reason_tokens() {
        assert_eq!(
            blocked_double_tap_diagnostic_line(
                DoubleTapGesture::LeftOption,
                DoubleTapBlockReason::BindingDisabled,
            ),
            "blocked_double_tap gesture=left_option reason=binding_disabled"
        );
        assert_eq!(
            blocked_double_tap_diagnostic_line(
                DoubleTapGesture::RightOption,
                DoubleTapBlockReason::ModifierComboActive,
            ),
            "blocked_double_tap gesture=right_option reason=modifier_combo_active"
        );
        assert_eq!(
            arm_ignored_diagnostic_line("wrong_arm_modifier"),
            "arm_ignored reason=wrong_arm_modifier"
        );
        assert_eq!(
            DoubleTapBlockReason::BindingDisabled.reason_token(),
            "binding_disabled"
        );
        assert_eq!(
            DoubleTapBlockReason::ModifierComboActive.reason_token(),
            "modifier_combo_active"
        );
    }

    /// Capture INFO records while driving blocked left/right double-tap and
    /// wrong-arm paths. Detector is the single owner of the stable line.
    #[test]
    fn blocked_and_ignored_diagnostics_emit_exactly_one_info_each() {
        use std::io::Write;
        use std::sync::{Arc, Mutex};

        #[derive(Clone, Default)]
        /// Thread-safe in-memory writer that captures tracing_subscriber output in tests.
        struct Buf(Arc<Mutex<Vec<u8>>>);
        impl Write for Buf {
            /// Append bytes into the shared buffer; always reports full write length.
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            /// No-op flush — buffer is already durable in memory for the assertion.
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let buf = Buf::default();
        let writer = buf.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .without_time()
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            let mut detector = HotkeyDetector::default();
            let config = test_config(
                ShortcutBinding::HoldFn,
                ShortcutBinding::Disabled, // left binding disabled → blocked_double_tap
                ShortcutBinding::DoubleRightOption,
            );
            let base = Instant::now();

            // Full left Option double-tap while binding disabled → blocked INFO once.
            let _ = detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base,
                    key: HotkeyPhysicalKey::LeftOption,
                    modifiers: mods(false, true, false, false, false),
                },
                config,
            );
            let _ = detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(1),
                    key: HotkeyPhysicalKey::LeftOption,
                    modifiers: mods(false, false, false, false, false),
                },
                config,
            );
            let _ = detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(100),
                    key: HotkeyPhysicalKey::LeftOption,
                    modifiers: mods(false, true, false, false, false),
                },
                config,
            );
            let blocked = detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(101),
                    key: HotkeyPhysicalKey::LeftOption,
                    modifiers: mods(false, false, false, false, false),
                },
                config,
            );
            assert!(
                matches!(
                    blocked,
                    Some(HotkeyEvent::DoubleTapBlocked {
                        gesture: DoubleTapGesture::LeftOption,
                        reason: DoubleTapBlockReason::BindingDisabled,
                    })
                ),
                "expected blocked left double-tap, got {blocked:?}"
            );

            // Wrong arm: default arm is Shift; hold Fn + Cmd → arm_ignored INFO once.
            let hold_fn_config = fn_hold_double_option_config();
            let _ = detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(400),
                    key: HotkeyPhysicalKey::Fn,
                    modifiers: mods(false, false, false, false, true),
                },
                hold_fn_config,
            );
            let _ = detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(410),
                    key: HotkeyPhysicalKey::Other,
                    modifiers: mods(false, false, false, true, true), // cmd + fn
                },
                hold_fn_config,
            );
        });

        let captured = String::from_utf8_lossy(&buf.0.lock().unwrap()).to_string();
        let blocked_count = captured
            .matches("blocked_double_tap gesture=left_option reason=binding_disabled")
            .count();
        let arm_count = captured
            .matches("arm_ignored reason=wrong_arm_modifier")
            .count();
        assert_eq!(
            blocked_count, 1,
            "exactly one blocked_double_tap INFO expected, got {blocked_count} in:\n{captured}"
        );
        assert_eq!(
            arm_count, 1,
            "exactly one arm_ignored INFO expected, got {arm_count} in:\n{captured}"
        );
    }

    #[test]
    /// Active modifier combo blocks Option double-tap with ModifierComboActive.
    fn detector_reports_modifier_blocked_option_double_tap() {
        let (mut detector, config, base) = fn_hold_double_option_detector();

        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base,
                    key: HotkeyPhysicalKey::LeftOption,
                    modifiers: mods(false, true, false, true, false),
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
                    modifiers: mods(false, false, false, true, false),
                },
                config,
            ),
            None
        );
        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(100),
                    key: HotkeyPhysicalKey::LeftOption,
                    modifiers: mods(false, true, false, true, false),
                },
                config,
            ),
            None
        );
        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(101),
                    key: HotkeyPhysicalKey::LeftOption,
                    modifiers: mods(false, false, false, true, false),
                },
                config,
            ),
            Some(HotkeyEvent::DoubleTapBlocked {
                gesture: DoubleTapGesture::LeftOption,
                reason: DoubleTapBlockReason::ModifierComboActive,
            })
        );
    }

    #[test]
    /// A real key during hold delay cancels the pending hold before Down is emitted.
    fn detector_cancels_hold_on_keydown_during_delay() {
        let mut detector = HotkeyDetector::default();
        let mut config = test_config(
            ShortcutBinding::HoldCtrl,
            ShortcutBinding::Disabled,
            ShortcutBinding::Disabled,
        );
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
            })
        );

        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::KeyDown {
                    now: base + Duration::from_millis(200),
                    key: HotkeyPhysicalKey::Other,
                    modifiers: mods(true, false, false, false, false),
                },
                config,
            ),
            Some(HotkeyEvent::Hold {
                action: HoldAction::Up,
                mode: HoldMode::Raw,
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
        assert!(!detector.hold_active);
    }

    #[test]
    /// Ctrl+Alt hold binding requires Option present before hold tracking starts.
    fn detector_hold_ctrl_alt_requires_option_before_starting_hold() {
        let mut detector = HotkeyDetector::default();
        let config = test_config(
            ShortcutBinding::HoldCtrlAlt,
            ShortcutBinding::Disabled,
            ShortcutBinding::Disabled,
        );
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
            None
        );
        assert!(!detector.hold_active, "Ctrl alone must not arm HoldCtrlAlt");

        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(1),
                    key: HotkeyPhysicalKey::LeftOption,
                    modifiers: mods(true, true, false, false, false),
                },
                config,
            ),
            Some(HotkeyEvent::Hold {
                action: HoldAction::Down,
                mode: HoldMode::Raw,
            })
        );
        assert!(detector.hold_active);
    }

    #[test]
    /// Legacy assistive hold path still emits Down/Up for the configured binding.
    fn detector_releases_legacy_assistive_hold_binding() {
        let mut detector = HotkeyDetector::default();
        let config = test_config(
            ShortcutBinding::HoldFn,
            ShortcutBinding::DoubleLeftOption,
            ShortcutBinding::HoldCtrlCmd,
        );
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
            None
        );

        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(1),
                    key: HotkeyPhysicalKey::Other,
                    modifiers: mods(true, false, false, true, false),
                },
                config,
            ),
            None
        );

        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::FlagsChanged {
                    now: base + Duration::from_millis(2),
                    key: HotkeyPhysicalKey::Other,
                    modifiers: mods(true, false, false, false, false),
                },
                config,
            ),
            None
        );
    }

    #[test]
    /// After an Option combo with another key, double-tap state resets cleanly.
    fn detector_resets_combo_flags_after_option_combo() {
        let (mut detector, config, base) = fn_hold_double_option_detector();

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
                    key: HotkeyPhysicalKey::Other,
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
    /// Double Control toggles raw mode; combo activity prevents a false double-tap.
    fn detector_raw_toggle_double_ctrl_and_combo_reset() {
        let mut detector = HotkeyDetector::default();
        let config = test_config(
            ShortcutBinding::DoubleCtrl,
            ShortcutBinding::Disabled,
            ShortcutBinding::Disabled,
        );
        let base = Instant::now();

        let first_event = detector.feed(
            HotkeyDetectorInput::FlagsChanged {
                now: base,
                key: HotkeyPhysicalKey::LeftControl,
                modifiers: mods(true, false, false, false, false),
            },
            config,
        );
        assert_eq!(first_event, None);
        assert_eq!(
            detector.feed(
                HotkeyDetectorInput::KeyDown {
                    now: base + Duration::from_millis(10),
                    key: HotkeyPhysicalKey::Other,
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

    fn digit_down(
        detector: &mut HotkeyDetector,
        config: HotkeyRuntimeConfig,
        now: Instant,
        digit: u8,
        modifiers: HotkeyModifierSnapshot,
    ) -> Option<HotkeyEvent> {
        detector.feed(
            HotkeyDetectorInput::KeyDown {
                now,
                key: HotkeyPhysicalKey::Digit(digit),
                modifiers,
            },
            config,
        )
    }

    #[test]
    fn ctrl_digit_opens_channel_and_fn_digit_does_not_when_ctrl_selected() {
        let config = fn_hold_double_option_config();
        assert_eq!(config.channel_modifier, ChannelModifier::Ctrl);
        let now = Instant::now();
        let mut detector = HotkeyDetector::default();
        let ctrl = mods(true, false, false, false, false);
        assert_eq!(
            digit_down(&mut detector, config, now, 4, ctrl),
            Some(HotkeyEvent::AgentChannel { digit: 4 })
        );
        assert_eq!(
            digit_down(&mut detector, config, now, 4, ctrl),
            None,
            "key repeat must not open the channel twice"
        );
        detector.feed(
            HotkeyDetectorInput::KeyUp {
                key: HotkeyPhysicalKey::Digit(4),
                modifiers: ctrl,
            },
            config,
        );
        assert_eq!(
            digit_down(
                &mut detector,
                config,
                now,
                4,
                mods(false, false, false, false, true)
            ),
            None
        );
        assert_eq!(
            digit_down(
                &mut detector,
                config,
                now,
                4,
                mods(false, false, false, false, false)
            ),
            None
        );
    }

    #[test]
    fn fn_digit_opens_channel_when_fn_selected_and_ctrl_digit_does_not() {
        let mut config = fn_hold_double_option_config();
        config.channel_modifier = ChannelModifier::Fn;
        let now = Instant::now();
        let mut detector = HotkeyDetector::default();
        assert_eq!(
            digit_down(
                &mut detector,
                config,
                now,
                7,
                mods(false, false, false, false, true)
            ),
            Some(HotkeyEvent::AgentChannel { digit: 7 })
        );
        detector.feed(
            HotkeyDetectorInput::KeyUp {
                key: HotkeyPhysicalKey::Digit(7),
                modifiers: mods(false, false, false, false, true),
            },
            config,
        );
        assert_eq!(
            digit_down(
                &mut detector,
                config,
                now,
                7,
                mods(true, false, false, false, false)
            ),
            None
        );
    }

    #[test]
    fn cmd_digit_never_opens_a_channel_under_any_configuration() {
        let base = fn_hold_double_option_config();
        let now = Instant::now();
        for modifier in [ChannelModifier::Ctrl, ChannelModifier::Fn] {
            let mut config = base;
            config.channel_modifier = modifier;
            let mut detector = HotkeyDetector::default();
            for modifiers in [
                mods(false, false, false, true, false),
                mods(true, false, false, true, false),
                mods(false, false, false, true, true),
            ] {
                assert_eq!(
                    digit_down(&mut detector, config, now, 1, modifiers),
                    None,
                    "cmd must not open a channel under {modifier:?} with {modifiers:?}"
                );
            }
        }
    }

    fn fn_edge(
        detector: &mut HotkeyDetector,
        config: HotkeyRuntimeConfig,
        now: Instant,
        down: bool,
    ) -> Option<HotkeyEvent> {
        detector.feed(
            HotkeyDetectorInput::FlagsChanged {
                now,
                key: HotkeyPhysicalKey::Fn,
                modifiers: mods(false, false, false, false, down),
            },
            config,
        )
    }

    #[test]
    fn fn_tap_below_threshold_toggles_dictation_only_when_enabled() {
        let mut config = fn_hold_double_option_config();
        let base = Instant::now();
        let mut held = HotkeyDetector::default();
        assert_eq!(
            fn_edge(&mut held, config, base, true),
            Some(HotkeyEvent::Hold {
                action: HoldAction::Down,
                mode: HoldMode::Raw,
            })
        );
        assert_eq!(
            fn_edge(&mut held, config, base + Duration::from_millis(100), false),
            Some(HotkeyEvent::Hold {
                action: HoldAction::Up,
                mode: HoldMode::Raw,
            })
        );

        config.fn_tap_toggles_dictation = true;
        let mut tapped = HotkeyDetector::default();
        assert_eq!(fn_edge(&mut tapped, config, base, true), None);
        assert_eq!(
            fn_edge(
                &mut tapped,
                config,
                base + Duration::from_millis(100),
                false
            ),
            Some(HotkeyEvent::ToggleRaw)
        );
        assert_eq!(
            fn_edge(&mut tapped, config, base + Duration::from_millis(200), true),
            None
        );
        assert_eq!(
            fn_edge(
                &mut tapped,
                config,
                base + Duration::from_millis(280),
                false
            ),
            Some(HotkeyEvent::ToggleRaw)
        );
    }

    #[test]
    fn fn_hold_past_threshold_stays_hold_to_talk_with_tap_enabled() {
        let mut config = fn_hold_double_option_config();
        config.fn_tap_toggles_dictation = true;
        let base = Instant::now();
        let mut detector = HotkeyDetector::default();
        assert_eq!(fn_edge(&mut detector, config, base, true), None);
        assert_eq!(
            detector.poll(base + Duration::from_millis(800), config),
            Some(HotkeyEvent::Hold {
                action: HoldAction::Down,
                mode: HoldMode::Raw,
            })
        );
        assert_eq!(
            fn_edge(
                &mut detector,
                config,
                base + Duration::from_millis(1200),
                false
            ),
            Some(HotkeyEvent::Hold {
                action: HoldAction::Up,
                mode: HoldMode::Raw,
            })
        );
    }

    fn middle(
        detector: &mut HotkeyDetector,
        config: HotkeyRuntimeConfig,
        now: Instant,
        pressed: bool,
    ) -> Option<HotkeyEvent> {
        detector.feed(
            HotkeyDetectorInput::MiddleButton {
                now,
                pressed,
                modifiers: mods(false, false, false, false, false),
            },
            config,
        )
    }

    #[test]
    fn middle_button_press_release_mirrors_fn_hold_semantics_when_enabled() {
        let mut config = fn_hold_double_option_config();
        config.middle_mouse_acts_as_fn = true;
        let base = Instant::now();
        let mut detector = HotkeyDetector::default();
        assert_eq!(
            middle(&mut detector, config, base, true),
            Some(HotkeyEvent::Hold {
                action: HoldAction::Down,
                mode: HoldMode::Raw,
            })
        );
        assert_eq!(
            middle(&mut detector, config, base + Duration::from_secs(1), false),
            Some(HotkeyEvent::Hold {
                action: HoldAction::Up,
                mode: HoldMode::Raw,
            })
        );
        assert!(!detector.hold_active);
    }

    #[test]
    fn middle_button_is_inert_when_the_option_is_off() {
        let config = fn_hold_double_option_config();
        assert!(!config.middle_mouse_acts_as_fn);
        let base = Instant::now();
        let mut detector = HotkeyDetector::default();
        assert_eq!(middle(&mut detector, config, base, true), None);
        assert_eq!(
            middle(
                &mut detector,
                config,
                base + Duration::from_millis(40),
                false
            ),
            None
        );
        assert!(!detector.hold_active);
        assert!(!detector.fn_press_pending);
    }

    /// Perform one physical gesture as synthetic modifier snapshots and return
    /// every event the detector emitted. Holds press, wait past the hold delay
    /// and release; double-taps press and release the same key twice inside
    /// the double-tap window. No OS event is injected.
    fn perform_gesture(config: HotkeyRuntimeConfig, gesture: ShortcutBinding) -> Vec<HotkeyEvent> {
        let base = Instant::now();
        let none = mods(false, false, false, false, false);
        let ms = |offset: u64| base + Duration::from_millis(offset);
        let steps: Vec<(u64, HotkeyPhysicalKey, HotkeyModifierSnapshot)> = match gesture {
            ShortcutBinding::Disabled => Vec::new(),
            ShortcutBinding::HoldFn => vec![
                (
                    0,
                    HotkeyPhysicalKey::Fn,
                    mods(false, false, false, false, true),
                ),
                (1_000, HotkeyPhysicalKey::Fn, none),
            ],
            ShortcutBinding::HoldCtrl => vec![
                (
                    0,
                    HotkeyPhysicalKey::LeftControl,
                    mods(true, false, false, false, false),
                ),
                (1_000, HotkeyPhysicalKey::LeftControl, none),
            ],
            ShortcutBinding::HoldCtrlAlt => vec![
                (
                    0,
                    HotkeyPhysicalKey::LeftOption,
                    mods(true, true, false, false, false),
                ),
                (1_000, HotkeyPhysicalKey::LeftOption, none),
            ],
            ShortcutBinding::HoldCtrlShift => vec![
                (
                    0,
                    HotkeyPhysicalKey::Other,
                    mods(true, false, true, false, false),
                ),
                (1_000, HotkeyPhysicalKey::Other, none),
            ],
            ShortcutBinding::HoldCtrlCmd => vec![
                (
                    0,
                    HotkeyPhysicalKey::Other,
                    mods(true, false, false, true, false),
                ),
                (1_000, HotkeyPhysicalKey::Other, none),
            ],
            ShortcutBinding::DoubleCtrl
            | ShortcutBinding::DoubleLeftOption
            | ShortcutBinding::DoubleRightOption => {
                let (key, down) = match gesture {
                    ShortcutBinding::DoubleCtrl => (
                        HotkeyPhysicalKey::LeftControl,
                        mods(true, false, false, false, false),
                    ),
                    ShortcutBinding::DoubleLeftOption => (
                        HotkeyPhysicalKey::LeftOption,
                        mods(false, true, false, false, false),
                    ),
                    _ => (
                        HotkeyPhysicalKey::RightOption,
                        mods(false, true, false, false, false),
                    ),
                };
                vec![
                    (0, key, down),
                    (40, key, none),
                    (90, key, down),
                    (130, key, none),
                ]
            }
        };

        // Holding Ctrl+Command with the Shift arm would log the wrong-arm
        // diagnostic, a callsite another test captures; the arm never changes
        // where a hold routes, so pick the one this gesture already holds.
        let mut config = config;
        if gesture == ShortcutBinding::HoldCtrlCmd {
            config.hold_arm_modifier = crate::config::HoldArmModifier::Cmd;
        }
        let mut detector = HotkeyDetector::default();
        steps
            .into_iter()
            .filter_map(|(offset, key, modifiers)| {
                detector.feed(
                    HotkeyDetectorInput::FlagsChanged {
                        now: ms(offset),
                        key,
                        modifiers,
                    },
                    config,
                )
            })
            .collect()
    }

    /// The work modes a sequence of detector events actually starts.
    fn started_modes(events: &[HotkeyEvent]) -> Vec<WorkMode> {
        events
            .iter()
            .filter_map(|event| match event {
                HotkeyEvent::Hold {
                    action: HoldAction::Down,
                    mode: HoldMode::Raw,
                }
                | HotkeyEvent::ToggleRaw => Some(WorkMode::Dictation),
                HotkeyEvent::ToggleNormal => Some(WorkMode::Formatting),
                HotkeyEvent::Hold {
                    action: HoldAction::Down,
                    mode: HoldMode::Chat | HoldMode::Selection,
                }
                | HotkeyEvent::ToggleAssistive => Some(WorkMode::Assistive),
                _ => None,
            })
            .collect()
    }

    const ROUTABLE_GESTURES: [ShortcutBinding; 8] = [
        ShortcutBinding::HoldFn,
        ShortcutBinding::HoldCtrl,
        ShortcutBinding::HoldCtrlAlt,
        ShortcutBinding::HoldCtrlShift,
        ShortcutBinding::HoldCtrlCmd,
        ShortcutBinding::DoubleCtrl,
        ShortcutBinding::DoubleLeftOption,
        ShortcutBinding::DoubleRightOption,
    ];

    /// P2-007 (linked: hotkeys-dead-binding-cells): `mode_binding_reachable`
    /// is the detector's routing, cell by cell. Each mode is bound alone, the
    /// gesture is performed through `feed`, and the predicate must agree with
    /// what started. A cell the predicate accepts but the reducer ignores — or
    /// the reverse — fails here, so Settings cannot drift from routing.
    #[test]
    fn reachability_predicate_matches_detector_routing_for_every_cell() {
        let mut reachable = Vec::new();
        for mode in [
            WorkMode::Dictation,
            WorkMode::Formatting,
            WorkMode::Assistive,
        ] {
            for gesture in ROUTABLE_GESTURES {
                let bind = |slot: WorkMode| {
                    if slot == mode {
                        gesture
                    } else {
                        ShortcutBinding::Disabled
                    }
                };
                let config = test_config(
                    bind(WorkMode::Dictation),
                    bind(WorkMode::Formatting),
                    bind(WorkMode::Assistive),
                );
                let started = started_modes(&perform_gesture(config, gesture));
                let predicted = mode_binding_reachable(mode, gesture);
                assert_eq!(
                    started,
                    if predicted { vec![mode] } else { Vec::new() },
                    "{mode:?} bound to {gesture:?}: predicate says reachable={predicted}"
                );
                if predicted {
                    reachable.push((mode, gesture));
                }
            }
        }
        assert_eq!(
            reachable,
            vec![
                (WorkMode::Dictation, ShortcutBinding::HoldFn),
                (WorkMode::Dictation, ShortcutBinding::HoldCtrl),
                (WorkMode::Dictation, ShortcutBinding::HoldCtrlAlt),
                (WorkMode::Dictation, ShortcutBinding::HoldCtrlShift),
                (WorkMode::Dictation, ShortcutBinding::HoldCtrlCmd),
                (WorkMode::Dictation, ShortcutBinding::DoubleCtrl),
                (WorkMode::Formatting, ShortcutBinding::DoubleLeftOption),
                (WorkMode::Assistive, ShortcutBinding::DoubleRightOption),
            ],
            "documented matrix in docs/HOTKEYS_CONTRACT.md"
        );
        for mode in [
            WorkMode::Dictation,
            WorkMode::Formatting,
            WorkMode::Assistive,
        ] {
            assert!(mode_binding_reachable(mode, ShortcutBinding::Disabled));
        }
    }

    /// The audited reproduction: Dictation and Formatting both on double-tap
    /// left Option. The gesture starts Formatting only; Dictation is dead.
    /// The defaults route each gesture to its own mode, and double-tap Ctrl
    /// dictation silences the Option toggles (the precedence Settings reports).
    #[test]
    fn duplicate_left_option_starts_formatting_only_and_defaults_route_each_mode() {
        let duplicate = test_config(
            ShortcutBinding::DoubleLeftOption,
            ShortcutBinding::DoubleLeftOption,
            ShortcutBinding::DoubleRightOption,
        );
        assert_eq!(
            started_modes(&perform_gesture(
                duplicate,
                ShortcutBinding::DoubleLeftOption
            )),
            vec![WorkMode::Formatting]
        );
        assert!(!mode_binding_reachable(
            WorkMode::Dictation,
            ShortcutBinding::DoubleLeftOption
        ));

        let defaults = fn_hold_double_option_config();
        for (gesture, mode) in [
            (ShortcutBinding::HoldFn, WorkMode::Dictation),
            (ShortcutBinding::DoubleLeftOption, WorkMode::Formatting),
            (ShortcutBinding::DoubleRightOption, WorkMode::Assistive),
        ] {
            assert_eq!(
                started_modes(&perform_gesture(defaults, gesture)),
                vec![mode],
                "default {gesture:?}"
            );
        }

        let double_ctrl = test_config(
            ShortcutBinding::DoubleCtrl,
            ShortcutBinding::DoubleLeftOption,
            ShortcutBinding::DoubleRightOption,
        );
        assert_eq!(
            started_modes(&perform_gesture(double_ctrl, ShortcutBinding::DoubleCtrl)),
            vec![WorkMode::Dictation]
        );
        for gesture in [
            ShortcutBinding::DoubleLeftOption,
            ShortcutBinding::DoubleRightOption,
        ] {
            assert!(
                started_modes(&perform_gesture(double_ctrl, gesture)).is_empty(),
                "double-tap Ctrl dictation must silence {gesture:?}"
            );
        }
    }
}
