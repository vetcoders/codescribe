//! Secret-free record of one settings capture, kept by a take's replay card.
//!
//! A replay reseals the settings the take ran with, not whatever the host holds
//! by then. Credential values never enter the record: only their presence,
//! which is all the snapshot digest reads. Rebuilding therefore reproduces the
//! take's `SettingsSnapshotDigest` exactly, and a digest mismatch names a real
//! difference instead of a redaction artifact.

use std::collections::{BTreeMap, HashMap};
use std::env::VarError;
use std::ffi::OsString;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::energy_calibration::SealedEnergyCalibration;
use super::loader::{CapturedLaneCredential, CapturedRuntimeInputs};
use super::prompts::CapturedRuntimePrompts;
use super::repair::RepairReceipt;
use super::settings::UserSettings;
use super::types::Config;

/// Stand-in for a secret that was present at capture. The digest material
/// already redacts these values to presence, so the stand-in keeps it equal.
pub const REDACTED_SECRET: &str = "<redacted:present>";

/// One runtime env override as the loader saw it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum RecordedOverride {
    Set(String),
    Absent,
    /// The bytes were not Unicode. They are not kept; every consumer treats
    /// this state as unusable regardless of the bytes.
    NotUnicode,
}

/// Credential availability for one provider key account. No key material.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedCredential {
    pub api_key_present: bool,
    pub signed_in: bool,
}

/// Every field of [`CapturedRuntimeInputs`], with secrets reduced to presence.
#[derive(Clone, Serialize, Deserialize)]
pub struct CapturedInputsRecord {
    pub values: Config,
    #[serde(serialize_with = "exact_user_settings")]
    pub user_settings: UserSettings,
    pub settings_path: PathBuf,
    /// The settings file as read. It is JSON written by the app, so text keeps
    /// its bytes and their digest exact.
    pub settings_text: Option<String>,
    pub env_overlay_keys: Vec<String>,
    pub loaded_at_unix_ms: u64,
    pub energy_calibration_path: PathBuf,
    pub energy_calibration: SealedEnergyCalibration,
    pub overrides: BTreeMap<String, RecordedOverride>,
    pub credentials: BTreeMap<String, RecordedCredential>,
    pub prompts: CapturedRuntimePrompts,
    pub repair_receipt: RepairReceipt,
}

impl CapturedInputsRecord {
    /// Record a capture. The destructuring is exhaustive on purpose: a new
    /// captured field does not compile until it is recorded or redacted here.
    pub fn redacted(input: &CapturedRuntimeInputs) -> Self {
        let CapturedRuntimeInputs {
            values,
            user_settings,
            settings_path,
            settings_bytes,
            env_overlay_keys,
            loaded_at_unix_ms,
            energy_calibration_path,
            energy_calibration,
            overrides,
            credentials,
            prompts,
            repair_receipt,
        } = input;
        let mut values = values.clone();
        for key in [&mut values.stt_file_api_key, &mut values.stt_live_api_key] {
            *key = key.as_ref().map(|_| REDACTED_SECRET.to_string());
        }
        let overrides = overrides
            .iter()
            .map(|(key, value)| {
                let value = match value {
                    Ok(value) => RecordedOverride::Set(value.clone()),
                    Err(VarError::NotPresent) => RecordedOverride::Absent,
                    Err(VarError::NotUnicode(_)) => RecordedOverride::NotUnicode,
                };
                (key.clone(), value)
            })
            .collect();
        let credentials = credentials
            .iter()
            .map(|(account, credential)| {
                (
                    account.clone(),
                    RecordedCredential {
                        api_key_present: credential.api_key.is_some(),
                        signed_in: credential.signed_in,
                    },
                )
            })
            .collect();
        Self {
            values,
            user_settings: user_settings.clone(),
            settings_path: settings_path.clone(),
            settings_text: settings_bytes
                .as_deref()
                .and_then(|bytes| String::from_utf8(bytes.to_vec()).ok()),
            env_overlay_keys: env_overlay_keys.clone(),
            loaded_at_unix_ms: *loaded_at_unix_ms,
            energy_calibration_path: energy_calibration_path.clone(),
            energy_calibration: energy_calibration.clone(),
            overrides,
            credentials,
            prompts: prompts.clone(),
            repair_receipt: repair_receipt.clone(),
        }
    }

    /// Rebuild captured inputs for the production resolver. Present secrets
    /// come back as [`REDACTED_SECRET`]: presence, and so the digest, match the
    /// take, while no lane can authenticate with a key this record never held.
    pub fn into_captured(self) -> CapturedRuntimeInputs {
        let overrides: HashMap<String, Result<String, VarError>> = self
            .overrides
            .into_iter()
            .map(|(key, value)| {
                let value = match value {
                    RecordedOverride::Set(value) => Ok(value),
                    RecordedOverride::Absent => Err(VarError::NotPresent),
                    RecordedOverride::NotUnicode => Err(VarError::NotUnicode(OsString::new())),
                };
                (key, value)
            })
            .collect();
        let credentials = self
            .credentials
            .into_iter()
            .map(|(account, credential)| {
                (
                    account,
                    CapturedLaneCredential {
                        api_key: credential
                            .api_key_present
                            .then(|| REDACTED_SECRET.to_string()),
                        signed_in: credential.signed_in,
                    },
                )
            })
            .collect();
        CapturedRuntimeInputs {
            values: self.values,
            user_settings: self.user_settings,
            settings_path: self.settings_path,
            settings_bytes: self.settings_text.map(String::into_bytes),
            env_overlay_keys: self.env_overlay_keys,
            loaded_at_unix_ms: self.loaded_at_unix_ms,
            energy_calibration_path: self.energy_calibration_path,
            energy_calibration: self.energy_calibration,
            overrides,
            credentials,
            prompts: self.prompts,
            repair_receipt: self.repair_receipt,
        }
    }
}

/// `UserSettings` omits `None` fields, and some of them read a missing key
/// back as a migration default (`seal_lane_armed` returns as `Some(true)`).
/// Wherever reading the written form back would add a key, the record writes
/// an explicit `null` there, so the settings, and the digest, rebuild exactly.
fn exact_user_settings<S: serde::Serializer>(
    settings: &UserSettings,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    use serde::ser::Error;
    let mut written = serde_json::to_value(settings).map_err(S::Error::custom)?;
    let read_back: UserSettings =
        serde_json::from_value(written.clone()).map_err(S::Error::custom)?;
    let read_back = serde_json::to_value(&read_back).map_err(S::Error::custom)?;
    pin_omitted_keys(&mut written, &read_back);
    written.serialize(serializer)
}

fn pin_omitted_keys(written: &mut serde_json::Value, read_back: &serde_json::Value) {
    let (serde_json::Value::Object(written), serde_json::Value::Object(read_back)) =
        (written, read_back)
    else {
        return;
    };
    for (key, value) in read_back {
        match written.get_mut(key) {
            Some(kept) => pin_omitted_keys(kept, value),
            None => {
                written.insert(key.clone(), serde_json::Value::Null);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn captured_with_secrets() -> CapturedRuntimeInputs {
        let mut input =
            CapturedRuntimeInputs::defaults_at(PathBuf::from("/workspace/codescribe-data"), 42);
        input.values.stt_live_api_key = Some("sk-live-secret".into());
        input.values.stt_file_api_key = None;
        input.settings_bytes = Some(br#"{"whisper_language":"pl"}"#.to_vec());
        input
            .overrides
            .insert("FORMATTING_LEVEL".into(), Ok("smart".into()));
        input
            .overrides
            .insert("STT_TAIL_PROVIDER".into(), Err(VarError::NotPresent));
        input.credentials.insert(
            "codescribe.openai".into(),
            CapturedLaneCredential {
                api_key: Some("sk-lane-secret".into()),
                signed_in: false,
            },
        );
        input
    }

    #[test]
    fn record_keeps_no_secret_value() {
        let record = CapturedInputsRecord::redacted(&captured_with_secrets());
        let json = serde_json::to_string(&record).expect("record serializes");
        assert!(!json.contains("sk-live-secret"));
        assert!(!json.contains("sk-lane-secret"));
        assert!(json.contains(REDACTED_SECRET));
        assert_eq!(
            record.credentials["codescribe.openai"],
            RecordedCredential {
                api_key_present: true,
                signed_in: false,
            }
        );
    }

    #[test]
    fn rebuilt_record_reseals_the_same_digest() {
        let captured = captured_with_secrets();
        let live = Config::runtime_snapshot_from_captured(captured.clone());
        let record = CapturedInputsRecord::redacted(&captured);
        let json = serde_json::to_vec(&record).expect("record serializes");
        let restored: CapturedInputsRecord =
            serde_json::from_slice(&json).expect("record deserializes");
        let replayed = Config::runtime_snapshot_from_captured(restored.into_captured());
        assert_eq!(replayed.digest(), live.digest());
    }

    #[test]
    fn settings_left_unset_stay_unset_after_the_record() {
        let captured = captured_with_secrets();
        assert_eq!(captured.user_settings.seal_lane_armed, None);
        let json = serde_json::to_value(CapturedInputsRecord::redacted(&captured))
            .expect("record serializes");
        assert_eq!(
            json["user_settings"].get("seal_lane_armed"),
            Some(&serde_json::Value::Null)
        );
        let restored: CapturedInputsRecord =
            serde_json::from_value(json).expect("record deserializes");
        assert_eq!(restored.user_settings, captured.user_settings);
    }

    #[test]
    fn a_changed_setting_changes_the_rebuilt_digest() {
        let captured = captured_with_secrets();
        let live = Config::runtime_snapshot_from_captured(captured.clone());
        let mut record = CapturedInputsRecord::redacted(&captured);
        record.user_settings.seal_ledger_enabled = Some(false);
        let replayed = Config::runtime_snapshot_from_captured(record.into_captured());
        assert_ne!(replayed.digest(), live.digest());
    }
}
