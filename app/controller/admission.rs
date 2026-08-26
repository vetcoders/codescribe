//! Admission readiness — the precondition of beginning a product recording.
//!
//! A take whose occurrences can never qualify is a lie: the WAV grows, the
//! overlay pulses, and the Transcript Bus never leaves `session_started`.
//! Two independent authorities decide whether the acoustic ledger can seal
//! anything at all, and both are known before any microphone opens:
//!
//! 1. a **measured** `EnergyCalibration` profile for the device the take
//!    would open (`RuntimeSettingsSnapshot::energy_calibration_for_capture`);
//! 2. the **seal lane** — `seal_utterance_final` only lets Silero-bounded
//!    regions qualify (`may_qualify = silero_bound`), so the lane must be
//!    armed (`CODESCRIBE_SILERO_FUSION`) and the Silero graph must load.
//!
//! This module evaluates both and returns one precise, actionable blocker or
//! a grant. It opens no stream, invents no floor, and owns no state: the
//! controller calls it before the recorder lock, the bridge projects it for
//! Settings, and the overlay renders the blocker code as a notice.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use codescribe_core::audio::capture_receipt::CapturePathMeta;
use codescribe_core::config::energy_calibration::calibration_now_unix_ms;
use codescribe_core::config::{
    EnergyCalibrationRefusal, EnergyCalibrationStatus, RuntimeSettingsSnapshot,
};
use codescribe_core::pipeline::streaming::{SILERO_FUSION_ENV, SealLaneProbe};

/// Why the next product recording must not begin.
#[derive(Debug, Clone, PartialEq)]
pub enum AdmissionBlocker {
    /// No input device could be resolved (or the probe itself failed).
    CaptureDeviceUnavailable { reason: String },
    /// The operator has not measured this machine yet.
    CalibrationMissing { path: PathBuf },
    /// The artifact exists but the loader refused it (tamper, schema, shape).
    CalibrationRefused { path: PathBuf, reason: String },
    /// Measured profiles exist, but none for the device that would open.
    CalibrationNoProfileForDevice {
        device_name: String,
        known_devices: Vec<String>,
    },
    /// A profile exists but cannot serve this capture path.
    CalibrationUnusable { device_name: String, reason: String },
    /// The seal lane is not armed, so no region can ever qualify.
    SealLaneDisarmed { env: &'static str },
    /// The seal lane is armed but Silero did not load in this process.
    SealVadUnavailable,
}

impl AdmissionBlocker {
    /// Stable marker shared with the Swift overlay/Settings rewrite tables.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::CaptureDeviceUnavailable { .. } => "admission_capture_device_unavailable",
            Self::CalibrationMissing { .. } => "admission_calibration_missing",
            Self::CalibrationRefused { .. } => "admission_calibration_refused",
            Self::CalibrationNoProfileForDevice { .. } => "admission_calibration_no_profile",
            Self::CalibrationUnusable { .. } => "admission_calibration_unusable",
            Self::SealLaneDisarmed { .. } => "admission_seal_lane_disarmed",
            Self::SealVadUnavailable => "admission_seal_vad_unavailable",
        }
    }

    /// What the operator can do about it (one sentence, no jargon).
    pub fn action(&self) -> String {
        match self {
            Self::CaptureDeviceUnavailable { .. } => {
                "Connect a microphone and refresh Audio settings.".to_string()
            }
            Self::CalibrationMissing { .. } | Self::CalibrationNoProfileForDevice { .. } => {
                "Run Calibrate microphone in Settings › Audio (about 10 seconds of normal speech)."
                    .to_string()
            }
            Self::CalibrationRefused { .. } | Self::CalibrationUnusable { .. } => {
                "Re-run Calibrate microphone in Settings › Audio; the stored measurement cannot be used."
                    .to_string()
            }
            Self::SealLaneDisarmed { env } => format!(
                "Set {env}=1 in ~/.codescribe/.env (the Silero seal lane bounds every committed utterance)."
            ),
            Self::SealVadUnavailable => {
                "Silero VAD failed to load; check the app log and reinstall the build.".to_string()
            }
        }
    }

    /// Human-readable explanation of the blocker (no action).
    pub fn explanation(&self) -> String {
        match self {
            Self::CaptureDeviceUnavailable { reason } => format!("no input device: {reason}"),
            Self::CalibrationMissing { path } => {
                format!("no acoustic calibration measured yet ({})", path.display())
            }
            Self::CalibrationRefused { path, reason } => {
                format!(
                    "acoustic calibration refused ({}): {reason}",
                    path.display()
                )
            }
            Self::CalibrationNoProfileForDevice {
                device_name,
                known_devices,
            } => format!(
                "no calibration profile for `{device_name}` (measured: {})",
                if known_devices.is_empty() {
                    "none".to_string()
                } else {
                    known_devices.join(", ")
                }
            ),
            Self::CalibrationUnusable {
                device_name,
                reason,
            } => format!("calibration for `{device_name}` cannot serve this take: {reason}"),
            Self::SealLaneDisarmed { env } => {
                format!("seal lane disarmed ({env} is off), so no utterance can commit")
            }
            Self::SealVadUnavailable => "Silero VAD is not available in this process".to_string(),
        }
    }

    fn from_calibration_refusal(device_name: &str, refusal: EnergyCalibrationRefusal) -> Self {
        match refusal {
            EnergyCalibrationRefusal::Missing { path } => Self::CalibrationMissing { path },
            EnergyCalibrationRefusal::NoProfileForDevice {
                device_name,
                known_devices,
            } => Self::CalibrationNoProfileForDevice {
                device_name,
                known_devices,
            },
            EnergyCalibrationRefusal::Malformed { path, reason }
            | EnergyCalibrationRefusal::Unreadable { path, reason } => {
                Self::CalibrationRefused { path, reason }
            }
            EnergyCalibrationRefusal::UnknownSchema { path, schema } => Self::CalibrationRefused {
                path,
                reason: format!("unknown schema `{schema}`"),
            },
            EnergyCalibrationRefusal::DigestMismatch { path } => Self::CalibrationRefused {
                path,
                reason: "digest mismatch".to_string(),
            },
            EnergyCalibrationRefusal::InvalidProfile { path, reason, .. } => {
                Self::CalibrationRefused { path, reason }
            }
            other => Self::CalibrationUnusable {
                device_name: device_name.to_string(),
                reason: other.to_string(),
            },
        }
    }
}

impl fmt::Display for AdmissionBlocker {
    /// `<code>: <explanation> — <action>`; the code prefix is what the Swift
    /// notice table keys on, the rest is already user-readable.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {} — {}",
            self.code(),
            self.explanation(),
            self.action()
        )
    }
}

impl std::error::Error for AdmissionBlocker {}

/// The next recording may begin: what it will open and under which floor.
#[derive(Debug, Clone, PartialEq)]
pub struct AdmissionGrant {
    pub device_name: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub calibration_version: String,
}

/// Decide admission from already-probed facts. Pure, so every ordering and
/// blocker is testable without hardware. Order is the operator's order of
/// action: device → calibration → seal lane → Silero.
pub fn evaluate_admission_readiness(
    snapshot: &RuntimeSettingsSnapshot,
    capture: Result<CapturePathMeta, String>,
    seal_lane: SealLaneProbe,
) -> Result<AdmissionGrant, AdmissionBlocker> {
    let capture =
        capture.map_err(|reason| AdmissionBlocker::CaptureDeviceUnavailable { reason })?;
    let now_unix_ms = calibration_now_unix_ms().map_err(|refusal| {
        AdmissionBlocker::from_calibration_refusal(&capture.device_name, refusal)
    })?;
    evaluate_probed_admission_at(snapshot, capture, seal_lane, now_unix_ms)
}

/// Deterministic clock-injected admission used by focused tests and offline
/// falsifiers. Production calls [`evaluate_admission_readiness`].
pub fn evaluate_admission_readiness_at(
    snapshot: &RuntimeSettingsSnapshot,
    capture: Result<CapturePathMeta, String>,
    seal_lane: SealLaneProbe,
    now_unix_ms: u64,
) -> Result<AdmissionGrant, AdmissionBlocker> {
    let capture =
        capture.map_err(|reason| AdmissionBlocker::CaptureDeviceUnavailable { reason })?;
    evaluate_probed_admission_at(snapshot, capture, seal_lane, now_unix_ms)
}

fn evaluate_probed_admission_at(
    snapshot: &RuntimeSettingsSnapshot,
    capture: CapturePathMeta,
    seal_lane: SealLaneProbe,
    now_unix_ms: u64,
) -> Result<AdmissionGrant, AdmissionBlocker> {
    let calibration = snapshot
        .energy_calibration()
        .for_capture_at(&capture, now_unix_ms)
        .map_err(|refusal| {
            AdmissionBlocker::from_calibration_refusal(&capture.device_name, refusal)
        })?;
    if !seal_lane.armed {
        return Err(AdmissionBlocker::SealLaneDisarmed {
            env: SILERO_FUSION_ENV,
        });
    }
    if !seal_lane.vad_available {
        return Err(AdmissionBlocker::SealVadUnavailable);
    }
    Ok(AdmissionGrant {
        device_name: capture.device_name,
        sample_rate: capture.sample_rate,
        channels: capture.channels,
        calibration_version: calibration.version,
    })
}

/// Probe the live machine (device without opening a stream, Silero graph)
/// and decide. Blocking: call from `spawn_blocking` on the runtime.
pub fn evaluate_live_admission(
    snapshot: &RuntimeSettingsSnapshot,
) -> Result<AdmissionGrant, AdmissionBlocker> {
    let capture = codescribe_core::audio::recorder::probe_input_capture_path()
        .map_err(|error| format!("{error:#}"));
    let seal_lane = codescribe_core::pipeline::streaming::seal_lane_probe();
    evaluate_admission_readiness(snapshot, capture, seal_lane)
}

/// Snapshot-level calibration facts for status surfaces (no probing).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalibrationStatusView {
    pub code: &'static str,
    pub path: PathBuf,
    pub devices: Vec<String>,
    pub detail: Option<String>,
}

/// Project the loader's calibration verdict for Settings.
pub fn calibration_status_view(snapshot: &RuntimeSettingsSnapshot) -> CalibrationStatusView {
    match snapshot.energy_calibration_status() {
        EnergyCalibrationStatus::Sealed { path, devices, .. } => CalibrationStatusView {
            code: "sealed",
            path: path.clone(),
            devices: devices.clone(),
            detail: None,
        },
        EnergyCalibrationStatus::Missing { path } => CalibrationStatusView {
            code: "missing",
            path: path.clone(),
            devices: Vec::new(),
            detail: None,
        },
        EnergyCalibrationStatus::Refused { path, reason } => CalibrationStatusView {
            code: "refused",
            path: path.clone(),
            devices: Vec::new(),
            detail: Some(reason.clone()),
        },
    }
}

/// What a guided calibration measured and stored. Counts and levels only.
#[derive(Debug, Clone, PartialEq)]
pub struct EnergyCalibrationReport {
    pub device_name: String,
    pub sample_rate: u32,
    pub measured_seconds: f32,
    pub active_speech_median_dbfs: f32,
    pub noise_floor_dbfs: Option<f32>,
    pub peak_dbfs: f32,
    pub existence_threshold_dbfs: f32,
    pub version: String,
    pub path: PathBuf,
}

/// Convenience for callers holding the controller's `Arc` generation.
pub fn evaluate_live_admission_arc(
    snapshot: &Arc<RuntimeSettingsSnapshot>,
) -> Result<AdmissionGrant, AdmissionBlocker> {
    evaluate_live_admission(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use codescribe_core::audio::capture_receipt::{
        CAPTURE_LEVEL_RECEIPT_CODE, CaptureLevelReceipt,
    };
    use codescribe_core::config::Config;
    use codescribe_core::config::energy_calibration::{
        ENERGY_CALIBRATION_MAX_AGE_MS, EnergyCalibrationArtifact, EnergyCalibrationProfile,
        SOURCE_SYNTHETIC_FIXTURE, energy_calibration_path,
    };
    use serial_test::serial;
    use tempfile::TempDir;

    fn isolated_data_dir() -> TempDir {
        let tmp = TempDir::new().expect("tempdir");
        // SAFETY: tests are serial and intentionally override process env.
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", tmp.path());
        }
        tmp
    }

    fn fixture_receipt(device: &str) -> CaptureLevelReceipt {
        CaptureLevelReceipt {
            code: CAPTURE_LEVEL_RECEIPT_CODE,
            device_name: device.to_string(),
            sample_rate: 48_000,
            channels: 1,
            sample_count: 480_000,
            digital_zero_samples: 0,
            active_speech_samples: 240_000,
            clipping_samples: 0,
            dropout_blocks: 0,
            all_audio_median_db: -40.0,
            active_speech_median_db: -30.0,
            peak_db: -6.0,
            noise_floor_db: -80.0,
            snr_db: Some(50.0),
            threshold_db: -52.0,
            low: false,
        }
    }

    fn capture(device: &str) -> Result<CapturePathMeta, String> {
        capture_with(device, 48_000, 1)
    }

    fn capture_with(
        device: &str,
        sample_rate: u32,
        channels: u16,
    ) -> Result<CapturePathMeta, String> {
        Ok(CapturePathMeta {
            device_name: device.to_string(),
            sample_rate,
            channels,
        })
    }

    const ARMED: SealLaneProbe = SealLaneProbe {
        armed: true,
        vad_available: true,
    };

    fn snapshot() -> RuntimeSettingsSnapshot {
        Config::load_runtime_snapshot_without_keychain().expect("snapshot seals")
    }

    #[test]
    #[serial]
    fn device_failure_is_the_first_blocker() {
        let _tmp = isolated_data_dir();
        let blocker = evaluate_admission_readiness_at(&snapshot(), Err("no host".into()), ARMED, 7)
            .unwrap_err();
        assert_eq!(blocker.code(), "admission_capture_device_unavailable");
        assert!(
            blocker
                .to_string()
                .starts_with("admission_capture_device_unavailable: ")
        );
    }

    #[test]
    #[serial]
    fn missing_calibration_refuses_before_any_lane_question() {
        let _tmp = isolated_data_dir();
        let disarmed = SealLaneProbe {
            armed: false,
            vad_available: false,
        };
        let blocker =
            evaluate_admission_readiness_at(&snapshot(), capture("Fixture Mic"), disarmed, 7)
                .unwrap_err();
        assert_eq!(blocker.code(), "admission_calibration_missing");
        assert!(blocker.action().contains("Calibrate microphone"));
    }

    #[test]
    #[serial]
    fn calibrated_device_then_lane_then_vad_then_grant() {
        let _tmp = isolated_data_dir();
        let profile = EnergyCalibrationProfile::derive(
            &fixture_receipt("Fixture Mic"),
            7,
            SOURCE_SYNTHETIC_FIXTURE,
        )
        .unwrap();
        EnergyCalibrationArtifact::record_profile(&energy_calibration_path(), profile, 7).unwrap();
        let snap = snapshot();
        assert_eq!(calibration_status_view(&snap).code, "sealed");
        assert_eq!(
            calibration_status_view(&snap).devices,
            vec!["Fixture Mic".to_string()]
        );

        let other =
            evaluate_admission_readiness_at(&snap, capture("Other Mic"), ARMED, 7).unwrap_err();
        assert_eq!(other.code(), "admission_calibration_no_profile");
        assert!(other.explanation().contains("Fixture Mic"));

        let disarmed = evaluate_admission_readiness_at(
            &snap,
            capture("Fixture Mic"),
            SealLaneProbe {
                armed: false,
                vad_available: true,
            },
            7,
        )
        .unwrap_err();
        assert_eq!(disarmed.code(), "admission_seal_lane_disarmed");
        assert!(disarmed.action().contains(SILERO_FUSION_ENV));

        let no_vad = evaluate_admission_readiness_at(
            &snap,
            capture("Fixture Mic"),
            SealLaneProbe {
                armed: true,
                vad_available: false,
            },
            7,
        )
        .unwrap_err();
        assert_eq!(no_vad.code(), "admission_seal_vad_unavailable");

        let grant =
            evaluate_admission_readiness_at(&snap, capture("Fixture Mic"), ARMED, 7).unwrap();
        assert_eq!(grant.device_name, "Fixture Mic");
        assert_eq!(grant.sample_rate, 48_000);
        assert_eq!(grant.calibration_version, "cal2-fixture-mic-7@48000hz");
    }

    #[test]
    #[serial]
    fn refused_artifact_is_named_not_repaired() {
        let _tmp = isolated_data_dir();
        std::fs::create_dir_all(energy_calibration_path().parent().unwrap()).unwrap();
        std::fs::write(energy_calibration_path(), b"{\"schema\":\"nope\"}").unwrap();
        let snap = snapshot();
        assert_eq!(calibration_status_view(&snap).code, "refused");
        let blocker =
            evaluate_admission_readiness_at(&snap, capture("Fixture Mic"), ARMED, 7).unwrap_err();
        assert_eq!(blocker.code(), "admission_calibration_refused");
    }

    #[test]
    #[serial]
    fn calibration_is_admitted_at_the_exact_validity_boundary() {
        let _tmp = isolated_data_dir();
        let measured_at = 10_000;
        let profile = EnergyCalibrationProfile::derive(
            &fixture_receipt("Fixture Mic"),
            measured_at,
            SOURCE_SYNTHETIC_FIXTURE,
        )
        .unwrap();
        EnergyCalibrationArtifact::record_profile(&energy_calibration_path(), profile, measured_at)
            .unwrap();
        let grant = evaluate_admission_readiness_at(
            &snapshot(),
            capture("Fixture Mic"),
            ARMED,
            measured_at + ENERGY_CALIBRATION_MAX_AGE_MS,
        )
        .expect("exactly 30 days remains valid");
        assert_eq!(grant.device_name, "Fixture Mic");
    }

    #[test]
    #[serial]
    fn expired_calibration_refuses_with_recalibration_as_the_only_action() {
        let _tmp = isolated_data_dir();
        let measured_at = 10_000;
        let profile = EnergyCalibrationProfile::derive(
            &fixture_receipt("Fixture Mic"),
            measured_at,
            SOURCE_SYNTHETIC_FIXTURE,
        )
        .unwrap();
        EnergyCalibrationArtifact::record_profile(&energy_calibration_path(), profile, measured_at)
            .unwrap();
        let blocker = evaluate_admission_readiness_at(
            &snapshot(),
            capture("Fixture Mic"),
            ARMED,
            measured_at + ENERGY_CALIBRATION_MAX_AGE_MS + 1,
        )
        .unwrap_err();
        assert_eq!(blocker.code(), "admission_calibration_unusable");
        assert!(blocker.explanation().contains("expired"));
        assert!(blocker.action().starts_with("Re-run Calibrate microphone"));
    }

    #[test]
    #[serial]
    fn future_dated_calibration_refuses_on_clock_anomaly() {
        let _tmp = isolated_data_dir();
        let measured_at = 10_000;
        let profile = EnergyCalibrationProfile::derive(
            &fixture_receipt("Fixture Mic"),
            measured_at,
            SOURCE_SYNTHETIC_FIXTURE,
        )
        .unwrap();
        EnergyCalibrationArtifact::record_profile(&energy_calibration_path(), profile, measured_at)
            .unwrap();
        let blocker = evaluate_admission_readiness_at(
            &snapshot(),
            capture("Fixture Mic"),
            ARMED,
            measured_at - 1,
        )
        .unwrap_err();
        assert_eq!(blocker.code(), "admission_calibration_unusable");
        assert!(blocker.explanation().contains("in the future"));
    }

    #[test]
    #[serial]
    fn same_display_name_with_changed_capture_generation_refuses() {
        let _tmp = isolated_data_dir();
        let profile = EnergyCalibrationProfile::derive(
            &fixture_receipt("Fixture Mic"),
            10_000,
            SOURCE_SYNTHETIC_FIXTURE,
        )
        .unwrap();
        EnergyCalibrationArtifact::record_profile(&energy_calibration_path(), profile, 10_000)
            .unwrap();
        let blocker = evaluate_admission_readiness_at(
            &snapshot(),
            capture_with("Fixture Mic", 48_000, 2),
            ARMED,
            10_000,
        )
        .unwrap_err();
        assert_eq!(blocker.code(), "admission_calibration_unusable");
        assert!(blocker.explanation().contains("capture generation changed"));
    }

    #[test]
    #[serial]
    fn recalibration_generation_changes_the_loader_sealed_snapshot_digest() {
        let _tmp = isolated_data_dir();
        let first = EnergyCalibrationProfile::derive(
            &fixture_receipt("Fixture Mic"),
            10_000,
            SOURCE_SYNTHETIC_FIXTURE,
        )
        .unwrap();
        EnergyCalibrationArtifact::record_profile(&energy_calibration_path(), first, 10_000)
            .unwrap();
        let first_digest = snapshot().digest().as_str().to_string();

        let second = EnergyCalibrationProfile::derive(
            &fixture_receipt("Fixture Mic"),
            10_001,
            SOURCE_SYNTHETIC_FIXTURE,
        )
        .unwrap();
        EnergyCalibrationArtifact::record_profile(&energy_calibration_path(), second, 10_001)
            .unwrap();
        let second_digest = snapshot().digest().as_str().to_string();

        assert_ne!(first_digest, second_digest);
    }
}
