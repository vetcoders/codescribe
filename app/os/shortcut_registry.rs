//! macOS keyboard shortcut registry conflict checks for CodeScribe hotkeys.
//!
//! Reads the system SymbolicHotkeys registry and reports potential collisions
//! with our modifier-only gestures (Fn/Ctrl/Option).

use crate::config::{Config, HoldMods, ToggleTrigger};
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

pub fn detect_hotkey_conflicts(config: &Config) -> Vec<HotkeyConflict> {
    let mut conflicts = detect_internal_conflicts(config);
    conflicts.extend(detect_macos_symbolic_conflicts(config));
    conflicts
}

fn active_gestures(config: &Config) -> Vec<HotkeyGesture> {
    let mut gestures = Vec::new();

    match config.hold_mods {
        HoldMods::Fn => gestures.push(HotkeyGesture::HoldFn),
        HoldMods::None => {}
        HoldMods::Ctrl => gestures.push(HotkeyGesture::HoldCtrl),
        HoldMods::CtrlAlt => gestures.push(HotkeyGesture::HoldCtrlAlt),
        HoldMods::CtrlShift => gestures.push(HotkeyGesture::HoldCtrlShift),
        HoldMods::CtrlCmd => gestures.push(HotkeyGesture::HoldCtrlCmd),
    }

    match config.toggle_trigger {
        ToggleTrigger::None => {}
        ToggleTrigger::DoubleCtrl => gestures.push(HotkeyGesture::ToggleDoubleCtrl),
        ToggleTrigger::DoubleLeftOption => gestures.push(HotkeyGesture::ToggleDoubleLeftOption),
        ToggleTrigger::DoubleRightOption => gestures.push(HotkeyGesture::ToggleDoubleRightOption),
        ToggleTrigger::DoubleOption => {
            gestures.push(HotkeyGesture::ToggleDoubleLeftOption);
            gestures.push(HotkeyGesture::ToggleDoubleRightOption);
        }
    }

    gestures
}

fn detect_internal_conflicts(config: &Config) -> Vec<HotkeyConflict> {
    let mut conflicts = Vec::new();

    if config.hold_mods == HoldMods::Ctrl && config.toggle_trigger == ToggleTrigger::DoubleCtrl {
        conflicts.push(HotkeyConflict {
            gesture: HotkeyGesture::ToggleDoubleCtrl,
            message: "Collides with Hold Ctrl. Use Ctrl+Option hold or disable Double Ctrl toggle."
                .to_string(),
        });
    }

    conflicts
}

#[cfg(target_os = "macos")]
fn detect_macos_symbolic_conflicts(config: &Config) -> Vec<HotkeyConflict> {
    let signatures = load_symbolic_signatures();
    if signatures.is_empty() {
        return Vec::new();
    }

    collect_symbolic_conflicts(config, &signatures)
}

#[cfg(not(target_os = "macos"))]
fn detect_macos_symbolic_conflicts(_config: &Config) -> Vec<HotkeyConflict> {
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
    config: &Config,
    signatures: &[SymbolicSignature],
) -> Vec<HotkeyConflict> {
    let mut conflicts = Vec::new();
    let mut dedup = HashSet::new();

    for gesture in active_gestures(config) {
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
            signature.id == 160
                || signature.id == 164
                || ((signature.keycode == FN_KEYCODE || signature.keycode == 65535)
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

    fn config_for(hold_mods: HoldMods, toggle_trigger: ToggleTrigger) -> Config {
        Config {
            hold_mods,
            toggle_trigger,
            ..Default::default()
        }
    }

    #[test]
    fn internal_conflict_detects_ctrl_hold_vs_double_ctrl() {
        let cfg = config_for(HoldMods::Ctrl, ToggleTrigger::DoubleCtrl);
        let conflicts = detect_internal_conflicts(&cfg);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].gesture, HotkeyGesture::ToggleDoubleCtrl);
    }

    #[test]
    fn internal_conflict_empty_for_safe_combo() {
        let cfg = config_for(HoldMods::Fn, ToggleTrigger::DoubleOption);
        let conflicts = detect_internal_conflicts(&cfg);
        assert!(conflicts.is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn symbolic_conflict_detects_fn_registry_collision() {
        let cfg = config_for(HoldMods::Fn, ToggleTrigger::None);
        let signatures = vec![SymbolicSignature {
            id: 160,
            keycode: 65535,
            modifiers: 0,
        }];

        let conflicts = collect_symbolic_conflicts(&cfg, &signatures);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].gesture, HotkeyGesture::HoldFn);
    }
}
