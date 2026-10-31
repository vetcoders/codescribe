//! macOS keyboard shortcut registry conflict checks for Codescribe hotkeys.
//!
//! Reads the system SymbolicHotkeys registry and reports potential collisions
//! with our modifier-only gestures (Fn/Ctrl/Option).

use crate::config::{DeferredInsertShortcut, ShortcutBinding, UserSettings, WorkMode};
use crate::os::hotkeys::{ModeHotkeyBindings, mode_binding_reachable};
#[cfg(target_os = "macos")]
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// A modifier-only gesture Codescribe listens for, in the vocabulary used when
/// reporting conflicts.
pub enum HotkeyGesture {
    HoldFn,
    HoldCtrl,
    HoldCtrlAlt,
    HoldCtrlShift,
    HoldCtrlCmd,
    ToggleDoubleCtrl,
    ToggleDoubleLeftOption,
    ToggleDoubleRightOption,
}

impl HotkeyGesture {
    /// Human-readable gesture name for Settings and conflict messages.
    pub fn label(self) -> &'static str {
        match self {
            Self::HoldFn => "Hold Fn/Globe",
            Self::HoldCtrl => "Hold Ctrl",
            Self::HoldCtrlAlt => "Hold Ctrl+Option",
            Self::HoldCtrlShift => "Hold Ctrl+Shift",
            Self::HoldCtrlCmd => "Hold Ctrl+Command",
            Self::ToggleDoubleCtrl => "Double-tap Ctrl",
            Self::ToggleDoubleLeftOption => "Double-tap Left Option",
            Self::ToggleDoubleRightOption => "Double-tap Right Option",
        }
    }

    /// The gesture a binding performs; `Disabled` performs none.
    fn from_binding(binding: ShortcutBinding) -> Option<Self> {
        match binding {
            ShortcutBinding::Disabled => None,
            ShortcutBinding::HoldFn => Some(Self::HoldFn),
            ShortcutBinding::HoldCtrl => Some(Self::HoldCtrl),
            ShortcutBinding::HoldCtrlAlt => Some(Self::HoldCtrlAlt),
            ShortcutBinding::HoldCtrlShift => Some(Self::HoldCtrlShift),
            ShortcutBinding::HoldCtrlCmd => Some(Self::HoldCtrlCmd),
            ShortcutBinding::DoubleCtrl => Some(Self::ToggleDoubleCtrl),
            ShortcutBinding::DoubleLeftOption => Some(Self::ToggleDoubleLeftOption),
            ShortcutBinding::DoubleRightOption => Some(Self::ToggleDoubleRightOption),
        }
    }
}

/// Why `binding` can never start `mode`, or `None` when the detector routes it.
///
/// Reachability is the detector's ([`mode_binding_reachable`]). Holds start
/// only Dictation; the Option double-taps start only Formatting or the Agent;
/// double-tap Ctrl starts any mode. The sentences are wire values the Settings
/// screen localizes; the bridge setter returns the same text when it refuses
/// the write.
pub fn unreachable_binding_message(
    mode: WorkMode,
    binding: ShortcutBinding,
) -> Option<&'static str> {
    if mode_binding_reachable(mode, binding) {
        return None;
    }
    Some(match mode {
        WorkMode::Dictation => {
            "This gesture only starts Formatting or Assistive, so Dictation would never start from it."
        }
        WorkMode::Formatting => {
            "This gesture only starts Dictation, so Formatting would never start from it."
        }
        WorkMode::Assistive => {
            "This gesture only starts Dictation, so Assistive would never start from it."
        }
    })
}

/// One gesture keeps one mode: the sentence for a mode bound to a gesture an
/// earlier mode (in Settings order) already uses. A wire value, like
/// [`unreachable_binding_message`].
pub fn duplicate_binding_message(owner: WorkMode) -> &'static str {
    match owner {
        WorkMode::Dictation => {
            "This gesture already starts Dictation; one gesture starts one mode."
        }
        WorkMode::Formatting => {
            "This gesture already starts Formatting; one gesture starts one mode."
        }
        WorkMode::Assistive => {
            "This gesture already starts Assistive; one gesture starts one mode."
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// One reason a configured gesture may not reach Codescribe.
pub struct HotkeyConflict {
    pub gesture: HotkeyGesture,
    pub message: String,
}

/// Every conflict for the current bindings: Codescribe's own collisions first,
/// then collisions with enabled macOS symbolic hotkeys.
pub fn detect_hotkey_conflicts(settings: &UserSettings) -> Vec<HotkeyConflict> {
    let bindings = ModeHotkeyBindings::from_settings(settings);
    let mut conflicts = detect_internal_conflicts(bindings);
    conflicts.extend(detect_macos_symbolic_conflicts(bindings));
    conflicts
}

/// Return the first enabled macOS symbolic shortcut colliding with the
/// configured deferred-insert command. The shared CGEventTap remains alive for
/// recording gestures; only this one-shot command is considered unavailable.
#[cfg(target_os = "macos")]
pub fn deferred_insert_shortcut_conflict(shortcut: DeferredInsertShortcut) -> Option<String> {
    if !shortcut.is_enabled() {
        return Some("Deferred insert shortcut is disabled".to_string());
    }

    load_symbolic_signatures()
        .into_iter()
        .find(|signature| deferred_insert_conflicts_with_symbolic(shortcut, *signature))
        .map(|signature| {
            format!(
                "{} conflicts with {} (macOS #{}).",
                shortcut.label(),
                symbolic_hotkey_name(signature.id),
                signature.id
            )
        })
}

/// Non-macOS stub: with no symbolic-hotkey registry to consult, only a
/// disabled shortcut counts as unavailable.
#[cfg(not(target_os = "macos"))]
pub fn deferred_insert_shortcut_conflict(shortcut: DeferredInsertShortcut) -> Option<String> {
    (!shortcut.is_enabled()).then(|| "Deferred insert shortcut is disabled".to_string())
}

/// Informational note for the Fn/Globe case.
///
/// When dictation is bound to Hold Fn *and* macOS still owns the Fn tap, the
/// hold gesture can swallow that tap. This is deliberately not a
/// [`HotkeyConflict`]: reporting it as one would block Save in Settings.
pub fn fn_tap_intercept_note(settings: &UserSettings) -> Option<&'static str> {
    let bindings = ModeHotkeyBindings::from_settings(settings);
    if bindings.dictation != ShortcutBinding::HoldFn {
        return None;
    }

    fn_tap_symbols_enabled().then_some(
        "Fn/Globe tap is configured by macOS. Codescribe Hold Fn may intercept that tap while dictation is active; this is informational, not a shortcut conflict.",
    )
}

/// Whether macOS still has a Fn/Globe tap action enabled — symbolic IDs 160
/// (Emoji & Symbols) or 164 (Start Dictation).
#[cfg(target_os = "macos")]
fn fn_tap_symbols_enabled() -> bool {
    load_symbolic_signatures()
        .iter()
        .any(|signature| matches!(signature.id, 160 | 164))
}

/// Non-macOS stub: there is no Fn/Globe tap action to compete with.
#[cfg(not(target_os = "macos"))]
fn fn_tap_symbols_enabled() -> bool {
    false
}

/// Gestures currently reachable under `bindings`, across all three modes.
///
/// Each mode contributes the gesture it is bound to when the detector routes
/// that gesture to it; a gesture shared by two modes is listed once.
fn active_gestures(bindings: ModeHotkeyBindings) -> Vec<HotkeyGesture> {
    let mut gestures = Vec::new();
    for (mode, binding) in mode_rows(bindings) {
        if mode_binding_reachable(mode, binding)
            && let Some(gesture) = HotkeyGesture::from_binding(binding)
            && !gestures.contains(&gesture)
        {
            gestures.push(gesture);
        }
    }
    gestures
}

/// The three bindings in Settings order.
fn mode_rows(bindings: ModeHotkeyBindings) -> [(WorkMode, ShortcutBinding); 3] {
    [
        (WorkMode::Dictation, bindings.dictation),
        (WorkMode::Formatting, bindings.formatting),
        (WorkMode::Assistive, bindings.assistive),
    ]
}

/// Conflicts between Codescribe's own bindings, before macOS is consulted.
///
/// First every mode bound to a gesture the detector never routes to it, then
/// every mode bound to a gesture an earlier mode already uses: one gesture
/// starts one mode.
fn detect_internal_conflicts(bindings: ModeHotkeyBindings) -> Vec<HotkeyConflict> {
    let mut conflicts = Vec::new();
    let rows = mode_rows(bindings);

    for (mode, binding) in rows {
        if let (Some(message), Some(gesture)) = (
            unreachable_binding_message(mode, binding),
            HotkeyGesture::from_binding(binding),
        ) {
            conflicts.push(HotkeyConflict {
                gesture,
                message: message.to_string(),
            });
        }
    }

    for (index, (mode, binding)) in rows.into_iter().enumerate() {
        let Some(gesture) = HotkeyGesture::from_binding(binding) else {
            continue;
        };
        if !mode_binding_reachable(mode, binding) {
            continue;
        }
        if let Some((owner, _)) = rows[..index].iter().find(|(other, other_binding)| {
            *other_binding == binding && mode_binding_reachable(*other, binding)
        }) {
            conflicts.push(HotkeyConflict {
                gesture,
                message: duplicate_binding_message(*owner).to_string(),
            });
        }
    }

    conflicts
}

/// Conflicts against the enabled entries of the macOS symbolic-hotkey registry.
#[cfg(target_os = "macos")]
fn detect_macos_symbolic_conflicts(bindings: ModeHotkeyBindings) -> Vec<HotkeyConflict> {
    let signatures = load_symbolic_signatures();
    if signatures.is_empty() {
        return Vec::new();
    }

    collect_symbolic_conflicts(bindings, &signatures)
}

/// Non-macOS stub: there is no symbolic-hotkey registry to check.
#[cfg(not(target_os = "macos"))]
fn detect_macos_symbolic_conflicts(_bindings: ModeHotkeyBindings) -> Vec<HotkeyConflict> {
    Vec::new()
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// One enabled entry of the macOS symbolic-hotkey registry, reduced to the
/// three fields a collision check needs.
struct SymbolicSignature {
    id: u32,
    keycode: i64,
    modifiers: i64,
}

/// macOS writes `0xFFFF` into the ASCII and keycode slots of a symbolic hotkey
/// that has no key assigned. Such an entry can stay `enabled = 1` in the plist
/// while binding nothing at all, so it must never be read as a real key — least
/// of all as Fn, whose keycode is 63.
#[cfg(target_os = "macos")]
const UNBOUND_KEYCODE: i64 = 65535;

#[cfg(target_os = "macos")]
/// Friendly name for the symbolic IDs worth naming in a conflict message;
/// everything else is reported generically.
fn symbolic_hotkey_name(id: u32) -> &'static str {
    match id {
        160 => "Show Emoji & Symbols (Globe/Fn)",
        164 => "Start Dictation (double Globe/Fn)",
        _ => "macOS system shortcut",
    }
}

#[cfg(target_os = "macos")]
/// Cross every active gesture with every symbolic signature, skipping unbound
/// entries and de-duplicating by `(gesture, symbolic id)`.
fn collect_symbolic_conflicts(
    bindings: ModeHotkeyBindings,
    signatures: &[SymbolicSignature],
) -> Vec<HotkeyConflict> {
    let mut conflicts = Vec::new();
    let mut dedup = HashSet::new();

    for gesture in active_gestures(bindings) {
        for signature in signatures {
            // An unbound entry binds no key, so it cannot collide with anything.
            if signature.keycode == UNBOUND_KEYCODE {
                continue;
            }
            if !gesture_conflicts_with_symbolic(gesture, *signature) {
                continue;
            }
            if !dedup.insert((gesture, signature.id)) {
                continue;
            }
            conflicts.push(HotkeyConflict {
                gesture,
                message: format!(
                    "Conflicts with {} (macOS #{}).",
                    symbolic_hotkey_name(signature.id),
                    signature.id
                ),
            });
        }
    }

    conflicts
}

#[cfg(target_os = "macos")]
/// Whether one gesture actually collides with one symbolic entry.
///
/// Ctrl-based hold gestures always answer `false`; only the modifier-only
/// gestures (Fn, double Ctrl, double Option) are checked against the registry.
fn gesture_conflicts_with_symbolic(gesture: HotkeyGesture, signature: SymbolicSignature) -> bool {
    /// Carbon left-Option keycode used by symbolic-hotkey entries.
    const LEFT_OPTION_KEYCODE: i64 = 58;
    /// Carbon right-Option keycode used by symbolic-hotkey entries.
    const RIGHT_OPTION_KEYCODE: i64 = 61;
    /// Carbon left-Control keycode used by symbolic-hotkey entries.
    const LEFT_CONTROL_KEYCODE: i64 = 59;
    /// Carbon right-Control keycode used by symbolic-hotkey entries.
    const RIGHT_CONTROL_KEYCODE: i64 = 62;
    /// Carbon Fn/Globe keycode; unbound sentinel must not be treated as this.
    const FN_KEYCODE: i64 = 63;

    match gesture {
        HotkeyGesture::HoldFn => {
            // macOS symbolic IDs 160 and 164 are tap/double-tap Globe/Fn actions
            // (Emoji & Symbols / Dictation). Codescribe's HoldFn binding listens
            // for a held modifier gesture, so reporting those tap-only shortcuts as
            // "Hold Fn" conflicts is noisy and misleading.
            //
            // Only the real Fn keycode counts here. Treating the unbound sentinel
            // as Fn made every empty-but-enabled system entry report a phantom
            // "Hold Fn" conflict, which wedged the Save button in Settings.
            signature.id != 160
                && signature.id != 164
                && signature.keycode == FN_KEYCODE
                && signature.modifiers == 0
        }
        HotkeyGesture::ToggleDoubleCtrl => {
            (signature.keycode == LEFT_CONTROL_KEYCODE
                || signature.keycode == RIGHT_CONTROL_KEYCODE)
                && signature.modifiers == 0
        }
        HotkeyGesture::ToggleDoubleLeftOption => {
            signature.keycode == LEFT_OPTION_KEYCODE && signature.modifiers == 0
        }
        HotkeyGesture::ToggleDoubleRightOption => {
            signature.keycode == RIGHT_OPTION_KEYCODE && signature.modifiers == 0
        }
        HotkeyGesture::HoldCtrl
        | HotkeyGesture::HoldCtrlAlt
        | HotkeyGesture::HoldCtrlShift
        | HotkeyGesture::HoldCtrlCmd => false,
    }
}

#[cfg(target_os = "macos")]
/// Whether a symbolic entry binds the same key as the deferred-insert chord.
///
/// The modifier mask must match exactly, so a chord that merely contains the
/// same modifiers is not reported as a collision.
fn deferred_insert_conflicts_with_symbolic(
    shortcut: DeferredInsertShortcut,
    signature: SymbolicSignature,
) -> bool {
    /// Carbon 'V' keycode for deferred-insert chords.
    const V_KEYCODE: i64 = 9;
    /// NSEvent Control modifier bit in symbolic-hotkey parameter packs.
    const CONTROL: i64 = 0x0004_0000;
    /// NSEvent Shift modifier bit in symbolic-hotkey parameter packs.
    const SHIFT: i64 = 0x0002_0000;
    /// NSEvent Option modifier bit in symbolic-hotkey parameter packs.
    const OPTION: i64 = 0x0008_0000;
    /// NSEvent Command modifier bit in symbolic-hotkey parameter packs.
    const COMMAND: i64 = 0x0010_0000;

    let modifiers = match shortcut {
        DeferredInsertShortcut::Disabled => return false,
        DeferredInsertShortcut::CommandOptionV => COMMAND | OPTION,
        DeferredInsertShortcut::CommandShiftV => COMMAND | SHIFT,
        DeferredInsertShortcut::CommandControlV => COMMAND | CONTROL,
    };
    signature.keycode == V_KEYCODE && signature.modifiers == modifiers
}

#[cfg(target_os = "macos")]
/// Read the enabled entries of `com.apple.symbolichotkeys.plist` through
/// `plutil`.
///
/// Every failure path — missing plist, `plutil` unavailable or failing,
/// unparseable JSON, short parameter list — degrades to an empty list, so a
/// machine whose registry cannot be read reports no conflicts rather than
/// blocking Settings.
fn load_symbolic_signatures() -> Vec<SymbolicSignature> {
    use directories::BaseDirs;
    use serde::Deserialize;
    use std::collections::HashMap;
    use std::process::Command;
    use tracing::debug;

    /// Root of `com.apple.symbolichotkeys.plist` after `plutil -convert json`.
    #[derive(Debug, Deserialize)]
    struct SymbolicRegistry {
        #[serde(rename = "AppleSymbolicHotKeys")]
        hotkeys: HashMap<String, SymbolicEntry>,
    }

    /// One system symbolic-hotkey slot; disabled or valueless entries are skipped.
    #[derive(Debug, Deserialize)]
    struct SymbolicEntry {
        #[serde(default)]
        enabled: bool,
        #[serde(default)]
        value: Option<SymbolicValue>,
    }

    /// Payload of an enabled slot; needs ≥3 parameters (ASCII, keycode, modifiers).
    #[derive(Debug, Deserialize)]
    struct SymbolicValue {
        #[serde(default)]
        parameters: Vec<i64>,
    }

    let Some(base_dirs) = BaseDirs::new() else {
        return Vec::new();
    };
    let plist_path = base_dirs
        .home_dir()
        .join("Library/Preferences/com.apple.symbolichotkeys.plist");
    if !plist_path.exists() {
        return Vec::new();
    }

    let output = match Command::new("/usr/bin/plutil")
        .arg("-convert")
        .arg("json")
        .arg("-o")
        .arg("-")
        .arg(&plist_path)
        .output()
    {
        Ok(output) => output,
        Err(e) => {
            debug!(
                "Shortcut registry check skipped; failed to run plutil on {}: {e}",
                plist_path.display()
            );
            return Vec::new();
        }
    };

    if !output.status.success() {
        debug!(
            "Shortcut registry check skipped; plutil failed for {} (status={})",
            plist_path.display(),
            output.status
        );
        return Vec::new();
    }

    let registry: SymbolicRegistry = match serde_json::from_slice(&output.stdout) {
        Ok(registry) => registry,
        Err(e) => {
            debug!(
                "Shortcut registry check skipped; invalid symbolic hotkeys JSON from {}: {e}",
                plist_path.display()
            );
            return Vec::new();
        }
    };

    let mut signatures = Vec::new();
    for (id_raw, entry) in registry.hotkeys {
        if !entry.enabled {
            continue;
        }
        let Some(value) = entry.value else {
            continue;
        };
        if value.parameters.len() < 3 {
            continue;
        }

        let Ok(id) = id_raw.parse::<u32>() else {
            continue;
        };
        signatures.push(SymbolicSignature {
            id,
            keycode: value.parameters[1],
            modifiers: value.parameters[2],
        });
    }

    signatures
}

/// Conflict-detection unit tests for internal and symbolic hotkey collisions.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ModeBinding, WorkMode};

    /// Build settings with only the three mode bindings set; other fields default.
    fn settings_for(
        dictation: ShortcutBinding,
        formatting: ShortcutBinding,
        assistive: ShortcutBinding,
    ) -> UserSettings {
        UserSettings {
            mode_bindings: Some(vec![
                ModeBinding {
                    mode: WorkMode::Dictation,
                    binding: dictation,
                },
                ModeBinding {
                    mode: WorkMode::Formatting,
                    binding: formatting,
                },
                ModeBinding {
                    mode: WorkMode::Assistive,
                    binding: assistive,
                },
            ]),
            ..Default::default()
        }
    }

    /// Double-Ctrl dictation next to opt-in Option gestures for the other
    /// modes is a legal combination: each gesture starts exactly one mode.
    #[test]
    fn double_ctrl_dictation_with_option_gestures_has_no_conflict() {
        let settings = settings_for(
            ShortcutBinding::DoubleCtrl,
            ShortcutBinding::DoubleLeftOption,
            ShortcutBinding::Disabled,
        );
        let conflicts = detect_internal_conflicts(ModeHotkeyBindings::from_settings(&settings));
        assert!(conflicts.is_empty(), "{conflicts:?}");
    }

    /// Distinct hold/toggle gestures for the three modes must report no conflicts.
    #[test]
    fn internal_conflict_empty_for_safe_combo() {
        let settings = settings_for(
            ShortcutBinding::HoldFn,
            ShortcutBinding::DoubleLeftOption,
            ShortcutBinding::DoubleRightOption,
        );
        let conflicts = detect_internal_conflicts(ModeHotkeyBindings::from_settings(&settings));
        assert!(conflicts.is_empty());
    }

    /// Dictation on an Option double-tap is unroutable: Option gestures only
    /// start Formatting or Assistive. The draft carries one blocking conflict
    /// with the Dictation sentence, before any save.
    #[test]
    fn dictation_on_option_gesture_is_blocking_unreachable() {
        let sentence = "This gesture only starts Formatting or Assistive, so Dictation would never start from it.";
        let settings = settings_for(
            ShortcutBinding::DoubleLeftOption,
            ShortcutBinding::DoubleLeftOption,
            ShortcutBinding::DoubleRightOption,
        );
        let conflicts = detect_internal_conflicts(ModeHotkeyBindings::from_settings(&settings));
        assert_eq!(
            conflicts,
            vec![HotkeyConflict {
                gesture: HotkeyGesture::ToggleDoubleLeftOption,
                message: sentence.to_string(),
            }]
        );
        let dictation_only = settings_for(
            ShortcutBinding::DoubleRightOption,
            ShortcutBinding::Disabled,
            ShortcutBinding::Disabled,
        );
        let conflicts =
            detect_internal_conflicts(ModeHotkeyBindings::from_settings(&dictation_only));
        assert_eq!(conflicts.len(), 1, "{conflicts:?}");
        assert_eq!(conflicts[0].gesture, HotkeyGesture::ToggleDoubleRightOption);
        assert_eq!(conflicts[0].message, sentence);
    }

    /// One gesture starts one mode: Formatting and Assistive on the same
    /// reachable gesture yields a single duplicate conflict naming the owner
    /// (the earlier row, Formatting).
    #[test]
    fn duplicate_double_ctrl_names_formatting_as_owner() {
        let settings = settings_for(
            ShortcutBinding::HoldFn,
            ShortcutBinding::DoubleCtrl,
            ShortcutBinding::DoubleCtrl,
        );
        let conflicts = detect_internal_conflicts(ModeHotkeyBindings::from_settings(&settings));
        assert_eq!(
            conflicts,
            vec![HotkeyConflict {
                gesture: HotkeyGesture::ToggleDoubleCtrl,
                message: "This gesture already starts Formatting; one gesture starts one mode."
                    .to_string(),
            }]
        );
    }

    /// Option gestures on Formatting and Assistive are both reachable and
    /// distinct, so the opt-in pair validates clean.
    #[test]
    fn option_gestures_on_formatting_and_assistive_are_clean() {
        let settings = settings_for(
            ShortcutBinding::HoldFn,
            ShortcutBinding::DoubleRightOption,
            ShortcutBinding::DoubleLeftOption,
        );
        let conflicts = detect_internal_conflicts(ModeHotkeyBindings::from_settings(&settings));
        assert!(conflicts.is_empty(), "{conflicts:?}");
    }

    /// A gesture no mode shares can still be unroutable for the mode it is on:
    /// Formatting on Hold Ctrl is persisted today and then does nothing.
    #[test]
    fn unsupported_formatting_hold_is_blocking_without_a_duplicate() {
        let settings = settings_for(
            ShortcutBinding::HoldFn,
            ShortcutBinding::HoldCtrl,
            ShortcutBinding::DoubleRightOption,
        );
        let conflicts = detect_internal_conflicts(ModeHotkeyBindings::from_settings(&settings));
        assert_eq!(conflicts.len(), 1, "{conflicts:?}");
        assert_eq!(conflicts[0].gesture, HotkeyGesture::HoldCtrl);
        assert_eq!(
            conflicts[0].message,
            "This gesture only starts Dictation, so Formatting would never start from it."
        );
    }

    /// Every mode × gesture cell: a message exactly when the detector does not
    /// route the cell, and the supported matrix is: holds -> Dictation only,
    /// Option gestures -> Formatting or Assistive only, Double Ctrl -> any mode.
    #[test]
    fn unreachable_message_covers_exactly_the_unrouted_cells() {
        let gestures = [
            ShortcutBinding::Disabled,
            ShortcutBinding::HoldFn,
            ShortcutBinding::HoldCtrl,
            ShortcutBinding::HoldCtrlAlt,
            ShortcutBinding::HoldCtrlShift,
            ShortcutBinding::HoldCtrlCmd,
            ShortcutBinding::DoubleCtrl,
            ShortcutBinding::DoubleLeftOption,
            ShortcutBinding::DoubleRightOption,
        ];
        let mut refused = 0;
        for mode in [
            WorkMode::Dictation,
            WorkMode::Formatting,
            WorkMode::Assistive,
        ] {
            for binding in gestures {
                let message = unreachable_binding_message(mode, binding);
                assert_eq!(
                    message.is_none(),
                    mode_binding_reachable(mode, binding),
                    "{mode:?} × {binding:?}"
                );
                refused += usize::from(message.is_some());
            }
        }
        // 27 cells − 3 Disabled − 12 routed (5 Dictation holds + 3 Double Ctrl
        // + 4 Option cells on Formatting/Assistive) = 12 refused: 10 hold cells on
        // Formatting/Assistive + 2 Option cells on Dictation.
        assert_eq!(refused, 12);
    }

    /// The allowed default combinations validate clean.
    #[test]
    fn allowed_combinations_carry_no_internal_conflict() {
        for (dictation, formatting, assistive) in [
            (
                ShortcutBinding::HoldFn,
                ShortcutBinding::DoubleLeftOption,
                ShortcutBinding::DoubleRightOption,
            ),
            (
                ShortcutBinding::HoldCtrlCmd,
                ShortcutBinding::DoubleLeftOption,
                ShortcutBinding::DoubleRightOption,
            ),
            (
                ShortcutBinding::DoubleCtrl,
                ShortcutBinding::Disabled,
                ShortcutBinding::Disabled,
            ),
            (
                ShortcutBinding::Disabled,
                ShortcutBinding::DoubleLeftOption,
                ShortcutBinding::DoubleRightOption,
            ),
        ] {
            let settings = settings_for(dictation, formatting, assistive);
            let conflicts = detect_internal_conflicts(ModeHotkeyBindings::from_settings(&settings));
            assert!(
                conflicts.is_empty(),
                "{dictation:?}/{formatting:?}/{assistive:?}: {conflicts:?}"
            );
        }
    }

    /// Assistive on the dictation hold: the detector never routes a hold to
    /// Assistive, and Dictation owns that hold.
    #[test]
    fn internal_conflict_detects_assistive_dictation_binding_collision() {
        let settings = settings_for(
            ShortcutBinding::HoldCtrlCmd,
            ShortcutBinding::DoubleLeftOption,
            ShortcutBinding::HoldCtrlCmd,
        );
        let conflicts = detect_internal_conflicts(ModeHotkeyBindings::from_settings(&settings));
        assert!(
            conflicts.iter().any(|c| c.gesture == HotkeyGesture::HoldCtrlCmd
                && c.message
                    == "This gesture only starts Dictation, so Assistive would never start from it."),
            "shared dictation/assistive hold binding should be reported as conflict: {conflicts:?}"
        );
    }

    /// The active set is exactly the routed cells: double-Ctrl dictation and
    /// opt-in Option gestures are visible; an unroutable assistive hold is
    /// not (it already carries its own blocking unreachable conflict).
    #[test]
    fn active_gestures_follow_the_routed_cells() {
        let bindings = ModeHotkeyBindings {
            dictation: ShortcutBinding::DoubleCtrl,
            formatting: ShortcutBinding::DoubleLeftOption,
            assistive: ShortcutBinding::HoldCtrlCmd,
        };
        let gestures = active_gestures(bindings);
        assert!(gestures.contains(&HotkeyGesture::ToggleDoubleCtrl));
        assert!(gestures.contains(&HotkeyGesture::ToggleDoubleLeftOption));
        assert!(
            !gestures.contains(&HotkeyGesture::HoldCtrlCmd),
            "an unrouted assistive hold is not an active gesture"
        );
        let disabled = ModeHotkeyBindings {
            dictation: ShortcutBinding::HoldFn,
            formatting: ShortcutBinding::Disabled,
            assistive: ShortcutBinding::Disabled,
        };
        assert_eq!(active_gestures(disabled), vec![HotkeyGesture::HoldFn]);
    }

    /// macOS Fn tap/double-tap system IDs must not be reported as Hold Fn conflicts.
    #[cfg(target_os = "macos")]
    #[test]
    fn symbolic_conflict_ignores_macos_fn_tap_for_hold_fn() {
        let settings = settings_for(
            ShortcutBinding::HoldFn,
            ShortcutBinding::Disabled,
            ShortcutBinding::Disabled,
        );
        let signatures = vec![
            SymbolicSignature {
                id: 160,
                keycode: 65535,
                modifiers: 0,
            },
            SymbolicSignature {
                id: 164,
                keycode: 65535,
                modifiers: 0,
            },
        ];

        let conflicts =
            collect_symbolic_conflicts(ModeHotkeyBindings::from_settings(&settings), &signatures);
        assert!(
            conflicts.is_empty(),
            "macOS Fn tap shortcuts must not be reported as Hold Fn conflicts"
        );
    }

    /// A real Fn keycode with zero modifiers must collide with Hold Fn.
    #[cfg(target_os = "macos")]
    #[test]
    fn symbolic_conflict_detects_raw_fn_hold_signature() {
        let settings = settings_for(
            ShortcutBinding::HoldFn,
            ShortcutBinding::Disabled,
            ShortcutBinding::Disabled,
        );
        let signatures = vec![SymbolicSignature {
            id: 999,
            keycode: 63,
            modifiers: 0,
        }];

        let conflicts =
            collect_symbolic_conflicts(ModeHotkeyBindings::from_settings(&settings), &signatures);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].gesture, HotkeyGesture::HoldFn);
    }

    /// Regression: macOS keeps system shortcut slots `enabled = 1` while leaving
    /// them unbound (`0xFFFF` in the ASCII and keycode slots). Those carry
    /// arbitrary IDs, so an ID allow-list cannot catch them — the unbound
    /// keycode itself has to disqualify the entry. Reading the sentinel as Fn
    /// turned six empty slots on a stock macOS install into six phantom
    /// conflicts and disabled Save in Settings → Hotkeys.
    #[cfg(target_os = "macos")]
    #[test]
    fn symbolic_conflict_ignores_unbound_entries_with_arbitrary_ids() {
        let settings = settings_for(
            ShortcutBinding::HoldFn,
            ShortcutBinding::DoubleLeftOption,
            ShortcutBinding::DoubleRightOption,
        );
        let signatures: Vec<SymbolicSignature> = [235, 244, 245, 246, 247, 256]
            .into_iter()
            .map(|id| SymbolicSignature {
                id,
                keycode: UNBOUND_KEYCODE,
                modifiers: 0,
            })
            .collect();

        let conflicts =
            collect_symbolic_conflicts(ModeHotkeyBindings::from_settings(&settings), &signatures);
        assert!(
            conflicts.is_empty(),
            "unbound macOS entries must never be reported as conflicts, got: {conflicts:?}"
        );
    }

    /// Deferred-insert collision requires exact keycode and exact modifier mask.
    #[cfg(target_os = "macos")]
    #[test]
    fn deferred_insert_collision_matches_key_and_exact_modifiers() {
        let collision = SymbolicSignature {
            id: 777,
            keycode: 9,
            modifiers: 0x0010_0000 | 0x0008_0000,
        };
        assert!(deferred_insert_conflicts_with_symbolic(
            DeferredInsertShortcut::CommandOptionV,
            collision
        ));
        assert!(!deferred_insert_conflicts_with_symbolic(
            DeferredInsertShortcut::CommandShiftV,
            collision
        ));
    }
}
