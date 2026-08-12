//! Pipeline contracts — shared data types for the transcription pipeline.
//!
//! These types define the boundaries between pipeline stages:
//!   AudioChunk → SpeechUtterance → RawTranscript → TranscriptDelta → DeltaSink
//!
//! Vibecrafted with AI Agents by Vetcoders (c)2026 Vetcoders

use serde::{Deserialize, Serialize};

// ═══════════════════════════════════════════════════════════
// Audio stage
// ═══════════════════════════════════════════════════════════

/// A chunk of raw audio samples from the recorder.
#[derive(Debug, Clone)]
pub struct AudioChunk {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    /// Start time relative to recording session (seconds).
    pub start_ts: f32,
    /// End time relative to recording session (seconds).
    pub end_ts: f32,
}

/// A complete speech utterance (after VAD gating / silence detection).
#[derive(Debug, Clone)]
pub struct SpeechUtterance {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub start_ts: f32,
    pub end_ts: f32,
}

impl SpeechUtterance {
    /// Duration in seconds.
    pub fn duration(&self) -> f32 {
        self.end_ts - self.start_ts
    }
}

// ═══════════════════════════════════════════════════════════
// STT stage
// ═══════════════════════════════════════════════════════════

/// Raw output from a speech-to-text engine (Whisper or future providers).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RawTranscript {
    /// The transcribed text (untouched by postprocessing).
    pub text: String,
    /// Per-segment breakdown, if the engine provides it.
    pub segments: Vec<TranscriptSegment>,
    /// Average log-probability across decoded tokens (lower = less confident).
    pub avg_logprob: Option<f32>,
    /// Compression ratio of the decoded text (high = repetitive/hallucinated).
    pub compression_ratio: Option<f32>,
    /// True when the quality gate (logprob + compression) dropped this result.
    pub quality_gate_dropped: bool,
}

/// A single segment from the STT engine (optional granularity).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptSegment {
    pub text: String,
    pub start_ts: f32,
    pub end_ts: f32,
}

/// Explicit options for file-based transcription.
///
/// This keeps final-pass behavior requestable instead of hiding cleanup behind
/// a plain-text helper.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileTranscriptionOptions {
    pub final_pass: FinalPassMode,
}

/// Optional final-pass behavior for file-level transcription.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinalPassMode {
    #[default]
    None,
    EmbeddedLexiconCleanup,
}

impl std::fmt::Display for FinalPassMode {
    /// Wire-token Display for [`FinalPassMode`] (snake_case for logs/UI).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => write!(f, "none"),
            Self::EmbeddedLexiconCleanup => write!(f, "embedded_lexicon_cleanup"),
        }
    }
}

/// What the requested final pass actually did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinalPassDisposition {
    Skipped,
    Unchanged,
    Changed,
    Rejected,
    Dropped,
}

impl std::fmt::Display for FinalPassDisposition {
    /// Wire-token Display for [`FinalPassDisposition`] (snake_case for logs/UI).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Skipped => write!(f, "skipped"),
            Self::Unchanged => write!(f, "unchanged"),
            Self::Changed => write!(f, "changed"),
            Self::Rejected => write!(f, "rejected"),
            Self::Dropped => write!(f, "dropped"),
        }
    }
}

/// Provenance for an explicitly requested file-level final pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinalPassVerdict {
    pub mode: FinalPassMode,
    pub disposition: FinalPassDisposition,
    pub reason: Option<String>,
    pub lexicon_rewrites: u64,
    pub repetition_cleanups: u64,
}

/// VAD speech-% threshold at/below which [`TranscriptionConfidenceFlag::VeryLowSpeech`] fires.
const VERY_LOW_SPEECH_PCT: f32 = 6.0;
/// Avg logprob ceiling for [`TranscriptionConfidenceFlag::PossibleHallucinationLogprob`].
const POSSIBLE_HALLUCINATION_LOGPROB: f32 = -1.0;

/// Engine-owned confidence flags derived from VAD + Whisper quality metadata,
/// plus app-level provenance flags surfaced through the controller truth
/// adjudicator. Kept as a single enum so downstream consumers (UI, sidecar
/// `truth.json`, QA tooling) always receive a typed value instead of a string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionConfidenceFlag {
    // ── Engine-owned (derived inside the transcription engine) ──
    VeryLowSpeech,
    PossibleHallucinationLogprob,
    QualityGateDropped,
    /// Silero-based post-filter dropped one or more Whisper segments
    /// that fell inside classified trailing silence.
    SileroDroppedTailHallucinations {
        count: u32,
    },

    // ── App-level provenance (surfaced by controller truth adjudication) ──
    /// Hold path attempted a final-pass against the saved WAV but the
    /// local Whisper path declined to run or produced no verdict.
    LocalFinalPassUnavailable,
    /// Truth-surface committed a cloud transcript after the local path
    /// failed to deliver a usable verdict.
    CloudFallbackUsed,
    /// Truth-surface committed the streaming preview text as the final
    /// verdict (no final-pass available).
    StreamingPreviewUsedAsVerdict,
    /// Streaming text was exposed as a verdict surface before any explicit
    /// final-pass adjudication ran.
    UnverifiedStream,
    /// Cloud was the primary transcript source but the cloud call did
    /// not return a usable transcript (empty or error).
    CloudPrimaryMissing,
    /// AI formatting pass ran but the emitted text is effectively the
    /// raw input (no edit applied) — status should not read "Applied".
    AiNoopDetected,
    /// File-level final pass produced text that catastrophically regressed
    /// versus the live streaming assembly (e.g. Apple SFSpeech 12 chars vs
    /// multi-utterance stream). Adjudicator kept the stream as floor of truth.
    FinalPassLengthRegression,
}

impl std::fmt::Display for TranscriptionConfidenceFlag {
    /// Wire-token Display for confidence flags; structured variants encode payload.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::VeryLowSpeech => write!(f, "very_low_speech"),
            Self::PossibleHallucinationLogprob => {
                write!(f, "possible_hallucination_logprob")
            }
            Self::QualityGateDropped => write!(f, "quality_gate_dropped"),
            Self::SileroDroppedTailHallucinations { count } => {
                write!(f, "silero_dropped_tail_hallucinations:{count}")
            }
            Self::LocalFinalPassUnavailable => write!(f, "local_final_pass_unavailable"),
            Self::CloudFallbackUsed => write!(f, "cloud_fallback_used"),
            Self::StreamingPreviewUsedAsVerdict => {
                write!(f, "streaming_preview_used_as_verdict")
            }
            Self::UnverifiedStream => write!(f, "unverified_stream"),
            Self::CloudPrimaryMissing => write!(f, "cloud_primary_missing"),
            Self::AiNoopDetected => write!(f, "ai_noop_detected"),
            Self::FinalPassLengthRegression => write!(f, "final_pass_length_regression"),
        }
    }
}

/// Whether a non-empty final-pass transcript is a catastrophic length regression
/// vs the live streaming assembly.
///
/// Product rule (div0 log 2026-07-23): Apple file STT can return ~12 chars on a
/// ~98s recording while the live assembly held a longer stream. Streaming is the
/// floor of truth; final may only replace it when it is not a collapse.
///
/// - Streams shorter than [`FINAL_PASS_REGRESSION_MIN_STREAM_CHARS`] never trigger
///   regression (preserves cold-start recovery when live was empty/thin).
/// - Otherwise final wins only if it keeps at least 40% of stream char count.
pub const FINAL_PASS_REGRESSION_MIN_STREAM_CHARS: usize = 24;

/// Evaluate the regression rule documented on
/// [`FINAL_PASS_REGRESSION_MIN_STREAM_CHARS`].
///
/// Returns `true` when `final_text` must be rejected in favour of
/// `streaming_text`. Both sides are compared trimmed and by char count, so
/// multi-byte transcripts are not penalised for byte length.
pub fn final_pass_is_length_regression(final_text: &str, streaming_text: &str) -> bool {
    let final_chars = final_text.trim().chars().count();
    let stream_chars = streaming_text.trim().chars().count();
    if stream_chars < FINAL_PASS_REGRESSION_MIN_STREAM_CHARS {
        return false;
    }
    if final_chars == 0 {
        return true;
    }
    // final < 0.4 * stream  ⇔  final * 5 < stream * 2
    final_chars.saturating_mul(5) < stream_chars.saturating_mul(2)
}

// ═══════════════════════════════════════════════════════════
// File-level transcription verdict
// ═══════════════════════════════════════════════════════════

/// Structured verdict from file-level transcription.
///
/// Carries the full truth the engine knows: text, VAD stats, explicit
/// no-speech reason, confidence flags, and provenance. Consumers decide what
/// to expose; nothing is hidden.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionVerdict {
    pub text: String,
    pub raw: RawTranscript,
    pub vad: Option<VadVerdict>,
    pub source: TranscriptionSource,
    pub engine: TranscriptionEngineVerdict,
    pub final_pass: Option<FinalPassVerdict>,
    pub confidence_flags: Vec<TranscriptionConfidenceFlag>,
}

impl TranscriptionVerdict {
    /// Build a verdict and materialize engine-owned confidence flags once at the
    /// API boundary so downstream consumers do not have to recreate heuristics.
    pub fn from_parts(
        text: String,
        raw: RawTranscript,
        vad: Option<VadVerdict>,
        source: TranscriptionSource,
        engine: TranscriptionEngineVerdict,
        final_pass: Option<FinalPassVerdict>,
    ) -> Self {
        let confidence_flags = collect_confidence_flags(
            vad.as_ref().map(|vad| vad.speech_pct),
            raw.avg_logprob,
            raw.quality_gate_dropped,
        );
        Self {
            text,
            raw,
            vad,
            source,
            engine,
            final_pass,
            confidence_flags,
        }
    }

    /// Build a verdict and append typed Silero drop telemetry when the
    /// file-level post-filter removed tail hallucinations.
    pub fn from_parts_with_silero_drops(
        text: String,
        raw: RawTranscript,
        vad: Option<VadVerdict>,
        source: TranscriptionSource,
        engine: TranscriptionEngineVerdict,
        final_pass: Option<FinalPassVerdict>,
        tail_drop_count: u32,
    ) -> Self {
        let mut verdict = Self::from_parts(text, raw, vad, source, engine, final_pass);
        if tail_drop_count > 0 {
            verdict.confidence_flags.push(
                TranscriptionConfidenceFlag::SileroDroppedTailHallucinations {
                    count: tail_drop_count,
                },
            );
        }
        verdict
    }
}

/// VAD analysis results preserved as data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VadVerdict {
    /// Percentage of audio classified as speech (0–100).
    pub speech_pct: f32,
    pub speech_windows: usize,
    pub total_windows: usize,
    /// True when VAD found no speech at all.
    pub no_speech: bool,
    /// Structured reason preserved when VAD concluded with no usable speech.
    pub no_speech_reason: Option<String>,
    /// Sparkline visualisation of speech distribution (one char per 500ms window).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sparkline: String,
}

/// Per-window silence semantics derived from Silero probabilities.
///
/// Window granularity matches `vad::extract_speech` (500ms buckets) so the
/// file-level Whisper post-filter can compare transcript timestamps against a
/// typed VAD timeline without trimming the decoded audio first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VadClass {
    Speech,
    UtteranceGap,
    SentenceBoundary,
    TrailingSilence,
}

impl std::fmt::Display for VadClass {
    /// Wire-token Display for [`VadClass`] (matches serde `snake_case` tokens).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Speech => write!(f, "speech"),
            Self::UtteranceGap => write!(f, "utterance_gap"),
            Self::SentenceBoundary => write!(f, "sentence_boundary"),
            Self::TrailingSilence => write!(f, "trailing_silence"),
        }
    }
}

/// Where the transcription text came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionSource {
    /// Final-pass local Whisper on saved WAV file.
    LocalFinalPass,
    /// Live streaming transcription (draft).
    Streaming,
    /// Cloud STT provider.
    Cloud,
    /// Degraded fallback path (cloud failed, streaming used).
    Fallback,
}

impl std::fmt::Display for TranscriptionSource {
    /// Wire-token Display for [`TranscriptionSource`] provenance labels.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LocalFinalPass => write!(f, "local_final_pass"),
            Self::Streaming => write!(f, "streaming"),
            Self::Cloud => write!(f, "cloud"),
            Self::Fallback => write!(f, "fallback"),
        }
    }
}

/// Which transcription engine produced the final verdict text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionEngine {
    Whisper,
    /// Apple on-device speech (SpeechTranscriber and/or SFSpeechRecognizer).
    Apple,
}

impl std::fmt::Display for TranscriptionEngine {
    /// Wire-token Display for [`TranscriptionEngine`] (`whisper` / `apple`).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Whisper => write!(f, "whisper"),
            Self::Apple => write!(f, "apple"),
        }
    }
}

/// How the active engine was provisioned at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionEngineMode {
    EmbeddedDefault,
    RuntimeFallback,
    /// Apple SpeechAnalyzer / SpeechTranscriber path (locales ST supports).
    SpeechTranscriber,
    /// Apple SFSpeechRecognizer on-device path (locales ST lacks, e.g. pl-PL).
    SfSpeechOnDevice,
    /// Apple SpeechAnalyzer / DictationTranscriber path — the system-dictation
    /// model family, which *does* serve pl-PL. Opt-in PoC lane (W4-A); kept a
    /// distinct mode so a verdict never claims ST provenance for DT text.
    DictationTranscriber,
}

impl std::fmt::Display for TranscriptionEngineMode {
    /// Wire-token Display for [`TranscriptionEngineMode`] provisioning path.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmbeddedDefault => write!(f, "embedded_default"),
            Self::RuntimeFallback => write!(f, "runtime_fallback"),
            Self::SpeechTranscriber => write!(f, "speech_transcriber"),
            Self::SfSpeechOnDevice => write!(f, "sf_speech_on_device"),
            Self::DictationTranscriber => write!(f, "dictation_transcriber"),
        }
    }
}

/// Engine-level provenance preserved with the file transcription verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptionEngineVerdict {
    pub engine: TranscriptionEngine,
    pub mode: TranscriptionEngineMode,
    pub fallback_used: bool,
}

impl TranscriptionEngineVerdict {
    /// Whisper provenance. `fallback_used` is derived, not passed: only
    /// [`TranscriptionEngineMode::RuntimeFallback`] counts as a fallback, so a
    /// caller cannot claim a clean embedded run for a degraded one.
    pub const fn whisper(mode: TranscriptionEngineMode) -> Self {
        Self {
            engine: TranscriptionEngine::Whisper,
            mode,
            fallback_used: matches!(mode, TranscriptionEngineMode::RuntimeFallback),
        }
    }

    /// Apple on-device provenance (SpeechTranscriber / SFSpeechRecognizer /
    /// DictationTranscriber). Never a fallback — the Apple lane is selected
    /// deliberately by the router, never reached by degradation.
    pub const fn apple(mode: TranscriptionEngineMode) -> Self {
        Self {
            engine: TranscriptionEngine::Apple,
            mode,
            fallback_used: false,
        }
    }
}

/// Derive the engine-owned subset of [`TranscriptionConfidenceFlag`] from raw
/// VAD + decoder metadata.
///
/// Single owner of the thresholds (`VERY_LOW_SPEECH_PCT`,
/// `POSSIBLE_HALLUCINATION_LOGPROB`) so no consumer re-implements the
/// heuristic. Missing inputs (`None`) never produce a flag — absence of
/// evidence is not evidence of low confidence. App-level provenance flags are
/// appended later by the controller truth adjudicator, not here.
pub(crate) fn collect_confidence_flags(
    vad_speech_pct: Option<f32>,
    avg_logprob: Option<f32>,
    quality_gate_dropped: bool,
) -> Vec<TranscriptionConfidenceFlag> {
    let mut flags = Vec::new();

    if vad_speech_pct.is_some_and(|speech_pct| speech_pct <= VERY_LOW_SPEECH_PCT) {
        flags.push(TranscriptionConfidenceFlag::VeryLowSpeech);
    }

    if avg_logprob.is_some_and(|avg| avg <= POSSIBLE_HALLUCINATION_LOGPROB) {
        flags.push(TranscriptionConfidenceFlag::PossibleHallucinationLogprob);
    }

    if quality_gate_dropped {
        flags.push(TranscriptionConfidenceFlag::QualityGateDropped);
    }

    flags
}

// ═══════════════════════════════════════════════════════════
// Delta / presentation stage
// ═══════════════════════════════════════════════════════════

/// Backspace character used in delta encoding.
pub const BACKSPACE: char = '\u{0008}';

/// An incremental update to the transcript buffer.
///
/// # Hard contract (append + backspace only)
///
/// - The payload is a UTF-8 char stream interpreted left-to-right.
/// - Any non-`\u{0008}` char means append that char to the tail.
/// - `\u{0008}` means remove one char from the current tail (if any).
/// - Backspace underflow is a no-op (never panic, never index by byte).
/// - Producers must emit deltas in-order; consumers must apply in the same order.
///
/// This contract keeps live correction cheap and deterministic without resending
/// full buffers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranscriptDelta {
    pub delta: String,
}

impl TranscriptDelta {
    /// Wrap a raw delta string (may contain backspace chars) without modification.
    pub fn from_raw(delta: impl Into<String>) -> Self {
        Self {
            delta: delta.into(),
        }
    }

    /// Create a simple append-only delta (no backspaces).
    pub fn append(text: impl Into<String>) -> Self {
        Self { delta: text.into() }
    }

    /// Create a delta that replaces the last `delete_count` characters, then appends `new_text`.
    pub fn replace(delete_count: usize, new_text: &str) -> Self {
        let mut delta = String::with_capacity(delete_count + new_text.len());
        for _ in 0..delete_count {
            delta.push(BACKSPACE);
        }
        delta.push_str(new_text);
        Self { delta }
    }

    /// Build a minimal delta from "before" and "after" full texts.
    ///
    /// Finds the common prefix, emits backspaces for the removed tail of `before`,
    /// then appends the new tail of `after`. Returns `None` if texts are identical.
    ///
    /// Output always follows the hard contract: only append chars + backspaces.
    pub fn from_diff(before: &str, after: &str) -> Option<Self> {
        if before == after {
            return None;
        }

        let common_prefix_len = before
            .chars()
            .zip(after.chars())
            .take_while(|(a, b)| a == b)
            .count();

        let delete_count = before.chars().count() - common_prefix_len;
        let new_tail: String = after.chars().skip(common_prefix_len).collect();

        Some(Self::replace(delete_count, &new_tail))
    }

    /// Apply this delta to a mutable string buffer (the inverse of `from_diff`).
    ///
    /// Backspace underflow is intentionally ignored, keeping this operation
    /// idempotent and safe for partial buffers.
    pub fn apply(&self, target: &mut String) {
        for ch in self.delta.chars() {
            if ch == BACKSPACE {
                target.pop();
            } else {
                target.push(ch);
            }
        }
    }

    /// Returns `true` if this delta contains only backspace characters (pure deletion).
    pub fn is_delete_only(&self) -> bool {
        !self.delta.is_empty() && self.delta.chars().all(|c| c == BACKSPACE)
    }

    /// Returns `true` if this delta contains no backspaces (pure append).
    pub fn is_append_only(&self) -> bool {
        !self.delta.is_empty() && self.delta.chars().all(|c| c != BACKSPACE)
    }
}

impl std::fmt::Display for TranscriptDelta {
    /// Display the raw delta payload (includes literal backspace chars).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.delta)
    }
}

// ═══════════════════════════════════════════════════════════
// Traits (adapter boundaries)
// ═══════════════════════════════════════════════════════════

/// Adapter for speech-to-text engines.
///
/// Implementations: `LocalWhisperEngine` (current), future cloud STT providers.
pub trait TranscriptionAdapter: Send + Sync {
    /// Transcribe one VAD-bounded utterance. `language` is a BCP-47-ish hint
    /// (`None` = engine autodetect). Returns the untouched engine output;
    /// postprocessing belongs to later pipeline stages.
    fn transcribe(
        &self,
        utterance: &SpeechUtterance,
        language: Option<&str>,
    ) -> anyhow::Result<RawTranscript>;
}

/// Sink for transcript deltas (UI, IPC, clipboard, etc).
///
/// This decouples the streaming pipeline from presentation concerns.
pub trait DeltaSink: Send + Sync {
    /// Apply one delta to whatever surface this sink owns. Called in producer
    /// order; implementations must not reorder or coalesce across calls, or the
    /// append+backspace contract on [`TranscriptDelta`] breaks.
    fn apply(&self, delta: &TranscriptDelta);
}

// ═══════════════════════════════════════════════════════════
// Engine events (intent layer)
// ═══════════════════════════════════════════════════════════

/// Events emitted by the transcription engine.
///
/// These are semantic events — the engine communicates what happened
/// and why, not how to display it. UI decides presentation.
///
/// Data flow: AudioChunk → VAD → Whisper → PostProcess → EngineEvent
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineEvent {
    /// VAD detected speech start.
    VadStart { speech_prob: f32, ts_ms: u64 },
    /// VAD detected speech end.
    VadEnd { speech_prob: f32, ts_ms: u64 },
    /// Session or utterance completed without usable speech content.
    ///
    /// Emitted when VAD sees no speech at all, or when speech-like segments are
    /// fully rejected by quality gates/hallucination filters.
    NoSpeech { reason: String },

    /// Interim preview — latest transcription of the current utterance.
    ///
    /// # Semantics (contract)
    ///
    /// - `text` is **utterance-local**: it contains the full post-processed text for
    ///   the current utterance only, NOT the accumulated session text.
    /// - On each new Whisper decode, `text` replaces the previous Preview for this
    ///   utterance (not appended). `rev` increments monotonically.
    /// - After `UtteranceFinal`, `text` resets to empty for the next utterance.
    ///
    /// # Sink responsibilities
    ///
    /// - Sinks that need incremental deltas (e.g. overlay append) must track
    ///   `last_preview` and compute diffs themselves (see `TranscriptDelta::from_diff`).
    /// - Sinks that need session-accumulated text must concatenate across utterances.
    /// - On `UtteranceFinal`, sinks must reset their `last_preview` state.
    Preview { rev: u64, text: String },

    /// Correction — re-transcription of accumulated audio improved previous output.
    ///
    /// # Semantics (contract)
    ///
    /// - `text` is the full corrected utterance-local text (replaces, not appends).
    /// - `previous_text` is what was shown before correction and acts as a baseline
    ///   for stale-correction guards in sinks.
    /// - Sinks should apply this as a replacement (delta diff or full overwrite)
    ///   and update their `last_preview` to `text`.
    /// - Must NOT finalize streaming state (keep `is_streaming = true` in UI).
    Correction {
        rev: u64,
        text: String,
        previous_text: String,
    },

    /// Complete utterance (VAD-bounded or flush).
    ///
    /// # Semantics (contract)
    ///
    /// - Emitted once per VAD-bounded speech segment (or on session flush).
    /// - `text` is the final post-processed utterance text.
    /// - `vad_speech_pct` preserves how much of the utterance Silero classified
    ///   as speech, so consumers do not have to reverse-engineer silence risk.
    /// - `confidence_flags` carries the engine-owned truth derived from VAD
    ///   speech ratio plus Whisper quality-gate metadata.
    /// - After this event, the engine clears its internal accumulated_text.
    /// - Sinks must reset `last_preview` to empty (next Preview starts fresh).
    /// - In toggle mode, the utterance callback processes this text (AI/clipboard).
    ///   The commit path should NOT re-write to the user bubble if Preview already
    ///   streamed into it (see `skip_user_bubble`).
    UtteranceFinal {
        utterance_id: u64,
        text: String,
        raw_text: String,
        start_ts: f32,
        end_ts: f32,
        segments: Vec<TranscriptSegment>,
        vad_speech_pct: Option<f32>,
        avg_logprob: Option<f32>,
        compression_ratio: Option<f32>,
        quality_gate_dropped: bool,
        confidence_flags: Vec<TranscriptionConfidenceFlag>,
    },

    /// Replace a bounded char range inside an already-committed utterance.
    ///
    /// This is the ADR "never rewrite from zero" patch primitive for Layers 1,
    /// 2, and 4. `start` and `end` are char offsets within `utterance_id`.
    ReplaceRange {
        utterance_id: u64,
        start: usize,
        end: usize,
        text: String,
        source: LayerSource,
    },

    /// Insert a visible annotation at a char position inside an utterance.
    InsertAnnotation {
        utterance_id: u64,
        position: usize,
        text: String,
        kind: AnnotationKind,
    },

    /// Mark the session text buffer immutable after the final layered pass.
    SessionFinalised {
        session_id: String,
        layer_summary: LayerSummary,
    },

    /// Content dropped by engine intelligence.
    Drop {
        kind: DropKind,
        text: String,
        reason: String,
    },

    /// Session-level statistics (emitted on stop/flush).
    Stats {
        dropped_audio_chunks: u64,
        hallucination_drops: u64,
        semantic_gate_drops: u64,
        filtered_empty_drops: u64,
        corrections_applied: u64,
        total_utterances: u64,
        /// Number of partial-pass refine runs attempted in this session.
        partial_runs_total: u64,
        /// Partial-pass runs triggered by utterance-count threshold.
        trigger_utterance_count: u64,
        /// Partial-pass runs triggered by speech-duration threshold.
        trigger_speech_count: u64,
        /// Partial-pass runs triggered by watchdog fallback.
        trigger_timer_count: u64,
        /// Refinement results suppressed by stale-guard checks.
        partial_stale_count: u64,
        /// Tracked refine runs superseded by newer partial-pass requests.
        partial_coalesced_count: u64,
        /// Partial-pass runs dropped (submit/queue/shutdown paths).
        partial_dropped_count: u64,
    },

    /// Recoverable error — engine continues.
    Warning { code: String, message: String },
}

/// Warning codes that mean the user's action actually FAILED and must surface
/// as an error on the UI (`listener.on_error`).
///
/// Every other warning is a quality receipt — the engine degraded or
/// normalized something and *kept going*. Those must never ride the error
/// channel: the composer treats `on_error` during capture as terminal, so a
/// routine receipt painted "Dictation stopped" over a live session, desynced
/// the toggle parity, and the next toggle started a second, orphaned capture
/// that held the microphone behind an Idle tray (2026-08-12 incident).
pub const USER_TERMINAL_WARNING_CODES: &[&str] = &["transcription_failed"];

/// Whether a [`EngineEvent::Warning`] code is a user-terminal failure
/// (forward to `on_error`) rather than a quality receipt (log only).
pub fn warning_is_user_terminal(code: &str) -> bool {
    USER_TERMINAL_WARNING_CODES.contains(&code)
}

/// Layer that produced a bounded replacement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerSource {
    TailPatch,
    Lexicon,
    InlineLlm,
    FinalBam,
}

/// Kind of annotation inserted into the visible transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationKind {
    HesitationPause,
    Paralingual { label: String },
}

/// Session-end summary for layered transcript mutation telemetry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayerSummary {
    pub tail_patch_replacements: u64,
    pub lexicon_replacements: u64,
    pub inline_llm_replacements: u64,
    pub final_bam_replacements: u64,
    pub annotations_inserted: u64,
}

/// Why a bounded mutation could not be applied to a committed buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptEventApplyError {
    InvalidRange {
        start: usize,
        end: usize,
        len: usize,
    },
}

impl EngineEvent {
    /// Apply ADR-visible bounded mutations to a committed utterance buffer.
    ///
    /// Append/backspace remain represented by `TranscriptDelta`; this helper is
    /// only for range replacement and annotation insertion contracts.
    pub fn apply_to_committed_text(
        &self,
        target: &mut String,
    ) -> Result<bool, TranscriptEventApplyError> {
        match self {
            Self::ReplaceRange {
                start, end, text, ..
            } => {
                let (start_byte, end_byte) = char_range_to_byte_range(target, *start, *end)?;
                target.replace_range(start_byte..end_byte, text);
                Ok(true)
            }
            Self::InsertAnnotation { position, text, .. } => {
                let (position_byte, _) = char_range_to_byte_range(target, *position, *position)?;
                target.insert_str(position_byte, text);
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

/// Translate a validated char range into a byte range safe for
/// `String::replace_range` / `insert_str`.
///
/// Char offsets are the wire contract (Swift and the engine both count
/// characters), byte indices are what `String` needs — mixing them is the
/// classic panic on Polish diacritics. Rejects inverted or out-of-bounds ranges
/// with [`TranscriptEventApplyError::InvalidRange`] instead of panicking.
fn char_range_to_byte_range(
    text: &str,
    start: usize,
    end: usize,
) -> Result<(usize, usize), TranscriptEventApplyError> {
    let len = text.chars().count();
    if start > end || end > len {
        return Err(TranscriptEventApplyError::InvalidRange { start, end, len });
    }

    let start_byte = char_offset_to_byte_index(text, start);
    let end_byte = char_offset_to_byte_index(text, end);
    Ok((start_byte, end_byte))
}

/// Byte index of the `offset`-th char, with `text.len()` for the one-past-the-end
/// position so an empty range at the tail stays representable. Offsets beyond
/// that clamp to `text.len()` rather than panicking; callers are expected to have
/// bounds-checked through [`char_range_to_byte_range`] first.
fn char_offset_to_byte_index(text: &str, offset: usize) -> usize {
    if offset == text.chars().count() {
        return text.len();
    }
    text.char_indices()
        .nth(offset)
        .map(|(byte_index, _)| byte_index)
        .unwrap_or(text.len())
}

/// Why the engine dropped content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DropKind {
    /// Whisper hallucination pattern detected (e.g. "thank you", "subscribe").
    Hallucination,
    /// Semantic gate: chunk too similar to previous output (streaming path only).
    SemanticGate,
    /// Overlap dedup produced empty result.
    OverlapEmpty,
    /// Text was empty after lexicon + cleanup processing (utterance path).
    /// Distinct from `SemanticGate` — no embedding comparison was involved.
    FilteredEmpty,
}

impl std::fmt::Display for DropKind {
    /// Human/debug label for [`DropKind`] (PascalCase, not serde tokens).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DropKind::Hallucination => write!(f, "Hallucination"),
            DropKind::SemanticGate => write!(f, "SemanticGate"),
            DropKind::OverlapEmpty => write!(f, "OverlapEmpty"),
            DropKind::FilteredEmpty => write!(f, "FilteredEmpty"),
        }
    }
}

/// Sink for engine events. Replaces DeltaSink for the unified pipeline.
///
/// Implementations decide how to present events — typing animation,
/// overlay updates, clipboard paste, IPC streaming, etc.
pub trait EventSink: Send + Sync {
    /// Receive one semantic engine event. Called from the engine's own thread,
    /// so implementations must not block — presentation work belongs on the
    /// consumer's queue, not on this call.
    fn on_event(&self, event: &EngineEvent);
}

// ═══════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════

/// Contract tests for the types above: `TranscriptDelta` append/backspace
/// roundtrips (including multi-byte Polish, CJK and emoji), verdict construction
/// and flag derivation, `EngineEvent` clone/apply behaviour, and serde
/// roundtrips that pin the wire tokens every consumer parses.
#[cfg(test)]
mod tests {
    use super::*;

    // ── TranscriptDelta roundtrip ──

    /// Append-only delta extends the buffer without rewriting the prefix.
    #[test]
    fn delta_append_roundtrip() {
        let mut buf = String::from("Hello");
        let delta = TranscriptDelta::append(" world");
        delta.apply(&mut buf);
        assert_eq!(buf, "Hello world");
    }

    /// Replace emits N backspaces then append; applies as a tail rewrite.
    #[test]
    fn delta_replace_roundtrip() {
        let mut buf = String::from("Hello worl");
        // Fix typo: delete "worl" (4 chars), append "world!"
        let delta = TranscriptDelta::replace(4, "world!");
        delta.apply(&mut buf);
        assert_eq!(buf, "Hello world!");
    }

    /// `from_diff` + `apply` roundtrips a mid-string Polish correction.
    #[test]
    fn delta_from_diff_roundtrip() {
        let before = "Cześć, jestem lekarzem";
        let after = "Cześć, jestem weterynarzem";

        let delta = TranscriptDelta::from_diff(before, after).unwrap();
        let mut buf = before.to_string();
        delta.apply(&mut buf);
        assert_eq!(buf, after);
    }

    /// Identical before/after yields `None` (no empty delta noise).
    #[test]
    fn delta_from_diff_identical_returns_none() {
        assert!(TranscriptDelta::from_diff("same", "same").is_none());
    }

    /// Full replacement when common prefix is empty still roundtrips.
    #[test]
    fn delta_from_diff_complete_replacement() {
        let before = "abc";
        let after = "xyz";
        let delta = TranscriptDelta::from_diff(before, after).unwrap();
        let mut buf = before.to_string();
        delta.apply(&mut buf);
        assert_eq!(buf, after);
    }

    /// Empty→text is pure append (`is_append_only`).
    #[test]
    fn delta_from_diff_empty_to_text() {
        let delta = TranscriptDelta::from_diff("", "hello").unwrap();
        let mut buf = String::new();
        delta.apply(&mut buf);
        assert_eq!(buf, "hello");
        assert!(delta.is_append_only());
    }

    /// Text→empty is pure delete (`is_delete_only`).
    #[test]
    fn delta_from_diff_text_to_empty() {
        let delta = TranscriptDelta::from_diff("hello", "").unwrap();
        let mut buf = String::from("hello");
        delta.apply(&mut buf);
        assert_eq!(buf, "");
        assert!(delta.is_delete_only());
    }

    /// Backspace on empty buffer is a no-op (never panics).
    #[test]
    fn delta_apply_backspace_underflow_is_noop() {
        let mut buf = String::new();
        TranscriptDelta::from_raw("\u{0008}\u{0008}abc").apply(&mut buf);
        assert_eq!(buf, "abc");
    }

    /// Multi-byte Polish diacritics use char counts, not bytes.
    #[test]
    fn delta_from_diff_unicode_polish() {
        let before = "Zółty pies";
        let after = "Żółty pies";
        let delta = TranscriptDelta::from_diff(before, after).unwrap();
        let mut buf = before.to_string();
        delta.apply(&mut buf);
        assert_eq!(buf, after);
    }

    /// Streaming-style append of a completion suffix on a shared stem.
    #[test]
    fn delta_backspace_sequence() {
        // Simulate Whisper correcting "transkryp" → "transkrypcja"
        let mut buf = String::from("transkryp");
        let delta = TranscriptDelta::append("cja");
        delta.apply(&mut buf);
        assert_eq!(buf, "transkrypcja");
    }

    /// Multi-chunk append/replace simulation mirrors live Whisper previews.
    #[test]
    fn delta_multi_step_simulation() {
        // Simulate streaming: chunk1 → chunk2 (with correction) → chunk3
        let mut buf = String::new();

        // Chunk 1: "Witaj "
        TranscriptDelta::append("Witaj ").apply(&mut buf);
        assert_eq!(buf, "Witaj ");

        // Chunk 2: Whisper corrects to "Witaj, " (backspace space, add ", ")
        TranscriptDelta::replace(1, ", ").apply(&mut buf);
        assert_eq!(buf, "Witaj, ");

        // Chunk 3: append "świecie!"
        TranscriptDelta::append("świecie!").apply(&mut buf);
        assert_eq!(buf, "Witaj, świecie!");
    }

    /// CJK appends are char-safe and remain append-only.
    #[test]
    fn delta_from_diff_multibyte_cjk() {
        let before = "日本語テスト";
        let after = "日本語テスト結果";
        let delta = TranscriptDelta::from_diff(before, after).unwrap();
        let mut buf = before.to_string();
        delta.apply(&mut buf);
        assert_eq!(buf, after);
        assert!(delta.is_append_only());
    }

    /// Emoji replacement deletes one char then appends (grapheme as char).
    #[test]
    fn delta_from_diff_emoji_replacement() {
        let before = "Hello 🌍";
        let after = "Hello 🌎";
        let delta = TranscriptDelta::from_diff(before, after).unwrap();
        let mut buf = before.to_string();
        delta.apply(&mut buf);
        assert_eq!(buf, after);
    }

    /// Mixed replace payload contains both backspaces and new text.
    #[test]
    fn delta_from_diff_mixed_replace_contains_backspace_and_append() {
        let before = "alpha beta";
        let after = "alpha gamma";
        let delta = TranscriptDelta::from_diff(before, after).unwrap();
        assert!(delta.delta.contains(BACKSPACE));
        assert!(delta.delta.ends_with("gamma"));

        let mut buf = before.to_string();
        delta.apply(&mut buf);
        assert_eq!(buf, after);
    }

    // ── SpeechUtterance ──

    /// `SpeechUtterance::duration` is end_ts − start_ts in seconds.
    #[test]
    fn utterance_duration() {
        let u = SpeechUtterance {
            samples: vec![0.0; 16000],
            sample_rate: 16000,
            start_ts: 1.5,
            end_ts: 2.5,
        };
        assert!((u.duration() - 1.0).abs() < f32::EPSILON);
    }

    // ── RawTranscript ──

    /// Default raw transcript is empty text with no segments.
    #[test]
    fn raw_transcript_default_has_no_segments() {
        let rt = RawTranscript::default();
        assert!(rt.text.is_empty());
        assert!(rt.segments.is_empty());
    }

    // ── EngineEvent ──

    /// Preview events clone field-for-field (rev + utterance-local text).
    #[test]
    fn engine_event_preview_clone() {
        let event = EngineEvent::Preview {
            rev: 1,
            text: "Hello world".to_string(),
        };
        let cloned = event.clone();
        if let EngineEvent::Preview { rev, text } = cloned {
            assert_eq!(rev, 1);
            assert_eq!(text, "Hello world");
        } else {
            panic!("Expected Preview variant");
        }
    }

    /// NoSpeech reason string survives clone for sink presentation.
    #[test]
    fn engine_event_no_speech_clone() {
        let event = EngineEvent::NoSpeech {
            reason: "vad_no_speech_detected".to_string(),
        };
        let cloned = event.clone();
        if let EngineEvent::NoSpeech { reason } = cloned {
            assert_eq!(reason, "vad_no_speech_detected");
        } else {
            panic!("Expected NoSpeech variant");
        }
    }

    /// DropKind Display labels stay stable for telemetry string matching.
    #[test]
    fn engine_event_drop_kind_display() {
        assert_eq!(DropKind::Hallucination.to_string(), "Hallucination");
        assert_eq!(DropKind::SemanticGate.to_string(), "SemanticGate");
        assert_eq!(DropKind::OverlapEmpty.to_string(), "OverlapEmpty");
        assert_eq!(DropKind::FilteredEmpty.to_string(), "FilteredEmpty");
    }

    /// Stats event preserves partial-pass counters for session telemetry.
    #[test]
    fn engine_event_stats_fields() {
        let event = EngineEvent::Stats {
            dropped_audio_chunks: 2,
            hallucination_drops: 3,
            semantic_gate_drops: 1,
            filtered_empty_drops: 0,
            corrections_applied: 4,
            total_utterances: 10,
            partial_runs_total: 7,
            trigger_utterance_count: 4,
            trigger_speech_count: 2,
            trigger_timer_count: 1,
            partial_stale_count: 3,
            partial_coalesced_count: 2,
            partial_dropped_count: 1,
        };
        if let EngineEvent::Stats {
            total_utterances,
            hallucination_drops,
            partial_runs_total,
            trigger_timer_count,
            ..
        } = event
        {
            assert_eq!(total_utterances, 10);
            assert_eq!(hallucination_drops, 3);
            assert_eq!(partial_runs_total, 7);
            assert_eq!(trigger_timer_count, 1);
        } else {
            panic!("Expected Stats variant");
        }
    }

    /// UtteranceFinal carries text, VAD%, logprob, and quality-gate fields.
    #[test]
    fn engine_event_utterance_final_roundtrip() {
        let event = EngineEvent::UtteranceFinal {
            utterance_id: 42,
            text: "cleaned text".to_string(),
            raw_text: "raw text from whisper".to_string(),
            start_ts: 1.5,
            end_ts: 3.2,
            segments: vec![TranscriptSegment {
                text: "cleaned".to_string(),
                start_ts: 1.5,
                end_ts: 3.2,
            }],
            vad_speech_pct: Some(84.0),
            avg_logprob: Some(-0.35),
            compression_ratio: Some(1.2),
            quality_gate_dropped: false,
            confidence_flags: Vec::new(),
        };
        if let EngineEvent::UtteranceFinal {
            utterance_id,
            text,
            raw_text,
            start_ts,
            end_ts,
            segments,
            vad_speech_pct,
            avg_logprob,
            compression_ratio,
            quality_gate_dropped,
            confidence_flags,
        } = event
        {
            assert_eq!(utterance_id, 42);
            assert_eq!(text, "cleaned text");
            assert_eq!(raw_text, "raw text from whisper");
            assert!((start_ts - 1.5).abs() < f32::EPSILON);
            assert!((end_ts - 3.2).abs() < f32::EPSILON);
            assert_eq!(segments.len(), 1);
            assert_eq!(vad_speech_pct, Some(84.0));
            assert_eq!(avg_logprob, Some(-0.35));
            assert_eq!(compression_ratio, Some(1.2));
            assert!(!quality_gate_dropped);
            assert!(confidence_flags.is_empty());
        } else {
            panic!("Expected UtteranceFinal variant");
        }
    }

    /// ReplaceRange serde + apply rewrites a char range on committed text.
    #[test]
    fn engine_event_replace_range_roundtrip_clone_and_apply() {
        let event = EngineEvent::ReplaceRange {
            utterance_id: 7,
            start: 6,
            end: 10,
            text: "world".to_string(),
            source: LayerSource::Lexicon,
        };

        let cloned = event.clone();
        assert_eq!(cloned, event);

        let json = serde_json::to_value(&event).expect("serialize replace_range");
        assert_eq!(
            json.get("type").and_then(serde_json::Value::as_str),
            Some("replace_range")
        );
        assert_eq!(
            json.get("source").and_then(serde_json::Value::as_str),
            Some("lexicon")
        );

        let roundtrip: EngineEvent =
            serde_json::from_value(json).expect("deserialize replace_range");
        assert_eq!(roundtrip, event);

        let mut committed = "hello wrld".to_string();
        let applied = roundtrip
            .apply_to_committed_text(&mut committed)
            .expect("apply bounded replacement");
        assert!(applied);
        assert_eq!(committed, "hello world");
    }

    /// InsertAnnotation serde + apply injects at a char position.
    #[test]
    fn engine_event_insert_annotation_roundtrip_clone_and_apply() {
        let event = EngineEvent::InsertAnnotation {
            utterance_id: 9,
            position: 8,
            text: "[śmiech] ".to_string(),
            kind: AnnotationKind::Paralingual {
                label: "laugh".to_string(),
            },
        };

        let cloned = event.clone();
        assert_eq!(cloned, event);

        let json = serde_json::to_value(&event).expect("serialize insert_annotation");
        assert_eq!(
            json.get("type").and_then(serde_json::Value::as_str),
            Some("insert_annotation")
        );

        let roundtrip: EngineEvent =
            serde_json::from_value(json).expect("deserialize insert_annotation");
        assert_eq!(roundtrip, event);

        let mut committed = "Pacjent spokojny".to_string();
        let applied = roundtrip
            .apply_to_committed_text(&mut committed)
            .expect("apply annotation insert");
        assert!(applied);
        assert_eq!(committed, "Pacjent [śmiech] spokojny");
    }

    /// SessionFinalised serdes; apply_to_committed_text is a deliberate noop.
    #[test]
    fn engine_event_session_finalised_roundtrip_and_noop_apply() {
        let event = EngineEvent::SessionFinalised {
            session_id: "session-abc".to_string(),
            layer_summary: LayerSummary {
                tail_patch_replacements: 1,
                lexicon_replacements: 2,
                inline_llm_replacements: 3,
                final_bam_replacements: 4,
                annotations_inserted: 5,
            },
        };

        let cloned = event.clone();
        assert_eq!(cloned, event);

        let json = serde_json::to_value(&event).expect("serialize session_finalised");
        assert_eq!(
            json.get("type").and_then(serde_json::Value::as_str),
            Some("session_finalised")
        );

        let roundtrip: EngineEvent =
            serde_json::from_value(json).expect("deserialize session_finalised");
        assert_eq!(roundtrip, event);

        let mut committed = "immutable".to_string();
        let applied = roundtrip
            .apply_to_committed_text(&mut committed)
            .expect("session finalised apply is a noop");
        assert!(!applied);
        assert_eq!(committed, "immutable");
    }

    // ── RawTranscript confidence metadata ──

    /// Default confidence fields are unset / not quality-dropped.
    #[test]
    fn raw_transcript_default_has_no_confidence() {
        let rt = RawTranscript::default();
        assert!(rt.avg_logprob.is_none());
        assert!(rt.compression_ratio.is_none());
        assert!(!rt.quality_gate_dropped);
    }

    /// Raw transcript can carry logprob + compression without a drop flag.
    #[test]
    fn raw_transcript_carries_confidence_metadata() {
        let rt = RawTranscript {
            text: "test".to_string(),
            avg_logprob: Some(-0.35),
            compression_ratio: Some(1.2),
            quality_gate_dropped: false,
            ..Default::default()
        };
        assert_eq!(rt.avg_logprob, Some(-0.35));
        assert_eq!(rt.compression_ratio, Some(1.2));
        assert!(!rt.quality_gate_dropped);
    }

    /// Quality-gate drop keeps metrics so consumers can still diagnose.
    #[test]
    fn raw_transcript_quality_gate_dropped_preserves_metadata() {
        let rt = RawTranscript {
            avg_logprob: Some(-1.5),
            compression_ratio: Some(4.0),
            quality_gate_dropped: true,
            ..Default::default()
        };
        assert!(rt.text.is_empty());
        assert!(rt.quality_gate_dropped);
        assert!(rt.avg_logprob.unwrap() < -1.0);
    }

    /// Default file options leave final-pass mode at `None` (opt-in only).
    #[test]
    fn file_transcription_options_default_disables_final_pass() {
        let options = FileTranscriptionOptions::default();
        assert_eq!(options.final_pass, FinalPassMode::None);
    }

    // ── TranscriptionVerdict ──

    /// Empty verdict with VAD no_speech still surfaces VeryLowSpeech flag.
    #[test]
    fn verdict_no_speech_carries_vad_truth() {
        let verdict = TranscriptionVerdict::from_parts(
            String::new(),
            RawTranscript::default(),
            Some(VadVerdict {
                speech_pct: 0.0,
                speech_windows: 0,
                total_windows: 60,
                no_speech: true,
                no_speech_reason: Some("vad_no_speech_detected".to_string()),
                sparkline: String::new(),
            }),
            TranscriptionSource::LocalFinalPass,
            TranscriptionEngineVerdict::whisper(TranscriptionEngineMode::EmbeddedDefault),
            None,
        );
        assert!(verdict.text.is_empty());
        assert!(verdict.vad.as_ref().unwrap().no_speech);
        assert_eq!(verdict.vad.as_ref().unwrap().total_windows, 60);
        assert_eq!(
            verdict.vad.as_ref().unwrap().no_speech_reason.as_deref(),
            Some("vad_no_speech_detected")
        );
        assert_eq!(verdict.source, TranscriptionSource::LocalFinalPass);
        assert_eq!(verdict.engine.engine, TranscriptionEngine::Whisper);
        assert_eq!(
            verdict.engine.mode,
            TranscriptionEngineMode::EmbeddedDefault
        );
        assert!(!verdict.engine.fallback_used);
        assert_eq!(
            verdict.confidence_flags,
            vec![TranscriptionConfidenceFlag::VeryLowSpeech]
        );
    }

    /// Speech verdict preserves text, VAD sparkline, engine fallback, final-pass.
    #[test]
    fn verdict_with_speech_carries_full_truth() {
        let verdict = TranscriptionVerdict::from_parts(
            "Cześć".to_string(),
            RawTranscript {
                text: "Cześć".to_string(),
                avg_logprob: Some(-0.25),
                compression_ratio: Some(1.1),
                quality_gate_dropped: false,
                ..Default::default()
            },
            Some(VadVerdict {
                speech_pct: 85.0,
                speech_windows: 17,
                total_windows: 20,
                no_speech: false,
                no_speech_reason: None,
                sparkline: "▁▃▅▇█▇▅▃▁▁▃▅▇█▇▅▃▁▁".to_string(),
            }),
            TranscriptionSource::LocalFinalPass,
            TranscriptionEngineVerdict::whisper(TranscriptionEngineMode::RuntimeFallback),
            Some(FinalPassVerdict {
                mode: FinalPassMode::EmbeddedLexiconCleanup,
                disposition: FinalPassDisposition::Changed,
                reason: None,
                lexicon_rewrites: 1,
                repetition_cleanups: 0,
            }),
        );
        assert_eq!(verdict.text, "Cześć");
        assert!(!verdict.vad.as_ref().unwrap().no_speech);
        assert_eq!(verdict.raw.avg_logprob, Some(-0.25));
        assert!(!verdict.raw.quality_gate_dropped);
        assert!(verdict.confidence_flags.is_empty());
        assert_eq!(
            verdict.final_pass.as_ref().unwrap().mode,
            FinalPassMode::EmbeddedLexiconCleanup
        );
        assert_eq!(
            verdict.final_pass.as_ref().unwrap().disposition,
            FinalPassDisposition::Changed
        );
        assert_eq!(verdict.engine.engine, TranscriptionEngine::Whisper);
        assert_eq!(
            verdict.engine.mode,
            TranscriptionEngineMode::RuntimeFallback
        );
        assert!(verdict.engine.fallback_used);
    }

    /// TranscriptionSource Display tokens match snake_case wire labels.
    #[test]
    fn transcription_source_display() {
        assert_eq!(
            TranscriptionSource::LocalFinalPass.to_string(),
            "local_final_pass"
        );
        assert_eq!(TranscriptionSource::Streaming.to_string(), "streaming");
        assert_eq!(TranscriptionSource::Cloud.to_string(), "cloud");
        assert_eq!(TranscriptionSource::Fallback.to_string(), "fallback");
        assert_eq!(TranscriptionEngine::Whisper.to_string(), "whisper");
        assert_eq!(TranscriptionEngine::Apple.to_string(), "apple");
        assert_eq!(
            TranscriptionEngineMode::EmbeddedDefault.to_string(),
            "embedded_default"
        );
        assert_eq!(
            TranscriptionEngineMode::RuntimeFallback.to_string(),
            "runtime_fallback"
        );
        assert_eq!(
            TranscriptionEngineMode::SpeechTranscriber.to_string(),
            "speech_transcriber"
        );
        assert_eq!(
            TranscriptionEngineMode::SfSpeechOnDevice.to_string(),
            "sf_speech_on_device"
        );
        assert_eq!(
            TranscriptionEngineMode::DictationTranscriber.to_string(),
            "dictation_transcriber"
        );
    }

    /// FinalPassMode/Disposition Display tokens match serde snake_case.
    #[test]
    fn final_pass_display_contract() {
        assert_eq!(FinalPassMode::None.to_string(), "none");
        assert_eq!(
            FinalPassMode::EmbeddedLexiconCleanup.to_string(),
            "embedded_lexicon_cleanup"
        );
        assert_eq!(FinalPassDisposition::Skipped.to_string(), "skipped");
        assert_eq!(FinalPassDisposition::Unchanged.to_string(), "unchanged");
        assert_eq!(FinalPassDisposition::Changed.to_string(), "changed");
        assert_eq!(FinalPassDisposition::Rejected.to_string(), "rejected");
        assert_eq!(FinalPassDisposition::Dropped.to_string(), "dropped");
        assert_eq!(
            TranscriptionConfidenceFlag::VeryLowSpeech.to_string(),
            "very_low_speech"
        );
        assert_eq!(
            TranscriptionConfidenceFlag::PossibleHallucinationLogprob.to_string(),
            "possible_hallucination_logprob"
        );
        assert_eq!(
            TranscriptionConfidenceFlag::QualityGateDropped.to_string(),
            "quality_gate_dropped"
        );
        assert_eq!(
            TranscriptionConfidenceFlag::UnverifiedStream.to_string(),
            "unverified_stream"
        );
    }

    /// from_parts materializes engine flags from VAD% + logprob heuristics.
    #[test]
    fn verdict_derives_engine_confidence_flags() {
        let verdict = TranscriptionVerdict::from_parts(
            "podejrzany wynik".to_string(),
            RawTranscript {
                text: "podejrzany wynik".to_string(),
                avg_logprob: Some(-1.2),
                compression_ratio: Some(4.0),
                quality_gate_dropped: true,
                ..Default::default()
            },
            Some(VadVerdict {
                speech_pct: 4.0,
                speech_windows: 1,
                total_windows: 20,
                no_speech: false,
                no_speech_reason: None,
                sparkline: String::new(),
            }),
            TranscriptionSource::LocalFinalPass,
            TranscriptionEngineVerdict::whisper(TranscriptionEngineMode::EmbeddedDefault),
            None,
        );

        assert_eq!(
            verdict.confidence_flags,
            vec![
                TranscriptionConfidenceFlag::VeryLowSpeech,
                TranscriptionConfidenceFlag::PossibleHallucinationLogprob,
                TranscriptionConfidenceFlag::QualityGateDropped,
            ]
        );
    }

    /// Silero tail-drop count appends SileroDroppedTailHallucinations flag.
    #[test]
    fn verdict_from_parts_with_silero_drops_adds_typed_flag() {
        let verdict = TranscriptionVerdict::from_parts_with_silero_drops(
            "krótki tekst".to_string(),
            RawTranscript {
                text: "krótki tekst".to_string(),
                ..Default::default()
            },
            Some(VadVerdict {
                speech_pct: 64.0,
                speech_windows: 8,
                total_windows: 12,
                no_speech: false,
                no_speech_reason: None,
                sparkline: "▁▃▅▇█▇".to_string(),
            }),
            TranscriptionSource::LocalFinalPass,
            TranscriptionEngineVerdict::whisper(TranscriptionEngineMode::EmbeddedDefault),
            None,
            2,
        );

        assert!(
            verdict.confidence_flags.contains(
                &TranscriptionConfidenceFlag::SileroDroppedTailHallucinations { count: 2 }
            )
        );
    }

    // ── Truth QA: UtteranceFinal confidence contract ──

    /// UtteranceFinal can embed precomputed confidence_flags for sinks.
    #[test]
    fn utterance_final_carries_confidence_metadata() {
        let event = EngineEvent::UtteranceFinal {
            utterance_id: 1,
            text: "test".to_string(),
            raw_text: "test".to_string(),
            start_ts: 0.0,
            end_ts: 1.0,
            segments: Vec::new(),
            vad_speech_pct: Some(4.0),
            avg_logprob: Some(-0.85),
            compression_ratio: Some(2.5),
            quality_gate_dropped: false,
            confidence_flags: vec![
                TranscriptionConfidenceFlag::VeryLowSpeech,
                TranscriptionConfidenceFlag::PossibleHallucinationLogprob,
            ],
        };
        if let EngineEvent::UtteranceFinal {
            vad_speech_pct,
            avg_logprob,
            compression_ratio,
            quality_gate_dropped,
            confidence_flags,
            ..
        } = event
        {
            assert_eq!(vad_speech_pct, Some(4.0));
            assert!(avg_logprob.unwrap() < -0.5, "low confidence must survive");
            assert!(
                compression_ratio.unwrap() > 2.0,
                "high compression must survive"
            );
            assert!(!quality_gate_dropped);
            assert_eq!(
                confidence_flags,
                vec![
                    TranscriptionConfidenceFlag::VeryLowSpeech,
                    TranscriptionConfidenceFlag::PossibleHallucinationLogprob,
                ]
            );
        }
    }

    /// Quality-gate-dropped UtteranceFinal still ships logprob metadata.
    #[test]
    fn utterance_final_quality_gate_truth() {
        let event = EngineEvent::UtteranceFinal {
            utterance_id: 1,
            text: String::new(),
            raw_text: String::new(),
            start_ts: 0.0,
            end_ts: 1.0,
            segments: Vec::new(),
            vad_speech_pct: Some(3.0),
            avg_logprob: Some(-1.5),
            compression_ratio: Some(4.0),
            quality_gate_dropped: true,
            confidence_flags: vec![
                TranscriptionConfidenceFlag::VeryLowSpeech,
                TranscriptionConfidenceFlag::PossibleHallucinationLogprob,
                TranscriptionConfidenceFlag::QualityGateDropped,
            ],
        };
        if let EngineEvent::UtteranceFinal {
            vad_speech_pct,
            quality_gate_dropped,
            avg_logprob,
            confidence_flags,
            ..
        } = event
        {
            assert_eq!(vad_speech_pct, Some(3.0));
            assert!(quality_gate_dropped, "gate drop must be visible in event");
            assert!(avg_logprob.unwrap() < -1.0);
            assert_eq!(
                confidence_flags,
                vec![
                    TranscriptionConfidenceFlag::VeryLowSpeech,
                    TranscriptionConfidenceFlag::PossibleHallucinationLogprob,
                    TranscriptionConfidenceFlag::QualityGateDropped,
                ]
            );
        }
    }

    // ── Truth QA: Serialization roundtrip ──

    /// Full TranscriptionVerdict JSON roundtrip preserves nested truth.
    #[test]
    fn verdict_serialization_roundtrip_preserves_all_truth() {
        let verdict = TranscriptionVerdict::from_parts(
            "Cześć, jak się masz".to_string(),
            RawTranscript {
                text: "Cześć, jak się masz".to_string(),
                segments: vec![TranscriptSegment {
                    text: "Cześć, jak się masz".to_string(),
                    start_ts: 0.0,
                    end_ts: 2.5,
                }],
                avg_logprob: Some(-0.35),
                compression_ratio: Some(1.2),
                quality_gate_dropped: false,
            },
            Some(VadVerdict {
                speech_pct: 78.0,
                speech_windows: 15,
                total_windows: 20,
                no_speech: false,
                no_speech_reason: None,
                sparkline: "▁▃▅▇█▇▅▃▁▁▃▅▇█▇▅▃▁▁".to_string(),
            }),
            TranscriptionSource::LocalFinalPass,
            TranscriptionEngineVerdict::whisper(TranscriptionEngineMode::EmbeddedDefault),
            Some(FinalPassVerdict {
                mode: FinalPassMode::EmbeddedLexiconCleanup,
                disposition: FinalPassDisposition::Changed,
                reason: None,
                lexicon_rewrites: 2,
                repetition_cleanups: 1,
            }),
        );

        let json = serde_json::to_string(&verdict).expect("verdict must serialize");
        let restored: TranscriptionVerdict =
            serde_json::from_str(&json).expect("verdict must deserialize");

        assert_eq!(restored.text, verdict.text);
        assert_eq!(restored.source, verdict.source);
        assert_eq!(restored.engine, verdict.engine);
        assert_eq!(restored.confidence_flags, verdict.confidence_flags);

        let vad = restored.vad.as_ref().unwrap();
        assert_eq!(vad.speech_pct, 78.0);
        assert_eq!(vad.sparkline, "▁▃▅▇█▇▅▃▁▁▃▅▇█▇▅▃▁▁");
        assert!(!vad.no_speech);

        assert_eq!(restored.raw.avg_logprob, Some(-0.35));
        assert_eq!(restored.raw.segments.len(), 1);

        let fp = restored.final_pass.as_ref().unwrap();
        assert_eq!(fp.mode, FinalPassMode::EmbeddedLexiconCleanup);
        assert_eq!(fp.disposition, FinalPassDisposition::Changed);
        assert_eq!(fp.lexicon_rewrites, 2);
    }

    /// Empty sparkline is skipped on serialize (skip_serializing_if empty).
    #[test]
    fn verdict_no_speech_serialization_omits_empty_sparkline() {
        let verdict = TranscriptionVerdict::from_parts(
            String::new(),
            RawTranscript::default(),
            Some(VadVerdict {
                speech_pct: 0.0,
                speech_windows: 0,
                total_windows: 30,
                no_speech: true,
                no_speech_reason: Some("vad_no_speech_detected".to_string()),
                sparkline: String::new(),
            }),
            TranscriptionSource::LocalFinalPass,
            TranscriptionEngineVerdict::whisper(TranscriptionEngineMode::EmbeddedDefault),
            None,
        );

        let json = serde_json::to_string(&verdict).expect("verdict must serialize");
        assert!(
            !json.contains("sparkline"),
            "empty sparkline should be omitted from JSON"
        );

        let restored: TranscriptionVerdict =
            serde_json::from_str(&json).expect("verdict must deserialize without sparkline");
        assert!(restored.vad.as_ref().unwrap().sparkline.is_empty());
        assert!(restored.vad.as_ref().unwrap().no_speech);
        assert_eq!(
            restored.engine,
            TranscriptionEngineVerdict::whisper(TranscriptionEngineMode::EmbeddedDefault)
        );
    }

    /// Non-empty VAD sparkline survives construction and field access.
    #[test]
    fn verdict_sparkline_preserved_through_vad() {
        let sparkline = "▁▁▃▅▇████▇▅▃▁▁▁";
        let verdict = TranscriptionVerdict::from_parts(
            "tekst".to_string(),
            RawTranscript {
                text: "tekst".to_string(),
                ..Default::default()
            },
            Some(VadVerdict {
                speech_pct: 60.0,
                speech_windows: 6,
                total_windows: 16,
                no_speech: false,
                no_speech_reason: None,
                sparkline: sparkline.to_string(),
            }),
            TranscriptionSource::LocalFinalPass,
            TranscriptionEngineVerdict::whisper(TranscriptionEngineMode::EmbeddedDefault),
            None,
        );

        assert_eq!(verdict.vad.as_ref().unwrap().sparkline, sparkline);
    }

    // ── Serde roundtrip edge cases for the truth-surface verdict structs ──

    /// Every FinalPassDisposition serdes as its snake_case token.
    #[test]
    fn final_pass_verdict_serde_roundtrip_covers_all_dispositions() {
        let cases = [
            FinalPassDisposition::Skipped,
            FinalPassDisposition::Unchanged,
            FinalPassDisposition::Changed,
            FinalPassDisposition::Rejected,
            FinalPassDisposition::Dropped,
        ];
        for disposition in cases {
            let verdict = FinalPassVerdict {
                mode: FinalPassMode::EmbeddedLexiconCleanup,
                disposition,
                reason: Some(format!("case_{disposition}")),
                lexicon_rewrites: 3,
                repetition_cleanups: 1,
            };
            let json = serde_json::to_string(&verdict).expect("serialize");
            let restored: FinalPassVerdict = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(restored, verdict, "disposition {disposition} round-trip");
        }
    }

    /// Optional final-pass reason None survives serde roundtrip.
    #[test]
    fn final_pass_verdict_serde_preserves_none_reason() {
        let verdict = FinalPassVerdict {
            mode: FinalPassMode::None,
            disposition: FinalPassDisposition::Skipped,
            reason: None,
            lexicon_rewrites: 0,
            repetition_cleanups: 0,
        };
        let json = serde_json::to_string(&verdict).unwrap();
        assert!(
            json.contains("\"reason\":null"),
            "reason=None must serialize as JSON null (got {json})"
        );
        let restored: FinalPassVerdict = serde_json::from_str(&json).unwrap();
        assert!(restored.reason.is_none());
    }

    /// VadVerdict no_speech_reason survives JSON roundtrip.
    #[test]
    fn vad_verdict_serde_roundtrip_preserves_no_speech_reason() {
        let verdict = VadVerdict {
            speech_pct: 0.0,
            speech_windows: 0,
            total_windows: 60,
            no_speech: true,
            no_speech_reason: Some("vad_no_speech_detected".to_string()),
            sparkline: String::new(),
        };
        let json = serde_json::to_string(&verdict).unwrap();
        // Empty sparkline must be elided by skip_serializing_if.
        assert!(
            !json.contains("\"sparkline\""),
            "empty sparkline must be skipped in serialized form (got {json})"
        );
        let restored: VadVerdict = serde_json::from_str(&json).unwrap();
        assert!(restored.no_speech);
        assert_eq!(
            restored.no_speech_reason.as_deref(),
            Some("vad_no_speech_detected")
        );
        assert!(restored.sparkline.is_empty());
    }

    /// Present sparkline is not stripped by serde defaults.
    #[test]
    fn vad_verdict_serde_roundtrip_preserves_sparkline_when_present() {
        let sparkline = "▁▃▇█▇▃▁";
        let verdict = VadVerdict {
            speech_pct: 62.5,
            speech_windows: 5,
            total_windows: 8,
            no_speech: false,
            no_speech_reason: None,
            sparkline: sparkline.to_string(),
        };
        let json = serde_json::to_string(&verdict).unwrap();
        assert!(json.contains("sparkline"));
        let restored: VadVerdict = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.sparkline, sparkline);
        assert!(!restored.no_speech);
        assert!(restored.no_speech_reason.is_none());
    }

    /// Missing sparkline field deserializes to empty String via default.
    #[test]
    fn vad_verdict_deserialize_accepts_missing_sparkline_via_default() {
        // Older snapshots may omit the sparkline field entirely; serde(default)
        // must accept the absence without failing.
        let json = r#"{
            "speech_pct": 50.0,
            "speech_windows": 4,
            "total_windows": 8,
            "no_speech": false,
            "no_speech_reason": null
        }"#;
        let restored: VadVerdict = serde_json::from_str(json).unwrap();
        assert!(restored.sparkline.is_empty());
        assert_eq!(restored.speech_pct, 50.0);
    }

    /// All VadClass variants serde under snake_case wire tokens.
    #[test]
    fn vad_class_serde_roundtrip_covers_all_variants() {
        let cases = [
            VadClass::Speech,
            VadClass::UtteranceGap,
            VadClass::SentenceBoundary,
            VadClass::TrailingSilence,
        ];
        for class in cases {
            let json = serde_json::to_string(&class).expect("serialize vad class");
            let restored: VadClass = serde_json::from_str(&json).expect("deserialize vad class");
            assert_eq!(restored, class);
        }
    }

    /// VadClass Display string equals the serde rename token.
    #[test]
    fn vad_class_display_matches_serde_token() {
        let cases = [
            VadClass::Speech,
            VadClass::UtteranceGap,
            VadClass::SentenceBoundary,
            VadClass::TrailingSilence,
        ];
        for class in cases {
            let json = serde_json::to_string(&class).expect("serialize vad class");
            assert_eq!(json, format!("\"{class}\""));
        }
    }

    /// All TranscriptionConfidenceFlag variants (incl. payload) serde.
    #[test]
    fn confidence_flag_serde_roundtrip_covers_all_variants() {
        let cases = [
            // Engine-owned
            TranscriptionConfidenceFlag::VeryLowSpeech,
            TranscriptionConfidenceFlag::PossibleHallucinationLogprob,
            TranscriptionConfidenceFlag::QualityGateDropped,
            // App-level provenance (new in 0.9.3)
            TranscriptionConfidenceFlag::LocalFinalPassUnavailable,
            TranscriptionConfidenceFlag::CloudFallbackUsed,
            TranscriptionConfidenceFlag::StreamingPreviewUsedAsVerdict,
            TranscriptionConfidenceFlag::UnverifiedStream,
            TranscriptionConfidenceFlag::CloudPrimaryMissing,
            TranscriptionConfidenceFlag::AiNoopDetected,
            TranscriptionConfidenceFlag::FinalPassLengthRegression,
        ];
        for flag in cases {
            let json = serde_json::to_string(&flag).expect("serialize flag");
            let restored: TranscriptionConfidenceFlag =
                serde_json::from_str(&json).expect("deserialize flag");
            assert_eq!(restored, flag, "round-trip for {flag}");
            // Serde rename_all=snake_case must match Display so truth.json
            // strings stay stable between legacy (string) and typed consumers.
            assert_eq!(
                json,
                format!("\"{flag}\""),
                "serde snake_case must match Display"
            );
        }

        let structured = TranscriptionConfidenceFlag::SileroDroppedTailHallucinations { count: 3 };
        let json = serde_json::to_value(structured).expect("serialize structured flag");
        assert_eq!(
            json,
            serde_json::json!({
                "silero_dropped_tail_hallucinations": {
                    "count": 3
                }
            })
        );
        let restored: TranscriptionConfidenceFlag =
            serde_json::from_value(json).expect("deserialize structured flag");
        assert_eq!(
            restored,
            TranscriptionConfidenceFlag::SileroDroppedTailHallucinations { count: 3 }
        );
        assert_eq!(
            structured.to_string(),
            "silero_dropped_tail_hallucinations:3"
        );
    }

    /// Legacy plain string confidence tokens still deserialize.
    #[test]
    fn confidence_flag_legacy_strings_still_deserialize() {
        // Guard-rail: pre-0.9.3 truth.json files stored flags as bare strings
        // (e.g. "local_final_pass_unavailable"). The typed enum must keep
        // accepting those exact tokens via rename_all=snake_case so old
        // sidecars remain readable once we migrate to Vec<typed-enum>.
        let legacy_tokens = [
            (
                "\"very_low_speech\"",
                TranscriptionConfidenceFlag::VeryLowSpeech,
            ),
            (
                "\"possible_hallucination_logprob\"",
                TranscriptionConfidenceFlag::PossibleHallucinationLogprob,
            ),
            (
                "\"quality_gate_dropped\"",
                TranscriptionConfidenceFlag::QualityGateDropped,
            ),
            (
                "\"local_final_pass_unavailable\"",
                TranscriptionConfidenceFlag::LocalFinalPassUnavailable,
            ),
            (
                "\"cloud_fallback_used\"",
                TranscriptionConfidenceFlag::CloudFallbackUsed,
            ),
            (
                "\"streaming_preview_used_as_verdict\"",
                TranscriptionConfidenceFlag::StreamingPreviewUsedAsVerdict,
            ),
            (
                "\"unverified_stream\"",
                TranscriptionConfidenceFlag::UnverifiedStream,
            ),
            (
                "\"cloud_primary_missing\"",
                TranscriptionConfidenceFlag::CloudPrimaryMissing,
            ),
            (
                "\"ai_noop_detected\"",
                TranscriptionConfidenceFlag::AiNoopDetected,
            ),
            (
                "\"final_pass_length_regression\"",
                TranscriptionConfidenceFlag::FinalPassLengthRegression,
            ),
        ];
        for (json, expected) in legacy_tokens {
            let restored: TranscriptionConfidenceFlag = serde_json::from_str(json)
                .unwrap_or_else(|e| panic!("legacy token {json} must deserialize: {e}"));
            assert_eq!(restored, expected, "legacy token {json}");
        }
    }

    /// Short final vs long stream is length regression (div0 Apple collapse).
    #[test]
    fn final_pass_length_regression_div0_apple_file_collapse() {
        // div0 2026-07-23: ~98s audio, stream 56 chars, Apple file final 12 chars.
        let stream = "Im wystarczy i jeszcze troche z live assembly stream";
        assert!(stream.chars().count() >= FINAL_PASS_REGRESSION_MIN_STREAM_CHARS);
        assert!(final_pass_is_length_regression("Im wystarczy", stream));
    }

    /// Thin stream and comparable finals do not trip regression guard.
    #[test]
    fn final_pass_length_regression_spares_cold_start_and_comparable_finals() {
        // Empty/thin live → final must still recover (cold Whisper / cold Apple).
        assert!(!final_pass_is_length_regression(
            "poczatek wypowiedzi z zimnego startu",
            ""
        ));
        assert!(!final_pass_is_length_regression(
            "krotki final",
            "krotki stream"
        ));
        // Comparable length: final may win even if not identical.
        assert!(!final_pass_is_length_regression(
            "czysty final pass tekst dlugi wystarczajaco",
            "powtarzajacy sie streaming preview tekst"
        ));
    }

    /// from_parts builds a verdict with derived flags and provenance intact.
    #[test]
    fn transcription_verdict_from_parts_roundtrip() {
        let verdict = TranscriptionVerdict::from_parts(
            "smoke text".to_string(),
            RawTranscript {
                text: "smoke text".to_string(),
                segments: vec![TranscriptSegment {
                    text: "smoke text".to_string(),
                    start_ts: 0.0,
                    end_ts: 1.5,
                }],
                avg_logprob: Some(-0.4),
                compression_ratio: Some(1.1),
                quality_gate_dropped: false,
            },
            None,
            TranscriptionSource::LocalFinalPass,
            TranscriptionEngineVerdict::whisper(TranscriptionEngineMode::EmbeddedDefault),
            None,
        );
        let json = serde_json::to_string(&verdict).expect("serialize");
        let restored: TranscriptionVerdict = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.text, verdict.text);
        assert_eq!(restored.raw.text, verdict.raw.text);
        assert_eq!(restored.source, verdict.source);
        assert_eq!(restored.engine, verdict.engine);
        assert_eq!(restored.confidence_flags, verdict.confidence_flags);
        assert!(restored.vad.is_none());
        assert!(restored.final_pass.is_none());
    }
}
