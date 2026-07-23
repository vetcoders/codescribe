//! macOS keyboard shortcut registry conflict checks for Codescribe hotkeys.
//!
//! Reads the system SymbolicHotkeys registry and reports potential collisions
//! with our modifier-only gestures (Fn/Ctrl/Option).

use crate::config::{DeferredInsertShortcut, ShortcutBinding, UserSettings};
use crate::os::hotkeys::ModeHotkeyBindings;
#[cfg(target_os = "macos")]
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyConflict {
    pub gesture: HotkeyGesture,
    pub message: String,
}

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

#[cfg(not(target_os = "macos"))]
pub fn deferred_insert_shortcut_conflict(shortcut: DeferredInsertShortcut) -> Option<String> {
    (!shortcut.is_enabled()).then(|| "Deferred insert shortcut is disabled".to_string())
}

pub fn fn_tap_intercept_note(settings: &UserSettings) -> Option<&'static str> {
    let bindings = ModeHotkeyBindings::from_settings(settings);
    if bindings.dictation != ShortcutBinding::HoldFn {
        return None;
    }

    fn_tap_symbols_enabled().then_some(
        "Fn/Globe tap is configured by macOS. Codescribe Hold Fn may intercept that tap while dictation is active; this is informational, not a shortcut conflict.",
    )
}

#[cfg(target_os = "macos")]
fn fn_tap_symbols_enabled() -> bool {
    load_symbolic_signatures()
        .iter()
        .any(|signature| matches!(signature.id, 160 | 164))
}

#[cfg(not(target_os = "macos"))]
fn fn_tap_symbols_enabled() -> bool {
    false
}

fn active_gestures(bindings: ModeHotkeyBindings) -> Vec<HotkeyGesture> {
    let mut gestures = Vec::new();

    if bindings.dictation == ShortcutBinding::DoubleCtrl {
        gestures.push(HotkeyGesture::ToggleDoubleCtrl);
    }

    match bindings.dictation {
        ShortcutBinding::HoldFn => gestures.push(HotkeyGesture::HoldFn),
        ShortcutBinding::HoldCtrl => gestures.push(HotkeyGesture::HoldCtrl),
        ShortcutBinding::HoldCtrlAlt => gestures.push(HotkeyGesture::HoldCtrlAlt),
        ShortcutBinding::HoldCtrlShift => gestures.push(HotkeyGesture::HoldCtrlShift),
        ShortcutBinding::HoldCtrlCmd => gestures.push(HotkeyGesture::HoldCtrlCmd),
        ShortcutBinding::Disabled
        | ShortcutBinding::DoubleCtrl
        | ShortcutBinding::DoubleLeftOption
        | ShortcutBinding::DoubleRightOption => {}
    }
    if bindings.formatting == ShortcutBinding::DoubleLeftOption {
        gestures.push(HotkeyGesture::ToggleDoubleLeftOption);
    }
    match bindings.assistive {
        ShortcutBinding::DoubleRightOption => gestures.push(HotkeyGesture::ToggleDoubleRightOption),
        ShortcutBinding::HoldFn => gestures.push(HotkeyGesture::HoldFn),
        ShortcutBinding::HoldCtrl => gestures.push(HotkeyGesture::HoldCtrl),
        ShortcutBinding::HoldCtrlAlt => gestures.push(HotkeyGesture::HoldCtrlAlt),
        ShortcutBinding::HoldCtrlShift => gestures.push(HotkeyGesture::HoldCtrlShift),
        ShortcutBinding::HoldCtrlCmd => gestures.push(HotkeyGesture::HoldCtrlCmd),
        ShortcutBinding::Disabled
        | ShortcutBinding::DoubleCtrl
        | ShortcutBinding::DoubleLeftOption => {}
    }

    gestures
}

fn detect_internal_conflicts(bindings: ModeHotkeyBindings) -> Vec<HotkeyConflict> {
    let mut conflicts = Vec::new();

    if bindings.dictation == ShortcutBinding::DoubleCtrl
        && bindings.formatting == ShortcutBinding::DoubleLeftOption
    {
        conflicts.push(HotkeyConflict {
            gesture: HotkeyGesture::ToggleDoubleLeftOption,
            message: "Dictation is set to Double Ctrl, so Left Option toggle is disabled."
                .to_string(),
        });
    }

    if bindings.dictation == ShortcutBinding::DoubleCtrl
        && bindings.assistive == ShortcutBinding::DoubleRightOption
    {
        conflicts.push(HotkeyConflict {
            gesture: HotkeyGesture::ToggleDoubleRightOption,
            message: "Dictation is set to Double Ctrl, so Right Option toggle is disabled."
                .to_string(),
        });
    }

    if bindings.assistive != ShortcutBinding::Disabled && bindings.dictation == bindings.assistive {
        let gesture = match bindings.assistive {
            ShortcutBinding::HoldFn => HotkeyGesture::HoldFn,
            ShortcutBinding::HoldCtrl => HotkeyGesture::HoldCtrl,
            ShortcutBinding::HoldCtrlAlt => HotkeyGesture::HoldCtrlAlt,
            ShortcutBinding::HoldCtrlShift => HotkeyGesture::HoldCtrlShift,
            ShortcutBinding::HoldCtrlCmd => HotkeyGesture::HoldCtrlCmd,
            ShortcutBinding::DoubleCtrl => HotkeyGesture::ToggleDoubleCtrl,
            ShortcutBinding::DoubleLeftOption => HotkeyGesture::ToggleDoubleLeftOption,
            ShortcutBinding::DoubleRightOption => HotkeyGesture::ToggleDoubleRightOption,
            ShortcutBinding::Disabled => HotkeyGesture::HoldFn,
        };
        conflicts.push(HotkeyConflict {
            gesture,
            message:
                "Dictation and Assistive use the same binding; Assistive selection shortcut may not be reachable."
                    .to_string(),
        });
    }

    conflicts
}

#[cfg(target_os = "macos")]
fn detect_macos_symbolic_conflicts(bindings: ModeHotkeyBindings) -> Vec<HotkeyConflict> {
    let signatures = load_symbolic_signatures();
    if signatures.is_empty() {
        return Vec::new();
    }

    collect_symbolic_conflicts(bindings, &signatures)
}

#[cfg(not(target_os = "macos"))]
fn detect_macos_symbolic_conflicts(_bindings: ModeHotkeyBindings) -> Vec<HotkeyConflict> {
    Vec::new()
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct SymbolicSignature {
    id: u32,
    keycode: i64,
    modifiers: i64,
}

#[cfg(target_os = "macos")]
fn symbolic_hotkey_name(id: u32) -> &'static str {
    match id {
        160 => "Show Emoji & Symbols (Globe/Fn)",
        164 => "Start Dictation (double Globe/Fn)",
        _ => "macOS system shortcut",
    }
}

#[cfg(target_os = "macos")]
fn collect_symbolic_conflicts(
    bindings: ModeHotkeyBindings,
    signatures: &[SymbolicSignature],
) -> Vec<HotkeyConflict> {
    let mut conflicts = Vec::new();
    let mut dedup = HashSet::new();

    for gesture in active_gestures(bindings) {
        for signature in signatures {
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
fn gesture_conflicts_with_symbolic(gesture: HotkeyGesture, signature: SymbolicSignature) -> bool {
    const LEFT_OPTION_KEYCODE: i64 = 58;
    const RIGHT_OPTION_KEYCODE: i64 = 61;
    const LEFT_CONTROL_KEYCODE: i64 = 59;
    const RIGHT_CONTROL_KEYCODE: i64 = 62;
    const FN_KEYCODE: i64 = 63;

    match gesture {
        HotkeyGesture::HoldFn => {
            // macOS symbolic IDs 160 and 164 are tap/double-tap Globe/Fn actions
            // (Emoji & Symbols / Dictation). Codescribe's HoldFn binding listens
            // for a held modifier gesture, so reporting those tap-only shortcuts as
            // "Hold Fn" conflicts is noisy and misleading.
            signature.id != 160
                && signature.id != 164
                && ((signature.keycode == FN_KEYCODE || signature.keycode == 65535)
                    && signature.modifiers == 0)
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
fn deferred_insert_conflicts_with_symbolic(
    shortcut: DeferredInsertShortcut,
    signature: SymbolicSignature,
) -> bool {
    const V_KEYCODE: i64 = 9;
    const CONTROL: i64 = 0x0004_0000;
    const SHIFT: i64 = 0x0002_0000;
    const OPTION: i64 = 0x0008_0000;
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
fn load_symbolic_signatures() -> Vec<SymbolicSignature> {
    use directories::BaseDirs;
    use serde::Deserialize;
    use std::collections::HashMap;
    use std::process::Command;
    use tracing::debug;

    #[derive(Debug, Deserialize)]
    struct SymbolicRegistry {
        #[serde(rename = "AppleSymbolicHotKeys")]
        hotkeys: HashMap<String, SymbolicEntry>,
    }

    #[derive(Debug, Deserialize)]
    struct SymbolicEntry {
        #[serde(default)]
        enabled: bool,
        #[serde(default)]
        value: Option<SymbolicValue>,
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ModeBinding, WorkMode};

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

    #[test]
    fn internal_conflict_detects_double_ctrl_vs_left_option() {
        let settings = settings_for(
            ShortcutBinding::DoubleCtrl,
            ShortcutBinding::DoubleLeftOption,
            ShortcutBinding::Disabled,
        );
        let conflicts = detect_internal_conflicts(ModeHotkeyBindings::from_settings(&settings));
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].gesture, HotkeyGesture::ToggleDoubleLeftOption);
    }

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

    #[test]
    fn internal_conflict_detects_assistive_dictation_binding_collision() {
        let settings = settings_for(
            ShortcutBinding::HoldCtrlCmd,
            ShortcutBinding::DoubleLeftOption,
            ShortcutBinding::HoldCtrlCmd,
        );
        let conflicts = detect_internal_conflicts(ModeHotkeyBindings::from_settings(&settings));
        assert!(
            conflicts
                .iter()
                .any(|c| c.gesture == HotkeyGesture::HoldCtrlCmd),
            "shared dictation/assistive hold binding should be reported as conflict"
        );
    }

    #[test]
    fn active_gestures_keeps_assistive_hold_visible_with_double_ctrl_dictation() {
        let bindings = ModeHotkeyBindings {
            dictation: ShortcutBinding::DoubleCtrl,
            formatting: ShortcutBinding::Disabled,
            assistive: ShortcutBinding::HoldCtrlCmd,
        };
        let gestures = active_gestures(bindings);
        assert!(
            gestures.contains(&HotkeyGesture::ToggleDoubleCtrl),
            "dictation double-ctrl toggle must remain visible"
        );
        assert!(
            gestures.contains(&HotkeyGesture::HoldCtrlCmd),
            "assistive hold binding must remain visible for conflict checks"
        );
    }

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
