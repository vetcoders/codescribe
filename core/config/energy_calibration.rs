//! Measured, versioned acoustic calibration — the single source of
//! [`EnergyCalibration`] for the runtime settings throne.
//!
//! Nothing here invents a threshold. A profile exists only after a guided
//! measurement on the operator's real capture path produced a
//! [`CaptureLevelReceipt`]; the existence floor is *derived* from the measured
//! active-speech level with the ITU-T P.56 method-B activity margin (15.9 dB
//! below the active-speech level, STL `sv-p56.c`). Floors are stored rate-free
//! (dBFS + milliseconds) and converted into the ledger's Σx² unit per session
//! at the actual capture rate, so one profile serves 16 kHz and 88.2 kHz alike.
//!
//! # Part contract
//! - **inputs:** `energy-calibration.json` beside `settings.json`; measured
//!   capture receipts from the product recorder path
//! - **outputs:** [`SealedEnergyCalibration`] frozen inside
//!   [`crate::config::RuntimeSettingsSnapshot`]; [`EnergyCalibration`] per
//!   capture path
//! - **forbidden authority:** no default profile, no permissive floor, no
//!   silent repair of a tampered or malformed file; a missing or refused
//!   artifact is an explicit fail-closed state the admission gate must name
//! - **consumers:** the core settings loader (one pass), `apple_stream_worker`
//!   (one read), the controller admission gate, Settings/overlay status rows

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::audio::capture_receipt::{CaptureLevelReceipt, db_to_linear};
use crate::pipeline::acoustic_ledger::EnergyCalibration;
use crate::vad::config::{SILERO_DEFAULT_MAX_SILENCE_SEC, SILERO_DEFAULT_MIN_SPEECH_SEC};

use super::settings::UserSettings;

/// On-disk schema tag. Bump on any incompatible shape change.
pub const ENERGY_CALIBRATION_SCHEMA: &str = "codescribe.energy-calibration.v1";
/// File name beside `settings.json`.
pub const ENERGY_CALIBRATION_FILE_NAME: &str = "energy-calibration.json";
/// ITU-T P.56 method B: the activity threshold sits this far below the
/// active-speech level (STL `sv-p56.c`, `M = 15.9 dB`).
pub const P56_ACTIVITY_MARGIN_DB: f32 = 15.9;
/// Named derivation rule stored in every profile so a later rule change is a
/// visible version, not a silent re-interpretation of old measurements.
pub const DERIVATION_RULE: &str = "itu-t-p56-method-b-activity-margin-v1";
/// The derived floor must sit at least this far above a measured (finite)
/// noise floor, otherwise ambient noise alone would qualify as existence.
pub const MIN_FLOOR_SEPARATION_DB: f32 = 6.0;
/// A measurement needs at least this much active speech to be a profile.
pub const MIN_MEASURED_SPEECH_SECONDS: f32 = 1.0;
/// Measurement source label for receipts produced by the guided calibration.
pub const SOURCE_GUIDED_CAPTURE: &str = "guided_capture";
/// Measurement source label for a synthetic test fixture (never production).
pub const SOURCE_SYNTHETIC_FIXTURE: &str = "synthetic_fixture";

/// Absolute path of the calibration artifact for this data dir.
pub fn energy_calibration_path() -> PathBuf {
    UserSettings::settings_dir().join(ENERGY_CALIBRATION_FILE_NAME)
}

/// Identity of the capture path a profile was measured on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalibrationCapturePath {
    pub device_name: String,
    pub sample_rate: u32,
    pub channels: u16,
}

/// What was measured (counts and levels only — never audio, never text).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalibrationMeasurement {
    pub source: String,
    pub sample_count: u64,
    pub active_speech_samples: u64,
    pub active_speech_median_dbfs: f32,
    /// `None` when the path gated silence to digital zero (macOS 27 behaviour).
    pub noise_floor_dbfs: Option<f32>,
    pub peak_dbfs: f32,
}

/// The rule and parameters that turned the measurement into floors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalibrationDerivation {
    pub rule: String,
    pub activity_margin_db: f32,
    /// Shortest region that may exist (Silero min-speech, milliseconds).
    pub min_region_ms: u32,
    /// Shortest valley that separates two regions (Silero max-silence, ms).
    pub min_valley_ms: u32,
}

/// Rate-free floors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalibrationFloors {
    /// Mean RMS a region must reach over `min_region_ms` to exist.
    pub existence_threshold_dbfs: f32,
}

/// One measured device profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnergyCalibrationProfile {
    /// Human-legible label carried into every acoustic serial (`@<rate>hz` is
    /// appended per session).
    pub version: String,
    pub measured_at_unix_ms: u64,
    pub capture_path: CalibrationCapturePath,
    pub measurement: CalibrationMeasurement,
    pub derivation: CalibrationDerivation,
    pub floors: CalibrationFloors,
}

/// The persisted artifact: every measured profile plus an integrity digest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnergyCalibrationArtifact {
    pub schema: String,
    pub updated_at_unix_ms: u64,
    pub profiles: Vec<EnergyCalibrationProfile>,
    /// SHA-256 of the canonical JSON with this field empty.
    pub digest: String,
}

/// Why a calibration could not be derived, loaded, or served.
#[derive(Debug, Clone, PartialEq)]
pub enum EnergyCalibrationRefusal {
    /// The measurement contained no active speech at all.
    NoActiveSpeech,
    /// Too little speech was measured to trust the median.
    InsufficientSpeech {
        measured_seconds: f32,
        required_seconds: f32,
    },
    /// The derived floor sits inside the measured noise.
    InsufficientSeparation {
        threshold_dbfs: f32,
        noise_floor_dbfs: f32,
    },
    /// A measured figure was not finite.
    NonFinite(&'static str),
    /// The capture path reported a zero sample rate.
    InvalidSampleRate(u32),
    /// The artifact file exists but could not be read.
    Unreadable { path: PathBuf, reason: String },
    /// The artifact file is not the documented JSON shape.
    Malformed { path: PathBuf, reason: String },
    /// The artifact carries a schema this build does not understand.
    UnknownSchema { path: PathBuf, schema: String },
    /// The artifact bytes do not match their own digest.
    DigestMismatch { path: PathBuf },
    /// A stored profile fails the documented validation.
    InvalidProfile {
        path: PathBuf,
        device_name: String,
        reason: String,
    },
    /// No artifact exists yet — the operator has not measured.
    Missing { path: PathBuf },
    /// The artifact is valid but has no profile for the current device.
    NoProfileForDevice {
        device_name: String,
        known_devices: Vec<String>,
    },
}

impl fmt::Display for EnergyCalibrationRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoActiveSpeech => write!(f, "no active speech was measured"),
            Self::InsufficientSpeech {
                measured_seconds,
                required_seconds,
            } => write!(
                f,
                "only {measured_seconds:.1}s of active speech measured; at least {required_seconds:.1}s required"
            ),
            Self::InsufficientSeparation {
                threshold_dbfs,
                noise_floor_dbfs,
            } => write!(
                f,
                "derived floor {threshold_dbfs:.1} dBFS sits within {MIN_FLOOR_SEPARATION_DB:.0} dB of the noise floor {noise_floor_dbfs:.1} dBFS"
            ),
            Self::NonFinite(field) => write!(f, "measured `{field}` is not finite"),
            Self::InvalidSampleRate(rate) => write!(f, "invalid capture sample rate {rate}"),
            Self::Unreadable { path, reason } => {
                write!(f, "cannot read {}: {reason}", path.display())
            }
            Self::Malformed { path, reason } => {
                write!(f, "malformed calibration {}: {reason}", path.display())
            }
            Self::UnknownSchema { path, schema } => write!(
                f,
                "unknown calibration schema `{schema}` in {}",
                path.display()
            ),
            Self::DigestMismatch { path } => {
                write!(f, "calibration digest mismatch in {}", path.display())
            }
            Self::InvalidProfile {
                path,
                device_name,
                reason,
            } => write!(
                f,
                "invalid profile for `{device_name}` in {}: {reason}",
                path.display()
            ),
            Self::Missing { path } => {
                write!(f, "no acoustic calibration at {}", path.display())
            }
            Self::NoProfileForDevice {
                device_name,
                known_devices,
            } => write!(
                f,
                "no calibration profile for `{device_name}` (measured: {})",
                if known_devices.is_empty() {
                    "none".to_string()
                } else {
                    known_devices.join(", ")
                }
            ),
        }
    }
}

impl std::error::Error for EnergyCalibrationRefusal {}

/// Loader verdict frozen into the snapshot. Consumers display it; they may not
/// upgrade `Missing`/`Refused` into a floor of their own.
#[derive(Debug, Clone, PartialEq)]
pub enum EnergyCalibrationStatus {
    Sealed {
        path: PathBuf,
        sha256: String,
        devices: Vec<String>,
    },
    Missing {
        path: PathBuf,
    },
    Refused {
        path: PathBuf,
        reason: String,
    },
}

impl EnergyCalibrationStatus {
    /// Stable machine code for UI/tests.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Sealed { .. } => "sealed",
            Self::Missing { .. } => "missing",
            Self::Refused { .. } => "refused",
        }
    }

    /// The artifact path this status describes.
    pub fn path(&self) -> &Path {
        match self {
            Self::Sealed { path, .. } | Self::Missing { path } | Self::Refused { path, .. } => path,
        }
    }
}

fn slugify(name: &str) -> String {
    let mut slug = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
        if slug.len() >= 24 {
            break;
        }
    }
    let slug = slug.trim_end_matches('-').to_string();
    if slug.is_empty() {
        "device".to_string()
    } else {
        slug
    }
}

fn ms_from_secs(secs: f32) -> u32 {
    (secs * 1000.0).round().max(1.0) as u32
}

impl EnergyCalibrationProfile {
    /// Derive a profile from one measured capture receipt. Refuses rather than
    /// guessing when the measurement cannot carry a floor.
    pub fn derive(
        receipt: &CaptureLevelReceipt,
        measured_at_unix_ms: u64,
        source: &str,
    ) -> Result<Self, EnergyCalibrationRefusal> {
        if receipt.sample_rate == 0 {
            return Err(EnergyCalibrationRefusal::InvalidSampleRate(
                receipt.sample_rate,
            ));
        }
        if !receipt.active_speech_median_dbfs_is_measured() {
            return Err(EnergyCalibrationRefusal::NoActiveSpeech);
        }
        if !receipt.peak_db.is_finite() {
            return Err(EnergyCalibrationRefusal::NonFinite("peak_db"));
        }
        let measured_seconds = receipt.active_speech_samples as f32 / receipt.sample_rate as f32;
        if measured_seconds < MIN_MEASURED_SPEECH_SECONDS {
            return Err(EnergyCalibrationRefusal::InsufficientSpeech {
                measured_seconds,
                required_seconds: MIN_MEASURED_SPEECH_SECONDS,
            });
        }
        let threshold = receipt.active_speech_median_db - P56_ACTIVITY_MARGIN_DB;
        let noise_floor = receipt
            .noise_floor_db
            .is_finite()
            .then_some(receipt.noise_floor_db);
        if let Some(noise) = noise_floor
            && threshold < noise + MIN_FLOOR_SEPARATION_DB
        {
            return Err(EnergyCalibrationRefusal::InsufficientSeparation {
                threshold_dbfs: threshold,
                noise_floor_dbfs: noise,
            });
        }
        let version = format!(
            "cal1-{}-{measured_at_unix_ms}",
            slugify(&receipt.device_name)
        );
        Ok(Self {
            version,
            measured_at_unix_ms,
            capture_path: CalibrationCapturePath {
                device_name: receipt.device_name.clone(),
                sample_rate: receipt.sample_rate,
                channels: receipt.channels,
            },
            measurement: CalibrationMeasurement {
                source: source.to_string(),
                sample_count: receipt.sample_count,
                active_speech_samples: receipt.active_speech_samples,
                active_speech_median_dbfs: receipt.active_speech_median_db,
                noise_floor_dbfs: noise_floor,
                peak_dbfs: receipt.peak_db,
            },
            derivation: CalibrationDerivation {
                rule: DERIVATION_RULE.to_string(),
                activity_margin_db: P56_ACTIVITY_MARGIN_DB,
                min_region_ms: ms_from_secs(SILERO_DEFAULT_MIN_SPEECH_SEC),
                min_valley_ms: ms_from_secs(SILERO_DEFAULT_MAX_SILENCE_SEC),
            },
            floors: CalibrationFloors {
                existence_threshold_dbfs: threshold,
            },
        })
    }

    /// Structural validation of a stored profile (no re-derivation).
    pub fn validate(&self) -> Result<(), String> {
        if self.version.trim().is_empty() {
            return Err("empty version".into());
        }
        if self.capture_path.device_name.trim().is_empty() {
            return Err("empty device name".into());
        }
        if self.capture_path.sample_rate == 0 {
            return Err("zero sample rate".into());
        }
        let t = self.floors.existence_threshold_dbfs;
        let s = self.measurement.active_speech_median_dbfs;
        if !t.is_finite() || !s.is_finite() {
            return Err("non-finite level".into());
        }
        if t >= s {
            return Err("existence floor is not below the measured speech level".into());
        }
        if let Some(noise) = self.measurement.noise_floor_dbfs
            && (!noise.is_finite() || t < noise + MIN_FLOOR_SEPARATION_DB)
        {
            return Err("existence floor sits within the noise floor".into());
        }
        if self.derivation.min_region_ms == 0 || self.derivation.min_valley_ms == 0 {
            return Err("zero region/valley duration".into());
        }
        Ok(())
    }

    /// The ledger calibration for one session at its actual capture rate.
    /// `min_energy_integral` is Σx² over the shortest region that may exist.
    pub fn ledger_calibration(
        &self,
        sample_rate: u32,
    ) -> Result<EnergyCalibration, EnergyCalibrationRefusal> {
        if sample_rate == 0 {
            return Err(EnergyCalibrationRefusal::InvalidSampleRate(sample_rate));
        }
        let rate = f64::from(sample_rate);
        let region_samples = (f64::from(self.derivation.min_region_ms) * rate / 1000.0)
            .ceil()
            .max(1.0);
        let valley_samples = (f64::from(self.derivation.min_valley_ms) * rate / 1000.0)
            .ceil()
            .max(1.0) as u64;
        let linear = f64::from(db_to_linear(self.floors.existence_threshold_dbfs));
        Ok(EnergyCalibration::new(
            format!("{}@{sample_rate}hz", self.version),
            linear * linear * region_samples,
            valley_samples,
        ))
    }
}

impl EnergyCalibrationArtifact {
    /// A fresh, empty artifact (no profiles — still fail-closed).
    pub fn empty(updated_at_unix_ms: u64) -> Self {
        Self {
            schema: ENERGY_CALIBRATION_SCHEMA.to_string(),
            updated_at_unix_ms,
            profiles: Vec::new(),
            digest: String::new(),
        }
    }

    fn canonical_material(&self) -> String {
        let mut copy = self.clone();
        copy.digest.clear();
        serde_json::to_string(&copy).unwrap_or_default()
    }

    /// Recompute and store the digest.
    pub fn seal(&mut self) -> &str {
        let material = self.canonical_material();
        let mut hasher = Sha256::new();
        hasher.update(material.as_bytes());
        self.digest = format!("{:x}", hasher.finalize());
        &self.digest
    }

    fn digest_matches(&self) -> bool {
        let mut copy = self.clone();
        copy.seal();
        copy.digest == self.digest
    }

    /// Replace the profile for the same device or append a new one.
    pub fn upsert_profile(&mut self, profile: EnergyCalibrationProfile, updated_at_unix_ms: u64) {
        self.profiles
            .retain(|p| p.capture_path.device_name != profile.capture_path.device_name);
        self.profiles.push(profile);
        self.profiles
            .sort_by(|a, b| a.capture_path.device_name.cmp(&b.capture_path.device_name));
        self.updated_at_unix_ms = updated_at_unix_ms;
        self.seal();
    }

    /// Profile measured on `device_name`, if any.
    pub fn profile_for_device(&self, device_name: &str) -> Option<&EnergyCalibrationProfile> {
        self.profiles
            .iter()
            .find(|p| p.capture_path.device_name == device_name)
    }

    /// Names of every measured device.
    pub fn device_names(&self) -> Vec<String> {
        self.profiles
            .iter()
            .map(|p| p.capture_path.device_name.clone())
            .collect()
    }

    /// Parse and validate artifact bytes. Refuses on schema, digest, or
    /// profile validation failure — never repairs.
    pub fn from_bytes(bytes: &[u8], path: &Path) -> Result<Self, EnergyCalibrationRefusal> {
        let artifact: Self =
            serde_json::from_slice(bytes).map_err(|error| EnergyCalibrationRefusal::Malformed {
                path: path.to_path_buf(),
                reason: error.to_string(),
            })?;
        if artifact.schema != ENERGY_CALIBRATION_SCHEMA {
            return Err(EnergyCalibrationRefusal::UnknownSchema {
                path: path.to_path_buf(),
                schema: artifact.schema,
            });
        }
        if !artifact.digest_matches() {
            return Err(EnergyCalibrationRefusal::DigestMismatch {
                path: path.to_path_buf(),
            });
        }
        let mut seen = std::collections::HashSet::new();
        for profile in &artifact.profiles {
            profile
                .validate()
                .map_err(|reason| EnergyCalibrationRefusal::InvalidProfile {
                    path: path.to_path_buf(),
                    device_name: profile.capture_path.device_name.clone(),
                    reason,
                })?;
            if !seen.insert(profile.capture_path.device_name.clone()) {
                return Err(EnergyCalibrationRefusal::InvalidProfile {
                    path: path.to_path_buf(),
                    device_name: profile.capture_path.device_name.clone(),
                    reason: "duplicate device profile".into(),
                });
            }
        }
        Ok(artifact)
    }

    /// Load from disk. `Ok(None)` means no artifact exists yet.
    pub fn load(path: &Path) -> Result<Option<Self>, EnergyCalibrationRefusal> {
        match fs::read(path) {
            Ok(bytes) => Self::from_bytes(&bytes, path).map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(EnergyCalibrationRefusal::Unreadable {
                path: path.to_path_buf(),
                reason: error.to_string(),
            }),
        }
    }

    /// Atomic write (temp file + rename) of the sealed artifact.
    pub fn save(&mut self, path: &Path) -> std::io::Result<()> {
        self.seal();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(self)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, path)
    }

    /// Load-or-create, upsert one measured profile, and persist. Returns the
    /// sealed artifact so the caller can report what is now on disk.
    pub fn record_profile(
        path: &Path,
        profile: EnergyCalibrationProfile,
        updated_at_unix_ms: u64,
    ) -> Result<Self, EnergyCalibrationRefusal> {
        let mut artifact = match Self::load(path) {
            Ok(Some(existing)) => existing,
            Ok(None) => Self::empty(updated_at_unix_ms),
            Err(refusal @ EnergyCalibrationRefusal::Unreadable { .. }) => return Err(refusal),
            // A malformed/tampered artifact must not trap the operator in a
            // state they cannot calibrate out of: the fresh measurement is the
            // newer truth and replaces it. The refusal is logged, not hidden.
            Err(refusal) => {
                tracing::warn!(
                    path = %path.display(),
                    %refusal,
                    "replacing refused acoustic calibration artifact with a fresh measurement"
                );
                Self::empty(updated_at_unix_ms)
            }
        };
        artifact.upsert_profile(profile, updated_at_unix_ms);
        artifact
            .save(path)
            .map_err(|error| EnergyCalibrationRefusal::Unreadable {
                path: path.to_path_buf(),
                reason: error.to_string(),
            })?;
        Ok(artifact)
    }
}

/// Calibration truth frozen by the settings loader into one snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct SealedEnergyCalibration {
    status: EnergyCalibrationStatus,
    artifact: Option<EnergyCalibrationArtifact>,
}

impl SealedEnergyCalibration {
    /// Read the artifact once. Never fails: absence and refusal are encoded as
    /// explicit states so the snapshot still seals and admission can name them.
    pub fn load(path: &Path) -> Self {
        match EnergyCalibrationArtifact::load(path) {
            Ok(Some(artifact)) => Self {
                status: EnergyCalibrationStatus::Sealed {
                    path: path.to_path_buf(),
                    sha256: artifact.digest.clone(),
                    devices: artifact.device_names(),
                },
                artifact: Some(artifact),
            },
            Ok(None) => Self {
                status: EnergyCalibrationStatus::Missing {
                    path: path.to_path_buf(),
                },
                artifact: None,
            },
            Err(refusal) => Self {
                status: EnergyCalibrationStatus::Refused {
                    path: path.to_path_buf(),
                    reason: refusal.to_string(),
                },
                artifact: None,
            },
        }
    }

    /// Wrap an already-validated artifact (tests and offline harnesses).
    pub fn sealed(path: PathBuf, artifact: EnergyCalibrationArtifact) -> Self {
        Self {
            status: EnergyCalibrationStatus::Sealed {
                path,
                sha256: artifact.digest.clone(),
                devices: artifact.device_names(),
            },
            artifact: Some(artifact),
        }
    }

    pub fn status(&self) -> &EnergyCalibrationStatus {
        &self.status
    }

    pub fn artifact(&self) -> Option<&EnergyCalibrationArtifact> {
        self.artifact.as_ref()
    }

    /// Digest of the sealed artifact, when one sealed.
    pub fn sha256(&self) -> Option<&str> {
        match &self.status {
            EnergyCalibrationStatus::Sealed { sha256, .. } => Some(sha256),
            _ => None,
        }
    }

    /// Non-secret material folded into the snapshot digest.
    pub fn digest_material(&self) -> String {
        match &self.status {
            EnergyCalibrationStatus::Sealed { sha256, .. } => {
                format!("energy_calibration=sealed:{sha256}")
            }
            EnergyCalibrationStatus::Missing { .. } => "energy_calibration=missing".to_string(),
            EnergyCalibrationStatus::Refused { reason, .. } => {
                format!("energy_calibration=refused:{reason}")
            }
        }
    }

    /// The ledger calibration for one capture path, or the precise refusal.
    pub fn for_capture(
        &self,
        device_name: &str,
        sample_rate: u32,
    ) -> Result<EnergyCalibration, EnergyCalibrationRefusal> {
        let artifact = match (&self.status, &self.artifact) {
            (EnergyCalibrationStatus::Sealed { .. }, Some(artifact)) => artifact,
            (EnergyCalibrationStatus::Missing { path }, _) => {
                return Err(EnergyCalibrationRefusal::Missing { path: path.clone() });
            }
            (EnergyCalibrationStatus::Refused { path, reason }, _) => {
                return Err(EnergyCalibrationRefusal::Malformed {
                    path: path.clone(),
                    reason: reason.clone(),
                });
            }
            (EnergyCalibrationStatus::Sealed { path, .. }, None) => {
                return Err(EnergyCalibrationRefusal::Missing { path: path.clone() });
            }
        };
        let profile = artifact.profile_for_device(device_name).ok_or_else(|| {
            EnergyCalibrationRefusal::NoProfileForDevice {
                device_name: device_name.to_string(),
                known_devices: artifact.device_names(),
            }
        })?;
        profile.ledger_calibration(sample_rate)
    }
}

impl CaptureLevelReceipt {
    /// Whether the receipt measured any active speech (finite median).
    pub fn active_speech_median_dbfs_is_measured(&self) -> bool {
        self.active_speech_median_db.is_finite()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::capture_receipt::CAPTURE_LEVEL_RECEIPT_CODE;
    use crate::pipeline::acoustic_ledger::{
        AcousticEvidence, AcousticLedger, AdmissionReceipt, AdmissionRefusal, OccurrenceIdentity,
    };
    use tempfile::TempDir;

    /// Operator-shaped receipt: MacBook Pro Microphone @ 88.2 kHz, speech
    /// −38.4 dBFS, silence gated to digital zero (noise floor −∞).
    fn operator_receipt() -> CaptureLevelReceipt {
        CaptureLevelReceipt {
            code: CAPTURE_LEVEL_RECEIPT_CODE,
            device_name: "MacBook Pro Microphone".into(),
            sample_rate: 88_200,
            channels: 1,
            sample_count: 882_000,
            active_speech_samples: 441_000,
            digital_zero_samples: 441_000,
            clipping_samples: 0,
            dropout_blocks: 0,
            all_audio_median_db: -60.0,
            active_speech_median_db: -38.4,
            peak_db: -12.5,
            noise_floor_db: f32::NEG_INFINITY,
            snr_db: None,
            threshold_db: -52.0,
            low: false,
        }
    }

    fn fixture_receipt_16k(speech_dbfs: f32) -> CaptureLevelReceipt {
        CaptureLevelReceipt {
            device_name: "fixture".into(),
            sample_rate: 16_000,
            sample_count: 160_000,
            active_speech_samples: 32_000,
            digital_zero_samples: 0,
            active_speech_median_db: speech_dbfs,
            noise_floor_db: -80.0,
            ..operator_receipt()
        }
    }

    #[test]
    fn derive_applies_p56_margin_to_measured_speech() {
        let profile =
            EnergyCalibrationProfile::derive(&operator_receipt(), 1_000, SOURCE_GUIDED_CAPTURE)
                .expect("operator receipt derives");
        assert!((profile.floors.existence_threshold_dbfs - (-54.3)).abs() < 1e-3);
        assert_eq!(profile.derivation.rule, DERIVATION_RULE);
        assert_eq!(profile.derivation.min_region_ms, 64);
        assert_eq!(profile.derivation.min_valley_ms, 300);
        assert_eq!(profile.measurement.noise_floor_dbfs, None);
        assert_eq!(profile.version, "cal1-macbook-pro-microphone-1000");
        profile.validate().expect("derived profile validates");
    }

    #[test]
    fn derive_refuses_silence_and_thin_measurements() {
        let mut silent = operator_receipt();
        silent.active_speech_median_db = f32::NEG_INFINITY;
        assert_eq!(
            EnergyCalibrationProfile::derive(&silent, 1, SOURCE_GUIDED_CAPTURE).unwrap_err(),
            EnergyCalibrationRefusal::NoActiveSpeech
        );
        let mut thin = operator_receipt();
        thin.active_speech_samples = 8_820; // 0.1 s
        assert!(matches!(
            EnergyCalibrationProfile::derive(&thin, 1, SOURCE_GUIDED_CAPTURE).unwrap_err(),
            EnergyCalibrationRefusal::InsufficientSpeech { .. }
        ));
        let mut noisy = fixture_receipt_16k(-40.0);
        noisy.noise_floor_db = -52.0; // floor −55.9 is below noise + 6
        assert!(matches!(
            EnergyCalibrationProfile::derive(&noisy, 1, SOURCE_GUIDED_CAPTURE).unwrap_err(),
            EnergyCalibrationRefusal::InsufficientSeparation { .. }
        ));
    }

    #[test]
    fn ledger_calibration_scales_linearly_with_capture_rate() {
        let profile =
            EnergyCalibrationProfile::derive(&operator_receipt(), 7, SOURCE_GUIDED_CAPTURE)
                .unwrap();
        let at_16k = profile.ledger_calibration(16_000).unwrap();
        let at_88k = profile.ledger_calibration(88_200).unwrap();
        assert_eq!(at_16k.version, "cal1-macbook-pro-microphone-7@16000hz");
        assert_eq!(at_88k.version, "cal1-macbook-pro-microphone-7@88200hz");
        let ratio = at_88k.min_energy_integral / at_16k.min_energy_integral;
        assert!((ratio - 88_200.0 / 16_000.0).abs() < 0.01, "ratio={ratio}");
        assert_eq!(at_16k.min_valley_samples, 4_800);
        assert_eq!(at_88k.min_valley_samples, 26_460);
        assert!(profile.ledger_calibration(0).is_err());
    }

    #[test]
    fn artifact_round_trips_and_refuses_tamper() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join(ENERGY_CALIBRATION_FILE_NAME);
        let profile =
            EnergyCalibrationProfile::derive(&operator_receipt(), 1, SOURCE_GUIDED_CAPTURE)
                .unwrap();
        let saved = EnergyCalibrationArtifact::record_profile(&path, profile.clone(), 2).unwrap();
        assert_eq!(saved.profiles.len(), 1);
        let loaded = EnergyCalibrationArtifact::load(&path)
            .unwrap()
            .expect("exists");
        assert_eq!(loaded, saved);

        // Re-recording the same device replaces, never duplicates.
        let again = EnergyCalibrationArtifact::record_profile(&path, profile, 3).unwrap();
        assert_eq!(again.profiles.len(), 1);
        assert_eq!(again.updated_at_unix_ms, 3);

        // Tamper: nudge the floor without resealing.
        let text = fs::read_to_string(&path).unwrap();
        let tampered = text.replace(
            "\"existence_threshold_dbfs\": -54.3",
            "\"existence_threshold_dbfs\": -90.0",
        );
        assert_ne!(text, tampered, "fixture must contain the derived floor");
        fs::write(&path, tampered).unwrap();
        assert!(matches!(
            EnergyCalibrationArtifact::load(&path).unwrap_err(),
            EnergyCalibrationRefusal::DigestMismatch { .. }
        ));
        fs::write(&path, b"{not json").unwrap();
        assert!(matches!(
            EnergyCalibrationArtifact::load(&path).unwrap_err(),
            EnergyCalibrationRefusal::Malformed { .. }
        ));
    }

    #[test]
    fn sealed_calibration_names_missing_refused_and_unknown_device() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join(ENERGY_CALIBRATION_FILE_NAME);

        let missing = SealedEnergyCalibration::load(&path);
        assert_eq!(missing.status().code(), "missing");
        assert!(matches!(
            missing
                .for_capture("MacBook Pro Microphone", 88_200)
                .unwrap_err(),
            EnergyCalibrationRefusal::Missing { .. }
        ));

        fs::write(&path, b"{\"schema\":\"other\"}").unwrap();
        let refused = SealedEnergyCalibration::load(&path);
        assert_eq!(refused.status().code(), "refused");
        assert!(
            refused
                .for_capture("MacBook Pro Microphone", 88_200)
                .is_err()
        );

        let profile =
            EnergyCalibrationProfile::derive(&operator_receipt(), 1, SOURCE_GUIDED_CAPTURE)
                .unwrap();
        EnergyCalibrationArtifact::record_profile(&path, profile, 1).unwrap();
        let sealed = SealedEnergyCalibration::load(&path);
        assert_eq!(sealed.status().code(), "sealed");
        assert!(sealed.sha256().is_some());
        assert!(sealed.for_capture("MacBook Pro Microphone", 88_200).is_ok());
        match sealed.for_capture("AirPods Pro", 48_000).unwrap_err() {
            EnergyCalibrationRefusal::NoProfileForDevice { known_devices, .. } => {
                assert_eq!(known_devices, vec!["MacBook Pro Microphone".to_string()]);
            }
            other => panic!("unexpected refusal {other:?}"),
        }
        assert_ne!(missing.digest_material(), sealed.digest_material());
    }

    fn evidence(calibration: &EnergyCalibration, samples: &[f32], start: u64) -> AcousticEvidence {
        let end = start + samples.len() as u64;
        let energy: f64 = samples.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
        let rms = (energy / samples.len() as f64).sqrt();
        AcousticEvidence {
            occurrence: OccurrenceIdentity::new("proof", 1, start, end),
            duration_ms: samples.len() as f64 / 16.0,
            energy_integral: energy,
            mean_rms_dbfs: if rms > 0.0 {
                20.0 * rms.log10()
            } else {
                -200.0
            },
            peak_dbfs: -20.0,
            vad_open_sample: Some(start),
            vad_close_sample: Some(end),
            evidence_calibration_version: calibration.version.clone(),
        }
    }

    /// The proof the mission asks for: a five-Iwo-shaped qualified burst
    /// (Σx² ≈ 170.7 over 4000 samples @ 16 kHz, ≈ −13.7 dBFS) seals through
    /// the real `AcousticLedger::qualify`, while silence and −60 dBFS noise
    /// over the same width refuse with `BelowCalibratedEnergy`.
    #[test]
    fn qualified_pcm_seals_while_silence_and_noise_cannot() {
        let profile = EnergyCalibrationProfile::derive(
            &fixture_receipt_16k(-13.7),
            1,
            SOURCE_SYNTHETIC_FIXTURE,
        )
        .unwrap();
        let calibration = profile.ledger_calibration(16_000).unwrap();
        let mut ledger = AcousticLedger::new();

        let amplitude = (170.7_f64 / 4000.0).sqrt() as f32;
        let burst = vec![amplitude; 4000];
        match ledger.qualify(&evidence(&calibration, &burst, 0), &calibration) {
            AdmissionReceipt::Qualified { .. } => {}
            AdmissionReceipt::Refused { reason, .. } => panic!("burst refused: {reason:?}"),
        }

        let silence = vec![0.0_f32; 4000];
        assert!(matches!(
            ledger.qualify(&evidence(&calibration, &silence, 8_000), &calibration),
            AdmissionReceipt::Refused {
                reason: AdmissionRefusal::BelowCalibratedEnergy,
                ..
            }
        ));

        let noise = vec![db_to_linear(-60.0); 4000];
        assert!(matches!(
            ledger.qualify(&evidence(&calibration, &noise, 16_000), &calibration),
            AdmissionReceipt::Refused {
                reason: AdmissionRefusal::BelowCalibratedEnergy,
                ..
            }
        ));
    }
}
