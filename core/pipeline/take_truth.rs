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

#[cfg(test)]
use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use serde::{Deserialize, Deserializer, Serialize};

use crate::pipeline::contracts::{
    TranscriptSegment, TranscriptionConfidenceFlag, TranscriptionVerdict,
};

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
    /// Percentage of audio classified as speech (0–100). Null when that
    /// measurement was not supplied. A stored number, including 0, stays a
    /// number. Live capture derives this only from an authenticated coverage
    /// receipt; a verdict maps its own VAD figure and does not invent 0.
    #[serde(default)]
    pub vad_speech_pct: Option<f32>,
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
    /// PCM word pins copied from producer-supplied individual pin evidence.
    /// Empty when the producer did not supply that evidence. Display
    /// tokenization and duration are not pins.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<WordPin>,
    /// Acoustic-ledger observation summary, when the take ran through the
    /// ledger. Observation only — the ledger remains the sole authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ledger: Option<LedgerSummary>,
    /// RFC 3339 generation timestamp, written by producers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_at: Option<String>,
    /// Capture session this observer was frozen from. Absent when the take
    /// did not supply one. Presence is what makes the card capture-bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Capture epoch paired with `session_id`. Absent stays absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_epoch: Option<u64>,
    /// Set only when this take's ledger or capture identity could not be read.
    /// `unavailable` is that report. It is not a ledger summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ledger_evidence: Option<String>,
}

/// A word pinned to PCM sample coordinates.
///
/// `observe_ledger` fills this only from a slot whose exact range is already
/// in the ledger's committed word-pin evidence and whose text is non-empty.
/// Display tokens and durations are not pins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WordPin {
    pub word: String,
    pub sample_start: u64,
    pub sample_end: u64,
    /// What pinned this word — the aligning instrument's name for the anchor.
    pub pin: String,
}

/// Acoustic-ledger observation summary carried by schema v2.
///
/// The first three fields stay the April-v2 summary. Later fields copy
/// read-only receipt facts for this capture. They are omitted when empty so
/// an older summary still parses and re-serializes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LedgerSummary {
    pub occurrences: usize,
    pub sealed: bool,
    #[serde(default)]
    pub refusals: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<LedgerDecisionFact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub seals: Vec<LedgerSealFact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<LedgerCoverageFact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<LedgerTerminalFact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shapings: Vec<LedgerShapingFact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub manual_revisions: Vec<LedgerManualRevisionFact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub manual_edits: Vec<LedgerManualEditFact>,
}

/// Capture coordinate copied from an occurrence. No text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerOccurrenceSpan {
    pub session_id: String,
    pub capture_epoch: u64,
    pub sample_start: u64,
    pub sample_end: u64,
}

/// Serial identity copied from the ledger: version, digest, and that serial's
/// own occurrence. Energy figures stay on the ledger. Cards written before
/// `occurrence` omit it; absence is not a coordinate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerSerialFact {
    pub version: u16,
    pub digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurrence: Option<LedgerOccurrenceSpan>,
}

/// One layer decision, without the candidate label or its tokens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerDecisionFact {
    pub ordinal: u64,
    pub receipt_id: String,
    pub layer: String,
    pub producer: String,
    pub decision: String,
    pub session_id: String,
    pub capture_epoch: u64,
    pub sample_start: u64,
    pub sample_end: u64,
    /// Copied from `observation.request`. Not derived from `ordinal`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<u64>,
    /// Copied from `observation.generation`. Not derived from `ordinal`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub serials: Vec<LedgerSerialFact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predecessor_ordinal: Option<u64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub clock_lie: bool,
}

/// One existing seal, occurrence or terminal. Not minted here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerSealFact {
    pub scope: String,
    pub receipt_id: String,
    pub session_id: String,
    pub capture_epoch: u64,
    pub sample_start: u64,
    pub sample_end: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sealed_occurrences: Vec<LedgerOccurrenceSpan>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub serials: Vec<LedgerSerialFact>,
    pub vad_close_sample: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layer_trail_ordinals: Vec<u64>,
}

/// Coverage measurement copied from the matching receipt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LedgerCoverageFact {
    pub session_id: String,
    pub capture_epoch: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate_hz: Option<u32>,
    pub speech_samples: u64,
    pub covered_samples: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_samples: Option<u64>,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
    pub speech_producer: String,
    pub availability: String,
    pub max_uncovered_samples: u64,
    pub incomplete_threshold_samples: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uncovered_speech: Vec<LedgerOccurrenceSpan>,
}

/// Terminal finality already issued for this capture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerTerminalFact {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

/// Light+ shaping receipt without presentation text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerShapingFact {
    pub receipt_id: String,
    pub provenance: String,
    pub session_id: String,
    pub capture_epoch: u64,
    pub sample_start: u64,
    pub sample_end: u64,
    pub source_revision: u64,
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_seal_receipt: Option<String>,
    pub sentence_break_before: bool,
    pub left_context_sha256: String,
}

/// Whole-document revision without rendered text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerManualRevisionFact {
    pub receipt_id: String,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_epoch: Option<u64>,
    pub provenance: String,
    pub source_revision: u64,
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_seal_receipts: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_occurrences: Vec<LedgerOccurrenceSpan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_receipt_id: Option<String>,
}

/// Manual label supersession without either label string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerManualEditFact {
    pub receipt_id: String,
    pub session_id: String,
    pub capture_epoch: u64,
    pub sample_start: u64,
    pub sample_end: u64,
    pub supersedes_seal: String,
    pub producer: String,
}

fn is_false(value: &bool) -> bool {
    !*value
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
            vad_speech_pct: vad.map(|vad| vad.speech_pct),
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
            session_id: None,
            capture_epoch: None,
            ledger_evidence: None,
        }
    }

    /// Capture card with no ledger rows. Verdict fields stay empty; this does
    /// not invent an engine, a seal, or a speech percentage.
    pub fn unavailable_evidence(session_id: Option<String>, capture_epoch: Option<u64>) -> Self {
        Self {
            session_id,
            capture_epoch,
            ledger_evidence: Some("unavailable".to_string()),
            generated_at: Some(generated_now()),
            ..Self::shell()
        }
    }

    /// Copy this capture's existing ledger facts. Reads only. Mints nothing.
    pub fn observe_ledger(
        ledger: &crate::pipeline::acoustic_ledger::AcousticLedger,
        session_id: &str,
        capture_epoch: u64,
    ) -> Self {
        use crate::pipeline::acoustic_ledger::{MutationReceipt, TerminalFinality};

        let same = |occurrence: &crate::pipeline::acoustic_ledger::OccurrenceIdentity| {
            occurrence.session == session_id && occurrence.capture_epoch == capture_epoch
        };
        let occurrences: Vec<_> = ledger
            .occurrences()
            .filter(|occurrence| same(occurrence))
            .cloned()
            .collect();
        let mut decisions = Vec::new();
        let mut refusals = Vec::new();
        for entry in ledger.layer_trail() {
            if !same(&entry.observation.occurrence) {
                continue;
            }
            let refusal = match &entry.decision {
                MutationReceipt::Refuse { reason, .. } => Some(reason.as_str().to_string()),
                _ => None,
            };
            if let Some(reason) = refusal.clone() {
                refusals.push(reason);
            }
            decisions.push(LedgerDecisionFact {
                ordinal: entry.ordinal as u64,
                receipt_id: entry.receipt_id.clone(),
                layer: entry.layer().to_string(),
                producer: entry.producer().as_str().to_string(),
                decision: entry.decision.as_str().to_string(),
                session_id: entry.observation.occurrence.session.clone(),
                capture_epoch: entry.observation.occurrence.capture_epoch,
                sample_start: entry.observation.occurrence.sample_start,
                sample_end: entry.observation.occurrence.sample_end,
                request: Some(entry.observation.request),
                generation: Some(entry.observation.generation),
                serials: entry.serials.iter().map(serial_fact).collect(),
                refusal,
                predecessor_ordinal: entry.predecessor_ordinal.map(|ordinal| ordinal as u64),
                clock_lie: entry.clock_lie,
            });
        }

        let mut seals = Vec::new();
        for occurrence in &occurrences {
            if let Some(seal) = ledger.seal_of(occurrence)
                && same(&seal.coverage)
            {
                seals.push(seal_fact(seal));
            }
        }
        let mut words = Vec::new();
        for occurrence in &occurrences {
            let pin_ranges = ledger.committed_word_pin_ranges(occurrence);
            for slot in ledger.slots_of(occurrence).unwrap_or(&[]) {
                let pinned = pin_ranges
                    .iter()
                    .any(|(start, end)| *start == slot.sample_start && *end == slot.sample_end);
                if pinned && !slot.text.is_empty() {
                    words.push(WordPin {
                        word: slot.text.clone(),
                        sample_start: slot.sample_start,
                        sample_end: slot.sample_end,
                        pin: slot.producer.as_str().to_string(),
                    });
                }
            }
        }

        let terminal_state = ledger.terminal_finality(session_id, capture_epoch);
        let (sealed, terminal, terminal_seal) = match &terminal_state {
            TerminalFinality::Sealed(receipt) if same(&receipt.coverage) => (
                true,
                Some(LedgerTerminalFact {
                    status: "sealed".to_string(),
                    reason: None,
                    receipt_id: Some(receipt.receipt_id.clone()),
                    scope: Some(receipt.scope.as_str().to_string()),
                }),
                Some(seal_fact(receipt)),
            ),
            TerminalFinality::ObservedSilence(coverage)
                if coverage.session_id == session_id && coverage.capture_epoch == capture_epoch =>
            {
                (
                    false,
                    Some(LedgerTerminalFact {
                        status: "observed_silence".to_string(),
                        reason: None,
                        receipt_id: None,
                        scope: None,
                    }),
                    None,
                )
            }
            TerminalFinality::Refused(refusal)
                if refusal.session_id() == session_id
                    && refusal.capture_epoch() == capture_epoch =>
            {
                refusals.push(refusal.reason().as_str().to_string());
                (
                    false,
                    Some(LedgerTerminalFact {
                        status: "refused".to_string(),
                        reason: Some(refusal.reason().as_str().to_string()),
                        receipt_id: None,
                        scope: None,
                    }),
                    None,
                )
            }
            _ => (false, None, None),
        };
        if let Some(seal) = terminal_seal
            && !seals
                .iter()
                .any(|existing| existing.receipt_id == seal.receipt_id)
        {
            seals.push(seal);
        }

        let coverage = ledger
            .latest_seal_coverage()
            .filter(|coverage| {
                coverage.session_id == session_id && coverage.capture_epoch == capture_epoch
            })
            .or_else(|| match &terminal_state {
                TerminalFinality::ObservedSilence(coverage)
                    if coverage.session_id == session_id
                        && coverage.capture_epoch == capture_epoch =>
                {
                    Some(coverage)
                }
                TerminalFinality::Refused(refusal) => refusal.coverage().filter(|coverage| {
                    coverage.session_id == session_id && coverage.capture_epoch == capture_epoch
                }),
                _ => None,
            });
        let vad_speech_pct = coverage.and_then(measured_speech_pct);
        let coverage = coverage.map(coverage_fact);

        let shapings = ledger
            .incremental_shapings()
            .iter()
            .filter(|receipt| receipt.session_id == session_id && same(&receipt.occurrence))
            .map(|receipt| LedgerShapingFact {
                receipt_id: receipt.receipt_id.clone(),
                provenance: receipt.provenance.clone(),
                session_id: receipt.session_id.clone(),
                capture_epoch: receipt.occurrence.capture_epoch,
                sample_start: receipt.occurrence.sample_start,
                sample_end: receipt.occurrence.sample_end,
                source_revision: receipt.source_revision,
                revision: receipt.revision,
                source_seal_receipt: receipt.source_seal_receipt.clone(),
                sentence_break_before: receipt.sentence_break_before,
                left_context_sha256: receipt.left_context_sha256.clone(),
            })
            .collect();

        let manual_revisions = ledger
            .manual_document_revisions()
            .iter()
            .filter(|receipt| {
                receipt.session_id == session_id && receipt.capture_epoch == Some(capture_epoch)
            })
            .map(|receipt| LedgerManualRevisionFact {
                receipt_id: receipt.receipt_id.clone(),
                session_id: receipt.session_id.clone(),
                capture_epoch: receipt.capture_epoch,
                provenance: receipt.provenance.clone(),
                source_revision: receipt.source_revision,
                revision: receipt.revision,
                source_seal_receipts: receipt.source_seal_receipts.clone(),
                source_occurrences: receipt
                    .source_occurrences
                    .iter()
                    .filter(|occurrence| same(occurrence))
                    .map(span_from_occurrence)
                    .collect(),
                capture_receipt_id: receipt.capture_receipt_id.clone(),
            })
            .collect();

        let manual_edits = ledger
            .manual_edits()
            .iter()
            .filter(|receipt| same(&receipt.occurrence))
            .map(|receipt| LedgerManualEditFact {
                receipt_id: receipt.receipt_id.clone(),
                session_id: receipt.occurrence.session.clone(),
                capture_epoch: receipt.occurrence.capture_epoch,
                sample_start: receipt.occurrence.sample_start,
                sample_end: receipt.occurrence.sample_end,
                supersedes_seal: receipt.supersedes_seal.clone(),
                producer: receipt.observation.producer.as_str().to_string(),
            })
            .collect();

        Self {
            session_id: Some(session_id.to_string()),
            capture_epoch: Some(capture_epoch),
            generated_at: Some(generated_now()),
            vad_speech_pct,
            words,
            ledger: Some(LedgerSummary {
                occurrences: occurrences.len(),
                sealed,
                refusals,
                decisions,
                seals,
                coverage,
                terminal,
                shapings,
                manual_revisions,
                manual_edits,
            }),
            ..Self::shell()
        }
    }

    fn shell() -> Self {
        Self {
            schema_version: TAKE_TRUTH_SCHEMA_VERSION,
            source: String::new(),
            engine: String::new(),
            mode: String::new(),
            fallback_class: None,
            fallback_used: false,
            vad_speech_pct: None,
            no_speech_reason: None,
            avg_logprob: None,
            confidence_flags: Vec::new(),
            sparkline: String::new(),
            commit_trigger: None,
            display_status: None,
            engine_mode: None,
            fine_sparkline: None,
            fine_hop_ms: None,
            energy_sparkline: None,
            energy_hop_ms: None,
            segments: Vec::new(),
            words: Vec::new(),
            ledger: None,
            generated_at: None,
            session_id: None,
            capture_epoch: None,
            ledger_evidence: None,
        }
    }

    fn authorizes_replacement_of(&self, existing: &Self) -> bool {
        existing.session_id.is_some()
            && self.session_id == existing.session_id
            && self.capture_epoch == existing.capture_epoch
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
/// The selected parent is canonicalized once, so a platform alias such as
/// `/var` is admitted, and that directory descriptor is held for the rest of
/// the call. Leaf metadata, the existing card, the private stage, and the
/// rename all use that descriptor. A capture-bound card is not replaced by an
/// observer that does not already carry the same session and epoch. This
/// function does not copy an identity onto the incoming card. The returned
/// path keeps the caller spelling, including a relative path or a macOS alias.
pub fn write_truth_sidecar(path: &Path, truth: &TakeTruth) -> anyhow::Result<PathBuf> {
    #[cfg(unix)]
    {
        write_truth_sidecar_pinned(path, truth)
    }
    #[cfg(not(unix))]
    {
        let _ = (path, truth);
        anyhow::bail!("truth sidecar publication requires unix directory descriptors")
    }
}

/// Publish `truth` into a directory the caller has already admitted.
///
/// `path` selects only the logical sidecar spelling, the same way
/// [`write_truth_sidecar`] does. That spelling is not used to admit a parent.
/// The per-directory lock, the existing-card read, the exclusive stage, the
/// write, the sync, the rename, and stage cleanup all use `directory`.
pub fn write_truth_sidecar_at(
    directory: &File,
    path: &Path,
    truth: &TakeTruth,
) -> anyhow::Result<PathBuf> {
    #[cfg(unix)]
    {
        let sidecar_path = truth_sidecar_path(path);
        // Parent text is validated and then discarded. Opening it would admit
        // a different directory than the one retention already pinned.
        let (_logical_parent, leaf) = truth_directory_parts(&sidecar_path)?;
        let lock = TruthDirLock::acquire_held(directory)?;
        publish_admitted_truth_card(&lock.directory, &leaf, &sidecar_path, truth)?;
        Ok(sidecar_path)
    }
    #[cfg(not(unix))]
    {
        let _ = (directory, path, truth);
        anyhow::bail!("truth sidecar publication requires unix directory descriptors")
    }
}

/// Read a truth sidecar back. Counterpart to [`write_truth_sidecar`], used to
/// prove roundtrip fidelity and by tooling that audits the archive.
///
/// The read uses the same parent admission as publication and refuses a
/// sidecar leaf that is a symlink or a special file.
pub fn read_truth_sidecar(path: &Path) -> anyhow::Result<TakeTruth> {
    let sidecar_path = truth_sidecar_path(path);
    #[cfg(unix)]
    {
        read_truth_sidecar_pinned(&sidecar_path)
            .with_context(|| format!("Failed to read truth sidecar {}", sidecar_path.display()))
    }
    #[cfg(not(unix))]
    {
        let _ = sidecar_path;
        anyhow::bail!("truth sidecar read requires unix directory descriptors")
    }
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

#[cfg(unix)]
enum ExistingTruthCard {
    Absent,
    Unbound,
    Bound(Box<TakeTruth>),
}

#[cfg(unix)]
enum TruthLeafKind {
    Absent,
    Regular,
    Symlink,
    Special,
}

#[cfg(unix)]
struct TruthDirLock {
    directory: File,
}

#[cfg(unix)]
impl TruthDirLock {
    fn acquire(parent: &Path) -> anyhow::Result<Self> {
        Self::lock_owned(admit_truth_directory(parent)?)
    }

    /// Lock a new description of a directory the caller already admitted.
    /// The description is opened from that capability. It does not look up a
    /// path, and it does not share the caller's open-file description.
    fn acquire_held(directory: &File) -> anyhow::Result<Self> {
        let directory = Self::reopen_directory_capability(directory)?;
        if !directory.metadata()?.is_dir() {
            anyhow::bail!("truth sidecar parent is not a directory");
        }
        Self::lock_owned(directory)
    }

    /// Open `.` relative to an admitted directory descriptor.
    /// `.` is that capability, not a sidecar leaf. Leaf checks still reject `.` and `..`.
    fn reopen_directory_capability(directory: &File) -> anyhow::Result<File> {
        use std::os::fd::{AsRawFd, FromRawFd};
        let name = c".";
        // SAFETY: the admitted descriptor and the static name live through openat.
        // A successful descriptor has exactly one File owner; errors close none.
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
                0,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error())
                .context("truth sidecar directory capability refused");
        }
        // SAFETY: `fd` came from a successful openat and has exactly one owner.
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    fn lock_owned(directory: File) -> anyhow::Result<Self> {
        use std::os::fd::AsRawFd;
        // SAFETY: `directory` stays open for the life of this lock.
        let rc = unsafe { libc::flock(directory.as_raw_fd(), libc::LOCK_EX) };
        if rc < 0 {
            return Err(std::io::Error::last_os_error())
                .context("truth sidecar publication lock refused");
        }
        Ok(Self { directory })
    }
}

#[cfg(unix)]
impl Drop for TruthDirLock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        // SAFETY: unlock only the directory descriptor this lock acquired.
        unsafe {
            libc::flock(self.directory.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

/// Split a sidecar path into its selected parent and one leaf component.
/// `..` is refused before any filesystem lookup. The parent may be relative.
#[cfg(unix)]
fn truth_directory_parts(sidecar_path: &Path) -> anyhow::Result<(PathBuf, std::ffi::CString)> {
    use std::os::unix::ffi::OsStrExt;
    if sidecar_path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        anyhow::bail!("truth sidecar refuses parent traversal");
    }
    if sidecar_path
        .components()
        .any(|component| matches!(component, Component::Prefix(_)))
    {
        anyhow::bail!("truth sidecar refuses unsafe path");
    }
    let leaf = sidecar_path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("truth sidecar requires a leaf"))?;
    let name = std::ffi::CString::new(leaf.as_bytes())
        .map_err(|_| anyhow::anyhow!("truth sidecar refuses unsafe leaf"))?;
    truth_component(&name)?;
    let parent = sidecar_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    Ok((parent.to_path_buf(), name))
}

#[cfg(unix)]
fn truth_component(name: &std::ffi::CStr) -> anyhow::Result<()> {
    let bytes = name.to_bytes();
    anyhow::ensure!(
        !bytes.is_empty() && !bytes.contains(&b'/') && bytes != b"." && bytes != b"..",
        "truth sidecar requires one safe component"
    );
    Ok(())
}

/// Canonicalize the selected parent, including a platform alias such as
/// `/var`, then walk that absolute path with `O_NOFOLLOW`. The returned
/// descriptor is the only directory later leaf operations may use.
#[cfg(unix)]
fn admit_truth_directory(parent: &Path) -> anyhow::Result<File> {
    use std::os::unix::ffi::OsStrExt;
    let resolved = parent
        .canonicalize()
        .with_context(|| format!("open truth directory {}", parent.display()))?;
    let mut directory = File::open("/").context("open truth directory /")?;
    for component in resolved.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(part) => {
                let part = std::ffi::CString::new(part.as_bytes())
                    .map_err(|_| anyhow::anyhow!("truth sidecar refuses unsafe path"))?;
                directory = open_truth_entry(&directory, &part, libc::O_RDONLY | libc::O_DIRECTORY)
                    .with_context(|| format!("open truth directory {}", resolved.display()))?;
            }
            _ => anyhow::bail!("truth sidecar parent is not absolute and normalized"),
        }
    }
    if !directory.metadata()?.is_dir() {
        anyhow::bail!(
            "truth sidecar parent is not a directory: {}",
            parent.display()
        );
    }
    Ok(directory)
}

/// Open exactly one directory-relative entry. `O_NOFOLLOW` refuses a symlink leaf.
#[cfg(unix)]
fn open_truth_entry(
    directory: &File,
    name: &std::ffi::CStr,
    flags: libc::c_int,
) -> anyhow::Result<File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    truth_component(name)?;
    // SAFETY: the directory and NUL-terminated name live through openat. A
    // successful descriptor has exactly one File owner; all errors close none.
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: `fd` came from a successful openat and has exactly one owner.
    Ok(unsafe { File::from_raw_fd(fd) })
}

#[cfg(unix)]
fn truth_leaf_kind(directory: &File, name: &std::ffi::CStr) -> anyhow::Result<TruthLeafKind> {
    use std::os::fd::AsRawFd;
    // SAFETY: `info` is written by fstatat before it is read. The call names one
    // component and does not follow a symlink.
    let mut info: libc::stat = unsafe { std::mem::zeroed() };
    let rc = unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            name.as_ptr(),
            &mut info,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if rc < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            return Ok(TruthLeafKind::Absent);
        }
        return Err(error.into());
    }
    match info.st_mode & libc::S_IFMT {
        libc::S_IFLNK => Ok(TruthLeafKind::Symlink),
        libc::S_IFREG => Ok(TruthLeafKind::Regular),
        _ => Ok(TruthLeafKind::Special),
    }
}

#[cfg(unix)]
fn read_truth_leaf(directory: &File, name: &std::ffi::CStr) -> anyhow::Result<String> {
    use std::io::Read;
    let mut file = open_truth_entry(directory, name, libc::O_RDONLY)?;
    let mut payload = String::new();
    file.read_to_string(&mut payload)?;
    Ok(payload)
}

#[cfg(unix)]
fn unlink_owned_stage(directory: &File, name: &std::ffi::CStr) {
    use std::os::fd::AsRawFd;
    // SAFETY: `name` is the stage this call created with O_EXCL in `directory`.
    unsafe {
        libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0);
    }
}

#[cfg(unix)]
fn truth_stage_name(leaf: &std::ffi::CStr) -> anyhow::Result<std::ffi::CString> {
    let leaf = std::str::from_utf8(leaf.to_bytes()).unwrap_or("artifact.truth.json");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let name = format!(".{leaf}.{}.{nanos}.tmp", std::process::id());
    let stage = std::ffi::CString::new(name)
        .map_err(|_| anyhow::anyhow!("truth sidecar refuses unsafe leaf"))?;
    truth_component(&stage)?;
    Ok(stage)
}

#[cfg(unix)]
fn existing_truth_card(
    directory: &File,
    name: &std::ffi::CStr,
    sidecar_path: &Path,
) -> anyhow::Result<ExistingTruthCard> {
    match truth_leaf_kind(directory, name)? {
        TruthLeafKind::Absent => Ok(ExistingTruthCard::Absent),
        TruthLeafKind::Symlink => anyhow::bail!(
            "refusing to write truth sidecar through symlink {}",
            sidecar_path.display()
        ),
        TruthLeafKind::Special => anyhow::bail!(
            "truth sidecar destination is not a regular file: {}",
            sidecar_path.display()
        ),
        TruthLeafKind::Regular => {
            let payload = read_truth_leaf(directory, name).with_context(|| {
                format!("read existing truth sidecar {}", sidecar_path.display())
            })?;
            match serde_json::from_str::<TakeTruth>(&payload) {
                Ok(truth) if truth.session_id.is_some() => {
                    Ok(ExistingTruthCard::Bound(Box::new(truth)))
                }
                Ok(_) => Ok(ExistingTruthCard::Unbound),
                Err(error) => Err(error).context(format!(
                    "refusing to replace unreadable truth sidecar {}",
                    sidecar_path.display()
                )),
            }
        }
    }
}

#[cfg(unix)]
fn write_truth_sidecar_pinned(path: &Path, truth: &TakeTruth) -> anyhow::Result<PathBuf> {
    let sidecar_path = truth_sidecar_path(path);
    let (parent, leaf) = truth_directory_parts(&sidecar_path)?;
    let lock = TruthDirLock::acquire(&parent)?;
    publish_admitted_truth_card(&lock.directory, &leaf, &sidecar_path, truth)?;
    Ok(sidecar_path)
}

#[cfg(unix)]
fn publish_admitted_truth_card(
    directory: &File,
    leaf: &std::ffi::CStr,
    sidecar_path: &Path,
    truth: &TakeTruth,
) -> anyhow::Result<()> {
    use std::os::fd::AsRawFd;
    let payload = serde_json::to_vec_pretty(truth).context("Failed to serialize truth sidecar")?;
    match existing_truth_card(directory, leaf, sidecar_path)? {
        ExistingTruthCard::Bound(existing)
            if !truth.authorizes_replacement_of(existing.as_ref()) =>
        {
            anyhow::bail!(
                "refusing truth sidecar replacement that does not match this capture; preserving capture-bound observer at {}",
                sidecar_path.display()
            );
        }
        ExistingTruthCard::Absent | ExistingTruthCard::Unbound | ExistingTruthCard::Bound(_) => {}
    }
    let stage = truth_stage_name(leaf)?;
    let mut created = open_truth_entry(
        directory,
        &stage,
        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
    )
    .with_context(|| format!("Failed to create truth sidecar {}", sidecar_path.display()))?;
    let published = (|| -> anyhow::Result<()> {
        created
            .write_all(&payload)
            .with_context(|| format!("Failed to write truth sidecar {}", sidecar_path.display()))?;
        created
            .sync_all()
            .with_context(|| format!("Failed to sync truth sidecar {}", sidecar_path.display()))?;
        match truth_leaf_kind(directory, leaf)? {
            TruthLeafKind::Symlink => anyhow::bail!(
                "refusing to write truth sidecar through symlink {}",
                sidecar_path.display()
            ),
            TruthLeafKind::Special => anyhow::bail!(
                "truth sidecar destination is not a regular file: {}",
                sidecar_path.display()
            ),
            TruthLeafKind::Absent | TruthLeafKind::Regular => {}
        }
        // SAFETY: both names and the held directory survive the call. renameat
        // replaces the leaf entry, does not follow it, and stays on this directory.
        if unsafe {
            libc::renameat(
                directory.as_raw_fd(),
                stage.as_ptr(),
                directory.as_raw_fd(),
                leaf.as_ptr(),
            )
        } < 0
        {
            return Err(std::io::Error::last_os_error()).with_context(|| {
                format!(
                    "Failed to finalize truth sidecar {}",
                    sidecar_path.display()
                )
            });
        }
        Ok(())
    })();
    if published.is_err() {
        unlink_owned_stage(directory, &stage);
    }
    published
}

#[cfg(unix)]
fn read_truth_sidecar_pinned(sidecar_path: &Path) -> anyhow::Result<TakeTruth> {
    let (parent, leaf) = truth_directory_parts(sidecar_path)?;
    let directory = admit_truth_directory(&parent)?;
    match truth_leaf_kind(&directory, &leaf)? {
        TruthLeafKind::Absent => {
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "truth sidecar is absent").into())
        }
        TruthLeafKind::Symlink => anyhow::bail!(
            "refusing to read truth sidecar through symlink {}",
            sidecar_path.display()
        ),
        TruthLeafKind::Special => anyhow::bail!(
            "truth sidecar is not a regular file: {}",
            sidecar_path.display()
        ),
        TruthLeafKind::Regular => {
            let payload = read_truth_leaf(&directory, &leaf)?;
            serde_json::from_str(&payload).with_context(|| {
                format!("Failed to parse truth sidecar {}", sidecar_path.display())
            })
        }
    }
}

fn generated_now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn span_from_occurrence(
    occurrence: &crate::pipeline::acoustic_ledger::OccurrenceIdentity,
) -> LedgerOccurrenceSpan {
    LedgerOccurrenceSpan {
        session_id: occurrence.session.clone(),
        capture_epoch: occurrence.capture_epoch,
        sample_start: occurrence.sample_start,
        sample_end: occurrence.sample_end,
    }
}

fn serial_fact(serial: &crate::pipeline::acoustic_ledger::AcousticSerial) -> LedgerSerialFact {
    LedgerSerialFact {
        version: serial.version,
        digest: serial.digest.clone(),
        occurrence: Some(span_from_occurrence(&serial.occurrence)),
    }
}

fn seal_fact(seal: &crate::pipeline::acoustic_ledger::LedgerSealReceipt) -> LedgerSealFact {
    LedgerSealFact {
        scope: seal.scope.as_str().to_string(),
        receipt_id: seal.receipt_id.clone(),
        session_id: seal.coverage.session.clone(),
        capture_epoch: seal.coverage.capture_epoch,
        sample_start: seal.coverage.sample_start,
        sample_end: seal.coverage.sample_end,
        sealed_occurrences: seal
            .sealed_occurrences
            .iter()
            .map(span_from_occurrence)
            .collect(),
        serials: seal.serials.iter().map(serial_fact).collect(),
        vad_close_sample: seal.vad_close_sample,
        layer_trail_ordinals: seal
            .layer_trail_ordinals
            .iter()
            .map(|ordinal| *ordinal as u64)
            .collect(),
    }
}

fn coverage_fact(
    coverage: &crate::pipeline::acoustic_ledger::SealCoverageReceipt,
) -> LedgerCoverageFact {
    use crate::pipeline::acoustic_ledger::SealCoverageStatus;
    let unavailable_reason = match coverage.status {
        SealCoverageStatus::Unavailable(gap) => Some(gap.as_str().to_string()),
        SealCoverageStatus::Complete | SealCoverageStatus::Incomplete => None,
    };
    LedgerCoverageFact {
        session_id: coverage.session_id.clone(),
        capture_epoch: coverage.capture_epoch,
        sample_rate_hz: coverage.sample_rate_hz,
        speech_samples: coverage.speech_samples,
        covered_samples: coverage.covered_samples,
        observed_samples: coverage.observed_samples,
        status: coverage.status.as_str().to_string(),
        unavailable_reason,
        speech_producer: coverage.speech_producer.clone(),
        availability: coverage.availability.clone(),
        max_uncovered_samples: coverage.max_uncovered_samples,
        incomplete_threshold_samples: coverage.incomplete_threshold_samples,
        uncovered_speech: coverage
            .uncovered_speech_ranges
            .iter()
            .map(|range| {
                span_from_occurrence(&crate::pipeline::acoustic_ledger::OccurrenceIdentity::from(
                    range,
                ))
            })
            .collect(),
    }
}

fn measured_speech_pct(
    coverage: &crate::pipeline::acoustic_ledger::SealCoverageReceipt,
) -> Option<f32> {
    use crate::pipeline::acoustic_ledger::SealCoverageStatus;
    if matches!(coverage.status, SealCoverageStatus::Unavailable(_)) {
        return None;
    }
    let observed = coverage.observed_samples.filter(|count| *count > 0)?;
    Some((coverage.speech_samples as f64 * 100.0 / observed as f64) as f32)
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
        assert!((truth.vad_speech_pct.expect("measured VAD") - 61.714287).abs() < 1e-6);
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
            vad_speech_pct: Some(61.714287),
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
                confidence: None,
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
                decisions: Vec::new(),
                seals: Vec::new(),
                coverage: None,
                terminal: None,
                shapings: Vec::new(),
                manual_revisions: Vec::new(),
                manual_edits: Vec::new(),
            }),
            generated_at: Some("2026-09-16T04:00:00+02:00".to_string()),
            session_id: None,
            capture_epoch: None,
            ledger_evidence: None,
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
        assert!((truth.vad_speech_pct.expect("measured VAD") - 42.0).abs() < f32::EPSILON);
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
                    confidence: None,
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
        assert!((truth.vad_speech_pct.expect("measured VAD") - 61.714287).abs() < 1e-6);
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
        assert_eq!(
            truth.vad_speech_pct, None,
            "absent VAD measurement stays unavailable"
        );
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
            vad_speech_pct: Some(61.714287),
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
                confidence: None,
                text: "treść".to_string(),
                start_ts: 0.0,
                end_ts: 1.0,
            }],
            words: Vec::new(),
            ledger: None,
            generated_at: Some("2026-09-16T04:00:00+02:00".to_string()),
            session_id: None,
            capture_epoch: None,
            ledger_evidence: None,
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

#[cfg(test)]
mod private_observer_receipt_controls {
    use super::*;
    use crate::pipeline::acoustic_ledger::{
        AcousticEvidence, AcousticEvidenceGap, AcousticLedger, EnergyCalibration,
        IncrementalShapingInput, ObservationIdentity, ObservationProducer, OccurrenceIdentity,
        SealCoverageReceipt, SealCoverageStatus, WordPin as AcousticWordPin,
    };

    fn add_occurrence(
        ledger: &mut AcousticLedger,
        session: &str,
        epoch: u64,
        start: u64,
        label: &str,
        pinned: bool,
    ) -> OccurrenceIdentity {
        let occurrence = OccurrenceIdentity::new(session, epoch, start, start + 16_000);
        let calibration = EnergyCalibration::new("observer-fixture", 1.0, 1);
        let evidence = AcousticEvidence {
            occurrence: occurrence.clone(),
            duration_ms: 1_000.0,
            energy_integral: 100.0,
            mean_rms_dbfs: -20.0,
            peak_dbfs: -10.0,
            vad_open_sample: Some(start),
            vad_close_sample: Some(start + 16_000),
            evidence_calibration_version: calibration.version.clone(),
        };
        assert!(ledger.qualify(&evidence, &calibration).is_qualified());
        ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Whisper]);
        let observation =
            ObservationIdentity::new(ObservationProducer::Whisper, 7, 1, occurrence.clone());
        let receipt = if pinned {
            ledger.admit_word_slots(
                &observation,
                &[AcousticWordPin::new(start + 1_000, start + 14_000, label)],
            )
        } else {
            ledger.admit(&observation, label)
        };
        assert!(
            receipt.grants_mutation(),
            "fixture must authenticate a real admission: {receipt:?}"
        );
        occurrence
    }

    fn measured_coverage(
        session: &str,
        epoch: u64,
        speech: u64,
        observed: u64,
    ) -> SealCoverageReceipt {
        SealCoverageReceipt {
            session_id: session.to_string(),
            capture_epoch: epoch,
            sample_rate_hz: Some(16_000),
            speech_samples: speech,
            covered_samples: speech,
            uncovered_speech_ranges: Vec::new(),
            max_uncovered_samples: 0,
            incomplete_threshold_samples: 100,
            status: SealCoverageStatus::Complete,
            speech_producer: "capture_energy".to_string(),
            availability: "observed".to_string(),
            observed_samples: Some(observed),
        }
    }

    #[test]
    fn private_observer_keeps_five_equal_word_pins_and_original_receipt_identity() {
        let mut ledger = AcousticLedger::new();
        let occurrences: Vec<_> = (0..5)
            .map(|i| add_occurrence(&mut ledger, "five-iwo", 9, i * 16_000, "Iwo", true))
            .collect();
        let original_trail = ledger.layer_trail().to_vec();
        let original_words: Vec<_> = occurrences
            .iter()
            .map(|o| ledger.slots_of(o).unwrap().to_vec())
            .collect();
        let original_text = ledger.rendered_text();
        let original_finality = ledger.terminal_finality("five-iwo", 9);
        let truth = TakeTruth::observe_ledger(&ledger, "five-iwo", 9);
        assert_eq!(truth.session_id.as_deref(), Some("five-iwo"));
        assert_eq!(truth.capture_epoch, Some(9));
        assert_eq!(
            truth.words.len(),
            5,
            "equal words on different PCM pins must never deduplicate"
        );
        for (i, word) in truth.words.iter().enumerate() {
            assert_eq!(word.word, "Iwo");
            assert_eq!(word.sample_start, i as u64 * 16_000 + 1_000);
            assert_eq!(word.sample_end, i as u64 * 16_000 + 14_000);
            assert_eq!(word.pin, "whisper");
        }
        let observed = truth.ledger.as_ref().unwrap();
        assert_eq!(observed.occurrences, 5);
        assert!(!observed.sealed);
        assert_eq!(observed.decisions.len(), original_trail.len());
        for (actual, original) in observed.decisions.iter().zip(&original_trail) {
            assert_eq!(actual.receipt_id, original.receipt_id);
            assert_eq!(actual.ordinal, original.ordinal as u64);
            assert_eq!(actual.decision, original.decision.as_str());
            assert_eq!(actual.serials.len(), original.serials.len());
            for (exported, serial) in actual.serials.iter().zip(&original.serials) {
                assert_eq!(exported.version, serial.version);
                assert_eq!(exported.digest, serial.digest);
            }
        }
        assert!(truth.vad_speech_pct.is_none());
        assert!(truth.avg_logprob.is_none());
        assert!(truth.engine.is_empty());
        assert_eq!(ledger.layer_trail(), original_trail);
        assert_eq!(ledger.rendered_text(), original_text);
        assert_eq!(ledger.terminal_finality("five-iwo", 9), original_finality);
        for (occ, slots) in occurrences.iter().zip(original_words) {
            assert_eq!(ledger.slots_of(occ).unwrap(), slots);
        }
    }

    #[test]
    fn private_observer_filters_session_and_epoch_without_promoting_whole_labels() {
        let mut ledger = AcousticLedger::new();
        add_occurrence(
            &mut ledger,
            "current",
            2,
            0,
            "coarse phrase contains multiple words",
            false,
        );
        add_occurrence(&mut ledger, "current", 1, 20_000, "old epoch", true);
        add_occurrence(&mut ledger, "next-take", 2, 40_000, "other session", true);
        let truth = TakeTruth::observe_ledger(&ledger, "current", 2);
        assert!(
            truth.words.is_empty(),
            "a whole-label range is not an individual word pin"
        );
        let inventory = truth.ledger.unwrap();
        assert_eq!(inventory.occurrences, 1);
        assert!(!inventory.decisions.is_empty());
        assert!(
            inventory
                .decisions
                .iter()
                .all(|entry| entry.session_id == "current" && entry.capture_epoch == 2)
        );
        assert!(!inventory.sealed);
        assert!(inventory.seals.is_empty());
    }

    #[test]
    fn private_observer_keeps_unknown_measurement_distinct_from_measured_silence() {
        let mut ledger = AcousticLedger::new();
        let absent = TakeTruth::observe_ledger(&ledger, "quiet", 3);
        assert_eq!(absent.vad_speech_pct, None);
        assert_eq!(
            serde_json::to_value(&absent).unwrap()["vad_speech_pct"],
            serde_json::Value::Null
        );
        assert!(ledger.record_seal_coverage(measured_coverage("quiet", 3, 0, 32_000)));
        let measured = TakeTruth::observe_ledger(&ledger, "quiet", 3);
        assert_eq!(measured.vad_speech_pct, Some(0.0));
        assert_eq!(
            measured
                .ledger
                .as_ref()
                .unwrap()
                .terminal
                .as_ref()
                .unwrap()
                .status,
            "observed_silence"
        );
        assert!(
            !measured.ledger.as_ref().unwrap().sealed,
            "measured silence is not a minted terminal seal"
        );
        let mut unavailable = measured_coverage("quiet", 3, 0, 32_000);
        unavailable.status = SealCoverageStatus::Unavailable(AcousticEvidenceGap::NotObserved);
        unavailable.observed_samples = None;
        assert!(ledger.record_seal_coverage(unavailable));
        let unavailable = TakeTruth::observe_ledger(&ledger, "quiet", 3);
        assert_eq!(unavailable.vad_speech_pct, None);
        let coverage = unavailable.ledger.unwrap().coverage.unwrap();
        assert_eq!(coverage.status, "unavailable");
        assert_eq!(coverage.unavailable_reason.as_deref(), Some("not_observed"));
    }

    #[test]
    fn private_observer_copies_issued_terminal_and_light_plus_receipts_without_mutation() {
        let mut ledger = AcousticLedger::new();
        let occ = add_occurrence(&mut ledger, "sealed", 5, 0, "gotowe", true);
        assert!(ledger.record_seal_coverage(measured_coverage("sealed", 5, 16_000, 32_000)));
        assert!(ledger.note_frontier_return(&occ, ObservationProducer::Whisper));
        let seal = ledger.seal_terminal("sealed", 5).unwrap();
        let shaped = crate::pipeline::light_plus::apply_live_span("", "gotowe", true);
        assert_ne!(shaped, "gotowe");
        let shaping = ledger
            .record_incremental_shaping(IncrementalShapingInput {
                session_id: "sealed",
                source_revision: 3,
                revision: 4,
                occurrence: &occ,
                source_label: "gotowe",
                left_context: "",
                shaped_text: &shaped,
                sentence_break_before: true,
            })
            .unwrap();
        let before_trail = ledger.layer_trail().to_vec();
        let before_finality = ledger.terminal_finality("sealed", 5);
        let truth = TakeTruth::observe_ledger(&ledger, "sealed", 5);
        let inventory = truth.ledger.unwrap();
        assert!(inventory.sealed);
        assert_eq!(truth.vad_speech_pct, Some(50.0));
        assert!(
            inventory
                .seals
                .iter()
                .any(|exported| exported.receipt_id == seal.receipt_id
                    && exported.scope == seal.scope.as_str())
        );
        assert_eq!(inventory.shapings.len(), 1);
        assert_eq!(inventory.shapings[0].receipt_id, shaping.receipt_id);
        assert_eq!(
            inventory.shapings[0].source_seal_receipt,
            shaping.source_seal_receipt
        );
        assert_eq!(
            inventory.shapings[0].left_context_sha256,
            shaping.left_context_sha256
        );
        assert_eq!(ledger.layer_trail(), before_trail);
        assert_eq!(ledger.terminal_finality("sealed", 5), before_finality);
        assert_eq!(ledger.incremental_shapings(), [shaping]);
        assert_eq!(ledger.text_of(&occ), Some("gotowe"));
    }
    #[test]
    fn private_observer_exports_the_observation_request_and_generation() {
        let mut ledger = AcousticLedger::new();
        add_occurrence(&mut ledger, "request-chain", 11, 0, "word", true);
        let original = ledger.layer_trail().last().unwrap();
        let truth = TakeTruth::observe_ledger(&ledger, "request-chain", 11);
        let value = serde_json::to_value(truth).unwrap();
        let decision = value["ledger"]["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["receipt_id"] == original.receipt_id)
            .unwrap();
        assert_eq!(
            decision["request"].as_u64(),
            Some(original.observation.request),
            "receipt without its original request loses observation identity"
        );
        assert_eq!(
            decision["generation"].as_u64(),
            Some(original.observation.generation),
            "generation must be copied, not guessed from ordinal"
        );
    }

    #[test]
    fn private_observer_serial_facts_keep_their_original_capture_coordinates() {
        let mut ledger = AcousticLedger::new();
        let occurrence = add_occurrence(&mut ledger, "serial-owner", 11, 0, "word", true);
        let truth = TakeTruth::observe_ledger(&ledger, "serial-owner", 11);
        let value = serde_json::to_value(truth).unwrap();
        let serial = &value["ledger"]["decisions"][0]["serials"][0];
        let original = ledger.serial_of(&occurrence).unwrap();
        assert_eq!(serial["digest"], original.digest);
        // Named coordinates or the existing canonical six-part serial wire
        // both preserve the original serial identity without new authority.
        let canonical = format!(
            "v{}:{}:{}:{}:{}:{}",
            original.version,
            original.digest,
            original.occurrence.session,
            original.occurrence.capture_epoch,
            original.occurrence.sample_start,
            original.occurrence.sample_end
        );
        let coordinates = &serial["occurrence"];
        let explicit = coordinates["session_id"].as_str()
            == Some(original.occurrence.session.as_str())
            && coordinates["capture_epoch"].as_u64() == Some(original.occurrence.capture_epoch)
            && coordinates["sample_start"].as_u64() == Some(original.occurrence.sample_start)
            && coordinates["sample_end"].as_u64() == Some(original.occurrence.sample_end);
        assert!(
            explicit
                || serial
                    .as_object()
                    .unwrap()
                    .values()
                    .any(|v| v.as_str() == Some(canonical.as_str())),
            "serial evidence lost its capture coordinate; digest alone is not the canonical receipt"
        );
    }
}

#[cfg(all(test, unix))]
mod private_truth_rebind_controls {
    use super::*;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{MetadataExt, symlink};

    #[test]
    fn private_truth_writer_stays_in_its_pinned_directory_after_path_rebind() {
        let dir = tempfile::tempdir_in("/private/tmp").unwrap();
        let parent = dir.path().join("retained");
        let detached = dir.path().join("detached");
        let outside = dir.path().join("other-owned-directory");
        fs::create_dir(&parent).unwrap();
        fs::create_dir(&outside).unwrap();
        let audio = parent.join("take.wav");
        fs::write(&audio, b"whole capture").unwrap();
        let truth: TakeTruth = serde_json::from_str(include_str!(
            "../../tests/fixtures/truth_sidecar_20260421.json"
        ))
        .unwrap();
        let old_card = truth_sidecar_path(&outside.join("take.wav"));
        let outside_bytes = serde_json::to_vec_pretty(&truth).unwrap();
        fs::write(&old_card, &outside_bytes).unwrap();
        let lock = File::open(&parent).expect("synthetic held directory");
        assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
        let witness = lock.metadata().unwrap();
        let before = matching_directory_descriptors(witness.dev(), witness.ino());
        let mut candidate = truth.clone();
        candidate.engine = "writer waiting on admitted directory".to_string();
        let thread = std::thread::spawn(move || write_truth_sidecar(&audio, &candidate));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while matching_directory_descriptors(witness.dev(), witness.ino()) <= before {
            if thread.is_finished() || std::time::Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let admitted = matching_directory_descriptors(witness.dev(), witness.ino()) > before;
        if admitted {
            fs::rename(&parent, &detached).unwrap();
            symlink(&outside, &parent).unwrap();
        }
        assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_UN) }, 0);
        let result = thread.join().unwrap();
        assert!(
            admitted,
            "writer did not reach the pinned-directory lock: {result:?}"
        );
        assert_eq!(
            fs::read(&old_card).unwrap(),
            outside_bytes,
            "path rebound while writer was locked: unrelated evidence overwritten"
        );
        assert_eq!(
            fs::read(detached.join("take.wav")).unwrap(),
            b"whole capture"
        );
        assert!(
            result.is_err() || read_truth_sidecar(&detached.join("take.wav")).is_ok(),
            "successful publication must remain beside admitted source"
        );
    }

    fn matching_directory_descriptors(dev: u64, ino: u64) -> usize {
        (0..1024)
            .filter(|fd| {
                let mut info: libc::stat = unsafe { std::mem::zeroed() };
                unsafe {
                    libc::fstat(*fd, &mut info) == 0
                        && info.st_dev as u64 == dev
                        && info.st_ino == ino
                }
            })
            .count()
    }
}

#[cfg(all(test, unix))]
mod private_held_directory_lock_controls {
    use super::*;
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    #[test]
    fn private_held_directory_publications_serialize_before_ownership_read() {
        let tmp = tempfile::tempdir().expect("synthetic directory");
        let directory = Arc::new(admit_truth_directory(tmp.path()).expect("admit directory"));
        let first = TruthDirLock::acquire_held(&directory).expect("first transaction");
        let (ready_tx, ready_rx) = mpsc::channel();
        let (entered_tx, entered_rx) = mpsc::channel();
        let peer = Arc::clone(&directory);
        let worker = std::thread::spawn(move || {
            ready_tx.send(()).expect("ready");
            let second = TruthDirLock::acquire_held(&peer).expect("second transaction");
            entered_tx.send(()).expect("entered");
            drop(second);
        });
        ready_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("scheduled peer");
        let blocked = matches!(
            entered_rx.recv_timeout(Duration::from_millis(250)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        drop(first);
        if blocked {
            entered_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("peer proceeds after release");
        }
        worker.join().expect("peer joins");
        assert!(
            blocked,
            "a second held-directory writer entered while the first ownership transaction was still locked"
        );
    }
}
