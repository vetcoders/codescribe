//! Conservative, backup-first repair of app-owned settings. Unknown data survives.
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::settings::{FormattingPolicy, SettingsV2, UserSettings};

/// Receipt values are limited to the two non-secret settings repaired here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RepairAction {
    FieldReset {
        field: String,
        from: Value,
        to: Value,
    },
    FileRecreated {
        reason: String,
    },
    SeededFromPack {
        field: String,
    },
    UnknownEnvKey {
        key: String,
    },
    PrecedenceNote {
        key: String,
    },
}

/// A refusal preserves the source file; callers can expose it without its contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigUnrepairable {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RepairReceipt {
    pub actions: Vec<RepairAction>,
    pub backups: Vec<PathBuf>,
    pub unrepairable: Vec<ConfigUnrepairable>,
}

impl RepairReceipt {
    pub fn summary(&self) -> Option<String> {
        if !self.unrepairable.is_empty() {
            return Some(format!(
                "Config needs attention: {} ({})",
                self.unrepairable[0].reason,
                self.unrepairable[0].path.display()
            ));
        }
        if self.actions.is_empty() {
            return None;
        }
        let changes = self
            .actions
            .iter()
            .filter(|a| {
                matches!(
                    a,
                    RepairAction::FieldReset { .. }
                        | RepairAction::FileRecreated { .. }
                        | RepairAction::SeededFromPack { .. }
                )
            })
            .count();
        let notes = self.actions.len() - changes;
        if notes > 0 {
            return Some(format!(
                "Config: {changes} repairs at launch; {notes} env key(s) need review (backup {})",
                self.backups
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        Some(format!(
            "Config repaired at launch: {} changes (backup {})",
            self.actions.len(),
            self.backups
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }
}

fn receipts() -> &'static Mutex<std::collections::BTreeMap<PathBuf, RepairReceipt>> {
    static RECEIPTS: OnceLock<Mutex<std::collections::BTreeMap<PathBuf, RepairReceipt>>> =
        OnceLock::new();
    RECEIPTS.get_or_init(Mutex::default)
}

/// Process-lifetime diagnostic receipt. Reading it never reloads or repairs files.
pub fn launch_receipt() -> RepairReceipt {
    receipts()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&UserSettings::settings_path())
        .cloned()
        .unwrap_or_default()
}

pub(super) fn record(receipt: RepairReceipt) {
    let mut receipts = receipts().lock().unwrap_or_else(|e| e.into_inner());
    let all = receipts.entry(UserSettings::settings_path()).or_default();
    for action in receipt.actions {
        if !all.actions.contains(&action) {
            all.actions.push(action);
        }
    }
    all.backups.extend(receipt.backups);
    for error in receipt.unrepairable {
        if !all.unrepairable.contains(&error) {
            all.unrepairable.push(error);
        }
    }
}

/// A unique UTC backup, private even when an old config had permissive mode bits.
fn backup(path: &Path, bytes: &[u8]) -> anyhow::Result<PathBuf> {
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.9fZ");
    let target = path.with_file_name(format!(
        "{}.bak-{stamp}-{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        uuid::Uuid::new_v4()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&target)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(target)
}

fn reset(value: &mut Value, pointer: &str, replacement: Value, receipt: &mut RepairReceipt) {
    if let Some(slot) = value.pointer_mut(pointer) {
        let from = std::mem::replace(slot, replacement.clone());
        receipt.actions.push(RepairAction::FieldReset {
            field: pointer.trim_start_matches('/').replace('/', "."),
            from,
            to: replacement,
        });
    }
}

/// Called with settings I/O lock held. No write happens until the entire candidate validates.
pub(super) fn repair_settings(path: &Path, pack: Option<&Path>) -> RepairReceipt {
    let mut receipt = RepairReceipt::default();
    let outcome = (|| -> anyhow::Result<()> {
        let original = match fs::read(path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        let defaults = serde_json::to_value(UserSettings::default().to_v2())?;
        let mut value = match original.as_deref().map(serde_json::from_slice::<Value>) {
            Some(Ok(value)) => value,
            Some(Err(_)) => {
                receipt.actions.push(RepairAction::FileRecreated {
                    reason: "invalid JSON; original bytes preserved in backup".into(),
                });
                defaults.clone()
            }
            None => defaults,
        };
        // V1 belongs to the existing migration; future schemas must not be downgraded.
        if value.get("schema_version").is_none() {
            serde_json::from_value::<UserSettings>(value)
                .map_err(|_| anyhow::anyhow!("invalid legacy settings; source left untouched"))?;
            return Ok(());
        }
        if !matches!(value["schema_version"].as_u64(), Some(2 | 3)) {
            anyhow::bail!("unsupported settings schema; source left untouched");
        }
        if let Some(zoom) = value.pointer("/ui/chat_zoom")
            && !zoom.is_null()
            && !zoom.as_f64().is_some_and(|v| (0.75..=2.0).contains(&v))
        {
            reset(&mut value, "/ui/chat_zoom", Value::from(1.0), &mut receipt);
        }
        if let Some(level) = value.pointer("/speech/formatting/level")
            && !level.is_null()
            && level
                .as_str()
                .is_none_or(|s| FormattingPolicy::parse(s).is_err())
        {
            reset(
                &mut value,
                "/speech/formatting/level",
                Value::from("off"),
                &mut receipt,
            );
        }
        if let Some(pack) = pack {
            let seed: Value = serde_json::from_slice(&fs::read(pack)?)?;
            for key in ["cloud_transcription_endpoint", "asr_mode"] {
                let pointer = format!("/speech/engine/{key}");
                let Some(wanted) = seed
                    .pointer(&pointer)
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                else {
                    continue;
                };
                if value
                    .pointer(&pointer)
                    .is_none_or(|v| v.is_null() || v.as_str().is_some_and(|s| s.trim().is_empty()))
                {
                    let root = value
                        .as_object_mut()
                        .ok_or_else(|| anyhow::anyhow!("settings root must be an object"))?;
                    let speech = root
                        .entry("speech")
                        .or_insert_with(|| serde_json::json!({}));
                    if speech.is_null() {
                        *speech = serde_json::json!({});
                    }
                    let speech = speech
                        .as_object_mut()
                        .ok_or_else(|| anyhow::anyhow!("speech must be an object"))?;
                    let engine = speech
                        .entry("engine")
                        .or_insert_with(|| serde_json::json!({}));
                    if engine.is_null() {
                        *engine = serde_json::json!({});
                    }
                    engine
                        .as_object_mut()
                        .ok_or_else(|| anyhow::anyhow!("engine must be an object"))?
                        .insert(key.into(), Value::from(wanted));
                    receipt.actions.push(RepairAction::SeededFromPack {
                        field: pointer.trim_start_matches('/').replace('/', "."),
                    });
                }
            }
        }
        let candidate: SettingsV2 = serde_json::from_value(value.clone())
            .map_err(|_| anyhow::anyhow!("invalid settings field type; source left untouched"))?;
        UserSettings::validate_v2(&candidate)?;
        if receipt.actions.is_empty() {
            return Ok(());
        }
        fs::create_dir_all(
            path.parent()
                .ok_or_else(|| anyhow::anyhow!("missing settings directory"))?,
        )?;
        if let Some(bytes) = original {
            receipt.backups.push(backup(path, &bytes)?);
        }
        UserSettings::write_json_atomic(path, &serde_json::to_string_pretty(&value)?)?;
        Ok(())
    })();
    if outcome.is_err() {
        receipt.actions.clear();
        receipt.unrepairable.push(ConfigUnrepairable {
            path: path.into(),
            reason:
                "settings repair refused; inspect file schema, field types and filesystem access"
                    .into(),
        });
    }
    receipt
}

pub(super) fn operator_pack() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe()
        && let Some(contents) = exe.parent().and_then(Path::parent)
    {
        let bundled = contents.join("Resources/operator-pack/settings.json");
        if bundled.is_file() {
            return Some(bundled);
        }
    }
    let root = std::env::var_os("CODESCRIBE_VOICE_LAB_SRC").map(PathBuf::from)?;
    let mut candidates = fs::read_dir(root.join("examples"))
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect::<Vec<_>>();
    candidates.sort();
    candidates
        .into_iter()
        .find(|p| p.join("keys").is_dir() && p.join("settings.json").is_file())
        .map(|p| p.join("settings.json"))
}

/// Report-only migration for retired registry entries. The registry still points
/// several aliases at removed readers; rewriting those would falsely claim repair.
pub(super) fn inspect_env(path: &Path) -> RepairReceipt {
    let mut receipt = RepairReceipt::default();
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return receipt,
        Err(_) => {
            receipt.unrepairable.push(ConfigUnrepairable {
                path: path.into(),
                reason: "cannot read optional env file".into(),
            });
            return receipt;
        }
    };
    let registry = include_str!("../../docs/ENV_REGISTRY.toml");
    let known = registry
        .lines()
        .filter_map(|line| line.strip_prefix("[vars.")?.strip_suffix(']'))
        .collect::<std::collections::HashSet<_>>();
    let mut deprecated = std::collections::HashSet::new();
    let mut current = "";
    for line in registry.lines() {
        if let Some(key) = line
            .strip_prefix("[vars.")
            .and_then(|s| s.strip_suffix(']'))
        {
            current = key;
        }
        if line.starts_with("deprecated =") {
            deprecated.insert(current);
        }
    }
    for line in source.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((key, _)) = line.strip_prefix("export ").unwrap_or(line).split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || !key.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
            continue;
        }
        if !known.contains(key) {
            receipt
                .actions
                .push(RepairAction::UnknownEnvKey { key: key.into() });
        } else if deprecated.contains(key) {
            receipt
                .actions
                .push(RepairAction::PrecedenceNote { key: key.into() });
        }
    }
    receipt
}

pub(super) fn log_launch_once() {
    static LOGGED: std::sync::Once = std::sync::Once::new();
    LOGGED.call_once(|| {
        let receipt = launch_receipt();
        let backup = receipt
            .backups
            .first()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "none".into());
        tracing::info!(
            "config_repair actions={} backup={}",
            receipt.actions.len(),
            backup
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repair_preserves_intent_and_original_bytes_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let bytes = br#"{"schema_version":3,"ui":{"chat_zoom":9},"future":{"custom":"keep"},"speech":{"formatting":{"level":"smart"}}}"#;
        fs::write(&path, bytes).unwrap();
        let receipt = repair_settings(&path, None);
        assert!(receipt.unrepairable.is_empty());
        assert_eq!(receipt.actions.len(), 1);
        assert_eq!(fs::read(&receipt.backups[0]).unwrap(), bytes);
        let value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["ui"]["chat_zoom"], 1.0);
        assert_eq!(value["future"]["custom"], "keep");
        assert_eq!(value["speech"]["formatting"]["level"], "smart");
        assert_eq!(repair_settings(&path, None), RepairReceipt::default());
    }

    #[cfg(unix)]
    #[test]
    fn repair_keeps_settings_and_backup_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, r#"{"schema_version":3,"ui":{"chat_zoom":9}}"#).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let receipt = repair_settings(&path, None);
        assert_eq!(receipt.actions.len(), 1);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&receipt.backups[0])
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn truncated_json_is_backed_up_before_recreation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, b"{\"ui\":").unwrap();
        let receipt = repair_settings(&path, None);
        assert!(matches!(
            receipt.actions.as_slice(),
            [RepairAction::FileRecreated { .. }]
        ));
        assert_eq!(fs::read(&receipt.backups[0]).unwrap(), b"{\"ui\":");
        assert!(serde_json::from_slice::<SettingsV2>(&fs::read(&path).unwrap()).is_ok());
    }

    #[test]
    fn unsupported_schema_and_bad_types_remain_untouched() {
        for bytes in [
            r#"{"schema_version":99,"ui":{"chat_zoom":9}}"#,
            r#"{"schema_version":3,"audio":false}"#,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("settings.json");
            fs::write(&path, bytes).unwrap();
            let receipt = repair_settings(&path, None);
            assert_eq!(receipt.unrepairable.len(), 1);
            assert!(receipt.actions.is_empty());
            assert_eq!(fs::read_to_string(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn pack_fills_only_empty_engine_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let pack = dir.path().join("pack.json");
        fs::write(&path, r#"{"schema_version":3,"speech":{"engine":{"asr_mode":"local","cloud_transcription_endpoint":""}}}"#).unwrap();
        fs::write(&pack, r#"{"speech":{"engine":{"asr_mode":"cloud","cloud_transcription_endpoint":"https://example.test/asr"}}}"#).unwrap();
        let receipt = repair_settings(&path, Some(&pack));
        assert_eq!(receipt.actions.len(), 1);
        let value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["speech"]["engine"]["asr_mode"], "local");
        assert_eq!(
            repair_settings(&path, Some(&pack)),
            RepairReceipt::default()
        );
    }
}
