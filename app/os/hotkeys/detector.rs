use super::config::HotkeyRuntimeConfig;
use crate::config::ShortcutBinding;
use std::time::{Duration, Instant};

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
    /// Front the existing Agent window without starting recording or sending.
    ShowAgent,
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
    /// A double-tap gesture was detected but could not be routed.
    DoubleTapBlocked {
        gesture: DoubleTapGesture,
        reason: DoubleTapBlockReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoubleTapGesture {
    LeftOption,
    RightOption,
}

impl DoubleTapGesture {
    pub fn label(self) -> &'static str {
        match self {
            Self::LeftOption => "Double-tap Left Option",
            Self::RightOption => "Double-tap Right Option",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoubleTapBlockReason {
    BindingDisabled,
    ModifierComboActive,
}

impl DoubleTapBlockReason {
    pub fn message(self) -> &'static str {
        match self {
            Self::BindingDisabled => "that gesture is not assigned to a Codescribe mode",
            Self::ModifierComboActive => "another modifier or hold gesture is active",
        }
    }
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
    Space,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyDetectorInput {
    KeyDown {
        now: Instant,
        key: HotkeyPhysicalKey,
        modifiers: HotkeyModifierSnapshot,
    },
    KeyUp {
        key: HotkeyPhysicalKey,
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
    show_agent_space_down: bool,
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
            show_agent_space_down: false,
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
            HotkeyDetectorInput::KeyDown {
                now,
                key,
                modifiers,
            } => self.handle_key_down(now, key, modifiers, config),
            HotkeyDetectorInput::KeyUp { key, modifiers } => {
                if key == HotkeyPhysicalKey::Space {
                    self.show_agent_space_down = false;
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
        }
    }

    pub fn is_combo_active(&self) -> bool {
        self.hold_active
    }

    fn handle_key_down(
        &mut self,
        now: Instant,
        key: HotkeyPhysicalKey,
        modifiers: HotkeyModifierSnapshot,
        config: HotkeyRuntimeConfig,
    ) -> Option<HotkeyEvent> {
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
            )
        };
        let force_ai_now = if assistive_selection_combo_active {
            false
        } else {
            compute_hold_force_ai(
                modifiers.option,
                modifiers.shift,
                modifiers.cmd,
                dictation_binding,
            )
        };

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

fn elapsed_between(now: Instant, previous: Instant) -> Duration {
    now.checked_duration_since(previous).unwrap_or_default()
}

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

    consume_double_tap(last_tap, now, interval_ms)
        .then_some(HotkeyEvent::DoubleTapBlocked { gesture, reason })
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

fn assistive_hold_binding(binding: ShortcutBinding) -> Option<ShortcutBinding> {
    match binding {
        ShortcutBinding::HoldFn
        | ShortcutBinding::HoldCtrl
        | ShortcutBinding::HoldCtrlAlt
        | ShortcutBinding::HoldCtrlShift
        | ShortcutBinding::HoldCtrlCmd => Some(binding),
        ShortcutBinding::Disabled
        | ShortcutBinding::DoubleCtrl
        | ShortcutBinding::DoubleLeftOption
        | ShortcutBinding::DoubleRightOption => None,
    }
}

fn compute_hold_mode(
    shift: bool,
    cmd: bool,
    dictation_binding: ShortcutBinding,
    hold_exclusive: bool,
) -> HoldMode {
    if hold_exclusive {
        return HoldMode::Raw;
    }

    match dictation_binding {
        ShortcutBinding::Disabled
        | ShortcutBinding::HoldCtrl
        | ShortcutBinding::HoldCtrlShift
        | ShortcutBinding::HoldCtrlCmd
        | ShortcutBinding::DoubleCtrl
        | ShortcutBinding::DoubleLeftOption
        | ShortcutBinding::DoubleRightOption => HoldMode::Raw,
        ShortcutBinding::HoldCtrlAlt => {
            if cmd {
                HoldMode::Selection
            } else if shift {
                HoldMode::Chat
            } else {
                HoldMode::Raw
            }
        }
        ShortcutBinding::HoldFn => {
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

fn compute_hold_force_ai(
    option: bool,
    shift: bool,
    cmd: bool,
    dictation_binding: ShortcutBinding,
) -> bool {
    match dictation_binding {
        ShortcutBinding::HoldCtrlAlt => option && !shift && !cmd,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::super::config::ModeHotkeyBindings;
    use super::*;

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
    fn detector_show_agent_command_table_emits_once_per_space_press() {
        let config = test_config(
            ShortcutBinding::HoldFn,
            ShortcutBinding::DoubleLeftOption,
            ShortcutBinding::DoubleRightOption,
        );
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
    fn compute_hold_mode_respects_modifiers() {
        // Fn base with Shift/Cmd modifiers
        assert_eq!(
            compute_hold_mode(false, false, ShortcutBinding::HoldFn, false),
            HoldMode::Raw
        );
        assert_eq!(
            compute_hold_mode(true, false, ShortcutBinding::HoldFn, false),
            HoldMode::Chat
        );
        assert_eq!(
            compute_hold_mode(false, true, ShortcutBinding::HoldFn, false),
            HoldMode::Selection
        );

        // Ctrl-only ignores Shift/Cmd modifiers
        assert_eq!(
            compute_hold_mode(true, false, ShortcutBinding::HoldCtrl, false),
            HoldMode::Raw
        );
        assert_eq!(
            compute_hold_mode(false, true, ShortcutBinding::HoldCtrl, false),
            HoldMode::Raw
        );

        // Ctrl+Option allows modifiers
        assert_eq!(
            compute_hold_mode(true, false, ShortcutBinding::HoldCtrlAlt, false),
            HoldMode::Chat
        );
        assert_eq!(
            compute_hold_mode(false, true, ShortcutBinding::HoldCtrlAlt, false),
            HoldMode::Selection
        );
        assert_eq!(
            compute_hold_mode(false, false, ShortcutBinding::HoldCtrlAlt, false),
            HoldMode::Raw
        );

        // Ctrl+Shift/Cmd are fixed to raw
        assert_eq!(
            compute_hold_mode(true, false, ShortcutBinding::HoldCtrlShift, false),
            HoldMode::Raw
        );
        assert_eq!(
            compute_hold_mode(false, true, ShortcutBinding::HoldCtrlCmd, false),
            HoldMode::Raw
        );
    }

    #[test]
    fn compute_hold_mode_exclusive_forces_raw() {
        assert_eq!(
            compute_hold_mode(true, true, ShortcutBinding::HoldFn, true),
            HoldMode::Raw
        );
        assert_eq!(
            compute_hold_mode(true, true, ShortcutBinding::HoldCtrlAlt, true),
            HoldMode::Raw
        );
    }

    #[test]
    fn detector_fn_hold_emits_down_and_up_for_one_physical_hold() {
        let mut detector = HotkeyDetector::default();
        let config = test_config(
            ShortcutBinding::HoldFn,
            ShortcutBinding::DoubleLeftOption,
            ShortcutBinding::DoubleRightOption,
        );
        let base = Instant::now();

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
                force_ai: false,
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
                force_ai: false,
            })
        );
        assert!(!detector.is_combo_active());
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
    fn detector_option_double_tap_window_table() {
        let table = [(200_u64, true), (201_u64, false)];

        for (gap_ms, expect_toggle) in table {
            let mut detector = HotkeyDetector::default();
            let config = test_config(
                ShortcutBinding::HoldFn,
                ShortcutBinding::DoubleLeftOption,
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
    fn detector_reports_modifier_blocked_option_double_tap() {
        let mut detector = HotkeyDetector::default();
        let config = test_config(
            ShortcutBinding::HoldFn,
            ShortcutBinding::DoubleLeftOption,
            ShortcutBinding::DoubleRightOption,
        );
        let base = Instant::now();

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
                force_ai: false,
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
        assert!(
            !detector.is_combo_active(),
            "Ctrl alone must not arm HoldCtrlAlt"
        );

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
                force_ai: true,
            })
        );
        assert!(detector.is_combo_active());
    }

    #[test]
    fn detector_routes_assistive_hold_binding_to_selection_mode() {
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
            Some(HotkeyEvent::Hold {
                action: HoldAction::Down,
                mode: HoldMode::Selection,
                force_ai: false,
            })
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
            Some(HotkeyEvent::Hold {
                action: HoldAction::Up,
                mode: HoldMode::Selection,
                force_ai: false,
            })
        );
    }

    #[test]
    fn detector_resets_combo_flags_after_option_combo() {
        let mut detector = HotkeyDetector::default();
        let config = test_config(
            ShortcutBinding::HoldFn,
            ShortcutBinding::DoubleLeftOption,
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
}
