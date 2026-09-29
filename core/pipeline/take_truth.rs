//! Take truth sidecar — the `.truth.json` observer contract, schema v2.
//!
//! Product doc: `docs/truth-contract.md` (2026-04-21) — the `Committed verdict`
//! "decyduje o zapisie, auto-paste i sidecarze prawdy". [`TakeTruth`] is that
//! sidecar. It is an OBSERVER of the [`TranscriptionVerdict`]: engines label,
//! the acoustic ledger decides, this projection only records. It never steers
//! text and never reads anything back into delivery.
//!
//! Schema law: the twelve April fields keep their names and JSON shapes
//! verbatim — the Founder's reference sidecar
//! `~/.codescribe/transcriptions/2026-04-21/211316_ogolnie-plan-ktory_raw.txt.truth.json`
//! ("no o to to to — o takie wlasnie mi chodzilo") must deserialize and
//! round-trip unchanged. Every field added in schema v2 is optional with serde
//! defaults, so April files and the historical corpus parse as-is. The sidecar
//! carries no transcript text: text lives in the `.txt`; truth lives here.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Deserializer, Serialize};

use crate::pipeline::contracts::{
    TranscriptSegment, TranscriptionConfidenceFlag, TranscriptionVerdict,
};
use crate::util::safe_path::safe_read_to_string;

/// Schema version written by current builds. April files carry no
/// `schema_version` key and read as 1.
pub const TAKE_TRUTH_SCHEMA_VERSION: u8 = 2;

/// `schema_version` reads as 1 when the key is absent (April v1 files).
fn default_schema_version() -> u8 {
    1
}

/// v1 files omit `schema_version`; their re-serialized form stays v1-shaped.
fn is_v1_schema_version(version: &u8) -> bool {
    *version <= 1
}

/// The `.truth.json` sidecar: what really produced one take's transcript.
///
/// The first twelve fields are the April contract — identical names and JSON
/// shapes, nullable fields serialize as explicit `null` exactly like the
/// reference file. Everything below `engine_mode` is schema v2: optional,
/// serde-defaulted, and skipped when unset, so a v1 file re-serializes to its
/// original twelve-key shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TakeTruth {
    /// Sidecar schema version. Absent in April (v1) files → reads as 1;
    /// current builds write [`TAKE_TRUTH_SCHEMA_VERSION`].
    #[serde(
        default = "default_schema_version",
        skip_serializing_if = "is_v1_schema_version"
    )]
    pub schema_version: u8,
    /// Where the committed text came from. April vocabulary:
    /// `local_final_pass`, `toggle_session_adjudicated`, `cloud_primary`, …
    /// [`TakeTruth::from_verdict`] writes the core `TranscriptionSource` wire
    /// token; producers with richer app vocabulary may overwrite.
    pub source: String,
    /// Serving engine label. April used app-side labels (`local_whisper`,
    /// `cloud_stt`); `from_verdict` writes the `TranscriptionEngine` wire
    /// token (`whisper` / `apple`) — core does not invent app vocabulary.
    pub engine: String,
    /// Output mode. April's `raw` is the delivery flavor (raw `Transcript` vs
    /// `Formatted transcript`); the core verdict does not carry delivery
    /// flavor, so `from_verdict` writes the `TranscriptionEngineMode` wire
    /// token — the only `mode` Display in `contracts.rs`. Producers that know
    /// the flavor overwrite this field.
    pub mode: String,
    /// Fallback severity class when one was taken (`acceptable` / `degraded` /
    /// `unsafe`, app adjudication vocabulary). Null in April files.
    pub fallback_class: Option<String>,
    pub fallback_used: bool,
    /// Percentage of audio Silero classified as speech (0–100). Silero alone
    /// decides speech vs silence; this is its number, not a derived one.
    pub vad_speech_pct: f32,
    pub no_speech_reason: Option<String>,
    pub avg_logprob: Option<f32>,
    /// Confidence flags from the truth adjudicator. Typed tokens; older
    /// sidecars carrying unknown bare strings lose only those tokens, never
    /// the whole file (see `deserialize_confidence_flags_lenient`).
    #[serde(default, deserialize_with = "deserialize_confidence_flags_lenient")]
    pub confidence_flags: Vec<TranscriptionConfidenceFlag>,
    /// Silero 500 ms sparkline (one char per window, `█▓░ ` alphabet).
    #[serde(default)]
    pub sparkline: String,
    pub commit_trigger: Option<String>,
    /// Human status line in `docs/truth-contract.md` vocabulary — the
    /// `Committed verdict` surface, e.g. `Final-pass local • Transcript`
    /// (app) or `CLI • Transcript` (CLI). Opaque to core; never parsed.
    pub display_status: Option<String>,

    // ── schema v2 — optional; April files parse and re-serialize unchanged ──
    /// Engine provisioning path (`TranscriptionEngineMode` wire token:
    /// `embedded_default`, `runtime_fallback`, `speech_transcriber`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_mode: Option<String>,
    /// Fine Silero sparkline — one char per 32 ms `feed()` chunk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fine_sparkline: Option<String>,
    /// Hop of `fine_sparkline` in milliseconds (32 for native Silero).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fine_hop_ms: Option<u16>,
    /// Log-mel energy sparkline (`▁▂▃▄▅▆▇█` by relative dB) — the second
    /// clock, "żeby można było to również mnie oceniać jak historyczny
    /// sparkline silero".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub energy_sparkline: Option<String>,
    /// Hop of `energy_sparkline` in milliseconds (10 for the mel clock).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub energy_hop_ms: Option<u16>,
    /// Engine segment breakdown (`start_ts` / `end_ts` / `text`), when the
    /// engine provides one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<TranscriptSegment>,
    /// Reserved for PCM-based word pins (next plan, `codescribe-word-pins`).
    /// Always empty from this schema's writers; the field exists so the next
    /// plan writes into a shape readers already tolerate.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<WordPin>,
    /// Acoustic-ledger observation summary, when the take ran through the
    /// ledger. Observation only — the ledger remains the sole authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ledger: Option<LedgerSummary>,
    /// RFC 3339 generation timestamp, written by producers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_at: Option<String>,
}

/// A word pinned to PCM sample coordinates. Reserved for the
/// `codescribe-word-pins` plan; never populated by this schema's writers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WordPin {
    pub word: String,
    pub sample_start: u64,
    pub sample_end: u64,
    /// What pinned this word — the aligning instrument's name for the anchor.
    pub pin: String,
}

/// Acoustic-ledger observation summary carried by schema v2.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LedgerSummary {
    pub occurrences: usize,
    pub sealed: bool,
    #[serde(default)]
    pub refusals: Vec<String>,
}

impl TakeTruth {
    /// Project a [`TranscriptionVerdict`] into the schema-v2 sidecar shape.
    ///
    /// Pure observer mapping: every value comes from the verdict or the
    /// arguments; nothing is synthesized. `display_status` is the human
    /// status line (`docs/truth-contract.md` vocabulary — only a `Committed
    /// verdict` surface such as `Final-pass local • Transcript` or
    /// `CLI • Transcript`, never a `Live preview`); an empty string records
    /// `None`. `energy_buckets` is the target width of the energy sparkline
    /// rendered from `raw.energy` via
    /// `crate::stt::whisper::energy::sparkline`.
    ///
    /// April-field mapping (identical names, identical shapes):
    /// - `source` ← `TranscriptionSource` wire token (`local_final_pass`, …)
    /// - `engine` ← `TranscriptionEngine` wire token (`whisper` / `apple`);
    ///   April's app-side labels (`local_whisper`) are producer vocabulary
    ///   and are not invented here
    /// - `mode` ← `TranscriptionEngineMode` wire token; April's delivery
    ///   flavor (`raw`) is not knowable from the verdict, producers overwrite
    /// - `fallback_class` / `commit_trigger` — app adjudication vocabulary
    ///   the verdict does not carry; left `None`
    pub fn from_verdict(
        verdict: &TranscriptionVerdict,
        display_status: &str,
        energy_buckets: usize,
    ) -> TakeTruth {
        let vad = verdict.vad.as_ref();
        let energy = verdict.raw.energy.as_ref();
        TakeTruth {
            schema_version: TAKE_TRUTH_SCHEMA_VERSION,
            source: verdict.source.to_string(),
            engine: verdict.engine.engine.to_string(),
            mode: verdict.engine.mode.to_string(),
            fallback_class: None,
            fallback_used: verdict.engine.fallback_used,
            vad_speech_pct: vad.map_or(0.0, |vad| vad.speech_pct),
            no_speech_reason: vad.and_then(|vad| vad.no_speech_reason.clone()),
            avg_logprob: verdict.raw.avg_logprob,
            confidence_flags: verdict.confidence_flags.clone(),
            sparkline: vad.map_or_else(String::new, |vad| vad.sparkline.clone()),
            commit_trigger: None,
            display_status: if display_status.is_empty() {
                None
            } else {
                Some(display_status.to_string())
            },
            engine_mode: Some(verdict.engine.mode.to_string()),
            fine_sparkline: vad
                .map(|vad| vad.fine_sparkline.clone())
                .filter(|sparkline| !sparkline.is_empty()),
            fine_hop_ms: vad.map(|vad| vad.fine_hop_ms).filter(|hop| *hop > 0),
            energy_sparkline: energy.map(|timeline| {
                crate::stt::whisper::energy::sparkline(&timeline.frames, energy_buckets)
            }),
            energy_hop_ms: energy.map(|timeline| timeline.hop_ms),
            segments: verdict.raw.segments.clone(),
            words: Vec::new(),
            ledger: None,
            generated_at: None,
        }
    }
}

/// Sidecar path for a transcript artifact: `<file name>.truth.json` beside it.
pub fn truth_sidecar_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| format!("{name}.truth.json"))
        .unwrap_or_else(|| "artifact.truth.json".to_string());
    path.with_file_name(file_name)
}

/// Serialize `truth` beside `path` and return the sidecar path.
///
/// Pretty JSON, written to a temp file in the same directory and renamed into
/// place, so a crash mid-write never leaves a truncated sidecar behind.
pub fn write_truth_sidecar(path: &Path, truth: &TakeTruth) -> anyhow::Result<PathBuf> {
    let sidecar_path = truth_sidecar_path(path);
    let payload = serde_json::to_vec_pretty(truth).context("Failed to serialize truth sidecar")?;
    let tmp_path = sidecar_path.with_file_name(format!(
        ".{}.tmp",
        sidecar_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("artifact.truth.json")
    ));
    fs::write(&tmp_path, &payload)
        .with_context(|| format!("Failed to write truth sidecar {}", tmp_path.display()))?;
    fs::rename(&tmp_path, &sidecar_path).with_context(|| {
        format!(
            "Failed to finalize truth sidecar {}",
            sidecar_path.display()
        )
    })?;
    Ok(sidecar_path)
}

/// Read a truth sidecar back. Counterpart to [`write_truth_sidecar`], used to
/// prove roundtrip fidelity and by tooling that audits the archive.
pub fn read_truth_sidecar(path: &Path) -> anyhow::Result<TakeTruth> {
    let sidecar_path = truth_sidecar_path(path);
    let payload = safe_read_to_string(&sidecar_path)
        .with_context(|| format!("Failed to read truth sidecar {}", sidecar_path.display()))?;
    serde_json::from_str(&payload)
        .with_context(|| format!("Failed to parse truth sidecar {}", sidecar_path.display()))
}

/// Accept both the typed-enum representation (0.9.3+) and the bare-string one
/// written by earlier builds. Unknown strings from older sidecars are dropped
/// with a warning rather than failing the whole file, so old `truth.json`
/// files on disk remain readable.
fn deserialize_confidence_flags_lenient<'de, D>(
    deserializer: D,
) -> Result<Vec<TranscriptionConfidenceFlag>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw: Vec<serde_json::Value> = Vec::deserialize(deserializer)?;
    let mut out = Vec::with_capacity(raw.len());
    for value in raw {
        match serde_json::from_value::<TranscriptionConfidenceFlag>(value.clone()) {
            Ok(flag) => out.push(flag),
            Err(_) => {
                if let Some(token) = value.as_str() {
                    tracing::warn!(
                        unknown_flag = token,
                        "Dropping unknown confidence flag while reading truth.json"
                    );
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::contracts::{
        EnergyTimeline, RawTranscript, TranscriptionEngineMode, TranscriptionEngineVerdict,
        TranscriptionSource, VadVerdict,
    };

    /// The Founder's April reference sidecar, byte-verbatim (copied 2026-09-16
    /// from `~/.codescribe/transcriptions/2026-04-21/`).
    const APRIL_FIXTURE: &str = include_str!("../../tests/fixtures/truth_sidecar_20260421.json");

    /// The April file deserializes: twelve fields keep their names and values,
    /// `schema_version` reads as 1, every v2 field sits at its default.
    #[test]
    fn april_fixture_deserializes_as_schema_v1() {
        let truth: TakeTruth = serde_json::from_str(APRIL_FIXTURE).expect("April fixture parses");
        assert_eq!(truth.schema_version, 1);
        assert_eq!(truth.source, "local_final_pass");
        assert_eq!(truth.engine, "local_whisper");
        assert_eq!(truth.mode, "raw");
        assert_eq!(truth.fallback_class, None);
        assert!(!truth.fallback_used);
        assert!((truth.vad_speech_pct - 61.714287).abs() < 1e-6);
        assert_eq!(truth.no_speech_reason, None);
        assert!((truth.avg_logprob.expect("avg_logprob") - (-0.26560482)).abs() < 1e-6);
        assert!(truth.confidence_flags.is_empty());
        assert_eq!(truth.sparkline.chars().count(), 350);
        assert_eq!(truth.commit_trigger, None);
        assert_eq!(
            truth.display_status.as_deref(),
            Some("Final-pass local • Transcript")
        );
        // v2 defaults on a v1 file.
        assert_eq!(truth.engine_mode, None);
        assert_eq!(truth.fine_sparkline, None);
        assert_eq!(truth.fine_hop_ms, None);
        assert_eq!(truth.energy_sparkline, None);
        assert_eq!(truth.energy_hop_ms, None);
        assert!(truth.segments.is_empty());
        assert!(truth.words.is_empty());
        assert_eq!(truth.ledger, None);
        assert_eq!(truth.generated_at, None);
    }

    /// Re-serialize → parse → equals; the re-serialized April file keeps its
    /// twelve-key v1 shape (no `schema_version`, no v2 keys, no text).
    #[test]
    fn april_fixture_reserializes_and_reparses_equal() {
        let truth: TakeTruth = serde_json::from_str(APRIL_FIXTURE).unwrap();
        let json = serde_json::to_string_pretty(&truth).unwrap();
        let reparsed: TakeTruth = serde_json::from_str(&json).unwrap();
        assert_eq!(reparsed, truth);

        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let object = value.as_object().unwrap();
        assert_eq!(
            object.len(),
            12,
            "re-serialized April file keeps its twelve keys"
        );
        assert!(!object.contains_key("schema_version"));
        assert!(!object.contains_key("text"));
    }

    /// A schema-v2 file with every field populated round-trips losslessly.
    #[test]
    fn v2_take_truth_with_all_fields_roundtrips() {
        let truth = TakeTruth {
            schema_version: TAKE_TRUTH_SCHEMA_VERSION,
            source: "local_final_pass".to_string(),
            engine: "whisper".to_string(),
            mode: "raw".to_string(),
            fallback_class: Some("degraded".to_string()),
            fallback_used: true,
            vad_speech_pct: 61.714287,
            no_speech_reason: Some("vad_no_speech_detected".to_string()),
            avg_logprob: Some(-0.26560482),
            confidence_flags: vec![
                TranscriptionConfidenceFlag::VeryLowSpeech,
                TranscriptionConfidenceFlag::CloudFallbackUsed,
            ],
            sparkline: "░███".to_string(),
            commit_trigger: Some("cloud_failed_fallback".to_string()),
            display_status: Some("CLI • Transcript".to_string()),
            engine_mode: Some("embedded_default".to_string()),
            fine_sparkline: Some("▁▁▃▅▅▃▁▁".to_string()),
            fine_hop_ms: Some(32),
            energy_sparkline: Some("▂▅█▅▂".to_string()),
            energy_hop_ms: Some(10),
            segments: vec![TranscriptSegment {
                text: "cześć".to_string(),
                start_ts: 0.0,
                end_ts: 1.5,
            }],
            words: vec![WordPin {
                word: "cześć".to_string(),
                sample_start: 16_000,
                sample_end: 24_000,
                pin: "silero_valley".to_string(),
            }],
            ledger: Some(LedgerSummary {
                occurrences: 5,
                sealed: true,
                refusals: vec!["empty_mutation".to_string()],
            }),
            generated_at: Some("2026-09-16T04:00:00+02:00".to_string()),
        };
        let json = serde_json::to_string_pretty(&truth).expect("serialize v2");
        assert!(json.contains("\"schema_version\": 2"));
        let restored: TakeTruth = serde_json::from_str(&json).expect("deserialize v2");
        assert_eq!(restored, truth);
    }

    /// Keys from a future schema are ignored, not fatal.
    #[test]
    fn unknown_keys_are_ignored() {
        let json = r#"{
            "source": "local_final_pass",
            "engine": "local_whisper",
            "mode": "raw",
            "fallback_class": null,
            "fallback_used": false,
            "vad_speech_pct": 42.0,
            "no_speech_reason": null,
            "avg_logprob": null,
            "confidence_flags": [],
            "sparkline": "",
            "commit_trigger": null,
            "display_status": null,
            "future_field": {"nested": [1, 2, 3]}
        }"#;
        let truth: TakeTruth = serde_json::from_str(json).expect("unknown keys must be ignored");
        assert_eq!(truth.schema_version, 1);
        assert!((truth.vad_speech_pct - 42.0).abs() < f32::EPSILON);
    }

    /// Known flag tokens from older sidecars survive; unknown ones drop with
    /// a warning — a stray token never fails the file.
    #[test]
    fn older_confidence_flags_drop_unknown_tokens() {
        let json = r#"{
            "source": "local_final_pass",
            "engine": "local_whisper",
            "mode": "raw",
            "fallback_class": null,
            "fallback_used": false,
            "vad_speech_pct": 61.0,
            "no_speech_reason": null,
            "avg_logprob": -0.2,
            "confidence_flags": ["very_low_speech", "flag_from_the_future"],
            "sparkline": "",
            "commit_trigger": null,
            "display_status": null
        }"#;
        let truth: TakeTruth = serde_json::from_str(json).expect("older flags parse");
        assert_eq!(
            truth.confidence_flags,
            vec![TranscriptionConfidenceFlag::VeryLowSpeech]
        );
    }

    /// `from_verdict` maps the verdict into the April field shapes: source and
    /// VAD numbers verbatim, engine/mode as the contracts.rs wire tokens,
    /// fine clock carried, schema stamped v2.
    #[test]
    fn from_verdict_maps_verdict_truth_verbatim() {
        let verdict = TranscriptionVerdict::from_parts(
            "tekst".to_string(),
            RawTranscript {
                text: "tekst".to_string(),
                segments: vec![TranscriptSegment {
                    text: "tekst".to_string(),
                    start_ts: 0.0,
                    end_ts: 1.0,
                }],
                avg_logprob: Some(-0.26560482),
                ..Default::default()
            },
            Some(VadVerdict {
                speech_pct: 61.714287,
                speech_windows: 10,
                total_windows: 16,
                no_speech: false,
                no_speech_reason: None,
                sparkline: "░███".to_string(),
                fine_sparkline: "▁▃█▃▁".to_string(),
                fine_hop_ms: 32,
            }),
            TranscriptionSource::LocalFinalPass,
            TranscriptionEngineVerdict::whisper(TranscriptionEngineMode::EmbeddedDefault),
            None,
        );
        let truth = TakeTruth::from_verdict(&verdict, "Final-pass local • Transcript", 0);
        assert_eq!(truth.schema_version, TAKE_TRUTH_SCHEMA_VERSION);
        assert_eq!(truth.source, "local_final_pass");
        assert_eq!(truth.engine, "whisper");
        assert_eq!(truth.mode, "embedded_default");
        assert_eq!(truth.engine_mode.as_deref(), Some("embedded_default"));
        assert!(!truth.fallback_used);
        assert!((truth.vad_speech_pct - 61.714287).abs() < 1e-6);
        assert!((truth.avg_logprob.expect("avg_logprob") - (-0.26560482)).abs() < 1e-6);
        assert_eq!(truth.sparkline, "░███");
        assert_eq!(truth.fine_sparkline.as_deref(), Some("▁▃█▃▁"));
        assert_eq!(truth.fine_hop_ms, Some(32));
        assert_eq!(truth.segments.len(), 1);
        assert!(truth.energy_sparkline.is_none());
        assert_eq!(truth.energy_hop_ms, None);
        assert_eq!(
            truth.display_status.as_deref(),
            Some("Final-pass local • Transcript")
        );
        assert!(truth.words.is_empty());
        assert_eq!(truth.ledger, None);
        assert_eq!(truth.generated_at, None);
    }

    /// A present `raw.energy` becomes an energy sparkline of the requested
    /// width, rendered by the mel clock; an empty status records `None`.
    #[test]
    fn from_verdict_renders_energy_sparkline_from_raw_energy() {
        let verdict = TranscriptionVerdict::from_parts(
            "zegar".to_string(),
            RawTranscript {
                text: "zegar".to_string(),
                energy: Some(EnergyTimeline {
                    hop_ms: 10,
                    frames: vec![-80.0, -20.0, -40.0],
                    voice: vec![-70.0, -25.0, -35.0],
                }),
                ..Default::default()
            },
            None,
            TranscriptionSource::LocalFinalPass,
            TranscriptionEngineVerdict::whisper(TranscriptionEngineMode::EmbeddedDefault),
            None,
        );
        let truth = TakeTruth::from_verdict(&verdict, "", 3);
        assert_eq!(truth.energy_hop_ms, Some(10));
        let sparkline = truth.energy_sparkline.expect("energy sparkline rendered");
        assert_eq!(sparkline.chars().count(), 3);
        assert_eq!(truth.display_status, None, "empty status records None");
        assert_eq!(truth.vad_speech_pct, 0.0, "no VAD verdict means 0.0");
    }

    /// The sidecar lives beside its artifact as `<file name>.truth.json`.
    #[test]
    fn truth_sidecar_path_appends_truth_json_beside_file() {
        assert_eq!(
            truth_sidecar_path(Path::new("/archive/2026-04-21/211316_plan_raw.txt")),
            PathBuf::from("/archive/2026-04-21/211316_plan_raw.txt.truth.json")
        );
    }

    /// write → read through the sidecar path round-trips on disk, and the
    /// temp+rename dance leaves no `.tmp` residue.
    #[test]
    fn write_then_read_sidecar_roundtrips_on_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let transcript = dir.path().join("take_raw.txt");
        fs::write(&transcript, "treść").expect("write transcript");

        let truth = TakeTruth {
            schema_version: TAKE_TRUTH_SCHEMA_VERSION,
            source: "local_final_pass".to_string(),
            engine: "whisper".to_string(),
            mode: "embedded_default".to_string(),
            fallback_class: None,
            fallback_used: false,
            vad_speech_pct: 61.714287,
            no_speech_reason: None,
            avg_logprob: Some(-0.26560482),
            confidence_flags: vec![TranscriptionConfidenceFlag::UnverifiedStream],
            sparkline: "░███".to_string(),
            commit_trigger: None,
            display_status: Some("CLI • Transcript".to_string()),
            engine_mode: Some("embedded_default".to_string()),
            fine_sparkline: Some("▁▃█▃▁".to_string()),
            fine_hop_ms: Some(32),
            energy_sparkline: Some("▂▅█".to_string()),
            energy_hop_ms: Some(10),
            segments: vec![TranscriptSegment {
                text: "treść".to_string(),
                start_ts: 0.0,
                end_ts: 1.0,
            }],
            words: Vec::new(),
            ledger: None,
            generated_at: Some("2026-09-16T04:00:00+02:00".to_string()),
        };

        let sidecar = write_truth_sidecar(&transcript, &truth).expect("write sidecar");
        assert_eq!(
            sidecar,
            dir.path().join("take_raw.txt.truth.json"),
            "sidecar lands beside the artifact"
        );
        let restored = read_truth_sidecar(&transcript).expect("read sidecar");
        assert_eq!(restored, truth);
        assert!(
            !dir.path().join(".take_raw.txt.truth.json.tmp").exists(),
            "temp file must be renamed away"
        );
    }
}
