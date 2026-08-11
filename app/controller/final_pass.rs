//! Stop-path final-pass routing, streaming completeness, and latency receipts.
//!
//! Settings / `FINAL_PASS_MODE` is law (operator 2026-08-05):
//! - **Always** (`on`): full Whisper file re-pass after release. The **only**
//!   mode in which a full-file re-pass is permitted.
//! - **Smart**: Whisper may final-pass **individual utterances only** — never the
//!   whole file. When streaming completeness is adjudicated Complete, nothing runs
//!   at stop. When coverage-complete, plausibly dense, but shapeless (long wall
//!   of words, no sentence terminals — the shape row, operator 2026-08-09),
//!   Whisper transcribes the file but only its punctuation/capitalization is
//!   adopted onto the committed words (`punctuation_transplant`; word sequence
//!   invariant at THIS stage
//!   because shape is the deficit here — live word corrections belong to the
//!   Layer 1 tail patch, which is a core element, on by default). When
//!   incomplete, only the uncommitted audio tail (from the last committed
//!   utterance end) is transcribed and **appended** to the committed streaming
//!   text. The doctrine behind all of this forbids TRUNCATING overlay text and
//!   DROPPING transcripts the user already saw (the pre-0.8.0 full-replace
//!   failure) — it does not forbid live correction of committed words.
//!   Pair with `CODESCRIBE_LAYERED_TRANSCRIPTION` (orthogonal toggle) for live
//!   Whisper tail-patches during hold — Smart does not force layered on.
//! - **Off**: hard off at stop — zero Whisper invocation on the stop path;
//!   streaming (+ post-process) is final.
//!
//! Dictionary / lexicon cleanup is **not** gated by this mode — it always runs
//! in the transcript post-process pipeline (`StreamPostProcessor`), in all modes.

use codescribe_core::pipeline::contracts::FinalPassDisposition;

use super::helpers::{CompletenessCommitSource, SessionTelemetrySnapshot};

/// Canonical stop-path routing mode + its env resolution now live in core
/// (`codescribe_core::config::final_pass`) so **every** stop lane — this
/// controller and the composer voice-note lane in the UniFFI bridge — consults
/// one parser instead of a per-lane twin. Re-exported at the historic path so
/// controller call sites and tests stay unchanged.
pub(crate) use codescribe_core::config::{FinalPassRoutingMode, final_pass_routing_mode};

/// Typed streaming-completeness decision for Smart skip (never "non-empty ⇒ complete").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamingCompleteness {
    /// The adjudicator sealed the transcript: committed, covered, no pending tail.
    Complete,
    /// Coverage is complete but the transcript is a wall of words: long text
    /// with (almost) no sentence-terminal punctuation. Everything was heard —
    /// nothing was shaped. Measured root cause of "final pass tak wkurwiał"
    /// (2026-08-09): Apple live commits ~1 period per ~1050 chars of Polish
    /// while Whisper over the same audio produces 9, and Smart's
    /// coverage-only Complete skipped the only pass that carries sentences.
    CompleteShapeDeficient,
    /// Something is missing; `reason` is the stable telemetry label for which
    /// check failed (`no_speech`, `empty`, `pending_tail`, `partial_pending`,
    /// `no_commit_source`, `no_coverage`).
    Incomplete { reason: &'static str },
}

/// Below this length shape is not judged: a short note legitimately has no
/// terminal punctuation and must keep skipping the final pass.
const SHAPE_MIN_CHARS: usize = 320;
/// A transcript with more characters per sentence terminal than this reads as
/// unshaped. Grounded in the 2026-08-09 measurement: human reference ≈ 175
/// chars/terminal, Whisper ≈ 141, Apple's wall ≈ 1052 — the floor sits far
/// from natural prose so lists and long clauses do not trip it.
const SHAPE_MAX_CHARS_PER_TERMINAL: usize = 400;
/// A single terminal-free stretch longer than this marks the transcript
/// deficient even when the average looks healthy. Measured 2026-08-10: the
/// morning dictation averaged ~140 chars/terminal (live tail patches had
/// seeded the head with sentences) yet carried a ~450-char run-on wall in the
/// middle — averages hide local walls, and the skip decision quoted the
/// average. Same magnitude as SHAPE_MIN_CHARS: a stretch long enough to be
/// judged at all must contain at least one sentence end.
const SHAPE_MAX_TERMINAL_FREE_RUN: usize = 320;

/// True when a coverage-complete transcript is long enough to need sentences
/// and carries (almost) none — globally (chars per terminal) or locally (one
/// run-on stretch with no terminal at all).
pub(crate) fn transcript_shape_deficient(text: &str) -> bool {
    let text = text.trim();
    let chars = text.chars().count();
    if chars < SHAPE_MIN_CHARS {
        return false;
    }
    let terminals = text
        .chars()
        .filter(|c| matches!(c, '.' | '!' | '?' | '…'))
        .count();
    if terminals == 0 || chars / terminals > SHAPE_MAX_CHARS_PER_TERMINAL {
        return true;
    }
    let mut run = 0usize;
    let mut longest_run = 0usize;
    for c in text.chars() {
        if matches!(c, '.' | '!' | '?' | '…') {
            run = 0;
        } else {
            run += 1;
            longest_run = longest_run.max(run);
        }
    }
    longest_run > SHAPE_MAX_TERMINAL_FREE_RUN
}

/// Recorder/adjudicator evidence for Smart final-pass skip.
///
/// Punctuation is never the authority: Completeness requires an adjudicated
/// commit source, coverage, and a cleared pending tail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StreamingCompletenessEvidence {
    pub streaming_text: String,
    pub no_speech_reason: Option<String>,
    pub pending_tail: bool,
    pub partial_stale_or_dropped: bool,
    pub commit_source: Option<CompletenessCommitSource>,
    pub committed_chars: usize,
    pub total_utterances: u64,
}

impl StreamingCompletenessEvidence {
    /// Build evidence from the live session snapshot + streaming splice text.
    pub(crate) fn from_session(streaming_text: &str, session: &SessionTelemetrySnapshot) -> Self {
        let partial_stale_or_dropped = session
            .stats
            .as_ref()
            .is_some_and(|s| s.partial_stale_count > 0 || s.partial_dropped_count > 0);
        let total_utterances = session
            .stats
            .as_ref()
            .map(|s| s.total_utterances)
            .unwrap_or(0);
        Self {
            streaming_text: streaming_text.to_string(),
            no_speech_reason: session.no_speech_reason.clone(),
            pending_tail: session.pending_tail,
            partial_stale_or_dropped,
            commit_source: session.last_commit_source,
            committed_chars: session.committed_chars,
            total_utterances,
        }
    }

    /// Coverage is present when the adjudicator sealed at least one utterance
    /// (commit source + committed chars or engine utterance count).
    pub(crate) fn has_coverage(&self) -> bool {
        self.committed_chars > 0 || self.total_utterances > 0
    }
}

/// Assess whether streaming holds an adjudicator-confirmed complete transcript.
///
/// Incomplete when: empty, no-speech, pending tail, partial stale/drop, missing
/// commit source, or zero coverage. A punctuated prefix with a pending tail is
/// always Incomplete — punctuation alone never authorizes Complete.
pub(crate) fn assess_streaming_completeness(
    evidence: &StreamingCompletenessEvidence,
) -> StreamingCompleteness {
    if evidence.no_speech_reason.is_some() {
        return StreamingCompleteness::Incomplete {
            reason: "no_speech",
        };
    }
    let text = evidence.streaming_text.trim();
    if text.is_empty() {
        return StreamingCompleteness::Incomplete { reason: "empty" };
    }
    if evidence.pending_tail {
        return StreamingCompleteness::Incomplete {
            reason: "pending_tail",
        };
    }
    if evidence.partial_stale_or_dropped {
        return StreamingCompleteness::Incomplete {
            reason: "partial_pending",
        };
    }
    if evidence.commit_source.is_none() {
        return StreamingCompleteness::Incomplete {
            reason: "no_commit_source",
        };
    }
    if !evidence.has_coverage() {
        return StreamingCompleteness::Incomplete {
            reason: "no_coverage",
        };
    }
    // Coverage says everything was heard; shape asks whether anything was
    // shaped. A long wall of words routes to the punctuation transplant
    // instead of a silent skip (operator decision 2026-08-09, superseding the
    // 2026-08-05 coverage-only mapping).
    if transcript_shape_deficient(text) {
        return StreamingCompleteness::CompleteShapeDeficient;
    }
    StreamingCompleteness::Complete
}

/// Convenience for tests that build evidence by field rather than from a session.
#[cfg(test)]
pub(crate) fn assess_streaming_completeness_fields(
    streaming_text: &str,
    no_speech_reason: Option<&str>,
    pending_tail: bool,
    partial_stale_or_dropped: bool,
    commit_source: Option<CompletenessCommitSource>,
    committed_chars: usize,
    total_utterances: u64,
) -> StreamingCompleteness {
    assess_streaming_completeness(&StreamingCompletenessEvidence {
        streaming_text: streaming_text.to_string(),
        no_speech_reason: no_speech_reason.map(str::to_string),
        pending_tail,
        partial_stale_or_dropped,
        commit_source,
        committed_chars,
        total_utterances,
    })
}

// ── Committed-density floor over the structural verdict ─────────────────────
//
// Structural completeness answers "did the adjudicator seal everything it
// saw". It cannot answer "was what it saw plausibly a whole minute of speech",
// because every input it reads has already passed through the live
// accumulator — so when the accumulator eats a phrase, coverage stays intact
// over the survivors and reports Complete. Measured 2026-08-10/11: two Polish
// takes delivered 220 chars over a COMPLETE 104 s WAV and 118 chars over a
// complete 107 s WAV. Both were labelled `complete_streaming_transcript` and
// skipped while 85–90% of the speech was gone. This floor is the second
// opinion that skip decision never had.

/// Below this audio length committed density is not judged at all.
///
/// A short note is legitimately sparse — "kup mleko i chleb" over nine seconds
/// is a real dictation, not a starving one — and escalating those would put a
/// Whisper pass on the stop path of every quick capture.
const DENSITY_MIN_AUDIO_SECS: f32 = 10.0;

/// Committed characters per audio second below which a coverage-complete
/// verdict describes starvation rather than completeness.
///
/// Grounded in the measured pair — 2.1 chars/s (220 ch / 104 s) and 1.1 chars/s
/// (118 ch / 107 s) — against a healthy take at ~13 chars/s. Conversational
/// Polish runs roughly 12–15 chars/s, so this floor sits far below anything a
/// real dictation produces. Deliberately loose: a false escalation costs one
/// tail-gap pass, a false skip loses speech permanently, and the doctrine
/// resolves that asymmetry in favour of keeping speech.
const DENSITY_MIN_CHARS_PER_SEC: f32 = 4.0;

/// Telemetry label replacing `complete_streaming_transcript` when the floor
/// overrides the verdict. Stable string — it is a log/receipt contract.
pub(crate) const DENSITY_STARVED_REASON: &str = "starved_density";

/// Committed characters per second of recorded audio.
///
/// `None` when the denominator is unusable (non-finite, zero, negative). A
/// missing audio length must never become a synthetic density: `0.0` would read
/// as maximal starvation and escalate every session that reached this code.
pub(crate) fn committed_density_chars_per_sec(
    audio_secs: f32,
    committed_chars: usize,
) -> Option<f32> {
    if !audio_secs.is_finite() || audio_secs <= 0.0 {
        return None;
    }
    Some(committed_chars as f32 / audio_secs)
}

/// True when a session long enough to judge committed too few characters to be
/// believable.
///
/// NaN-safe through [`committed_density_chars_per_sec`]: an unmeasurable
/// duration yields `None` and reports "not starved" rather than escalating on
/// evidence it does not have.
pub(crate) fn committed_density_starved(audio_secs: f32, committed_chars: usize) -> bool {
    let Some(density) = committed_density_chars_per_sec(audio_secs, committed_chars) else {
        return false;
    };
    audio_secs > DENSITY_MIN_AUDIO_SECS && density < DENSITY_MIN_CHARS_PER_SEC
}

/// Apply the committed-density floor to a structural completeness verdict.
///
/// Every structurally complete verdict (`Complete` or
/// `CompleteShapeDeficient`) is eligible for demotion into the existing
/// `Incomplete` arm. [`final_pass_action`] carries the consequence with no new
/// variant and no second controller: `Smart` +
/// `Incomplete{starved_density}` → `TailGapFill`, which is the residual /
/// tail-gap path the live session text already feeds through
/// [`smart_tail_gap_source`]. This ordering matters: sentence shape cannot
/// excuse implausibly sparse coverage, because a weak punctuation alignment
/// may otherwise return the starving canvas untouched. `Off` still skips and
/// `Always` still re-passes, because routing ignores completeness for both —
/// the floor changes Smart alone.
///
/// `audio_secs = None` leaves the verdict untouched: an unknown denominator is
/// not evidence of starvation. The call site logs that silence so the absence
/// shows up in the receipts instead of having to be inferred from them.
pub(crate) fn apply_committed_density_floor(
    completeness: StreamingCompleteness,
    audio_secs: Option<f32>,
    committed_chars: usize,
) -> StreamingCompleteness {
    let Some(audio_secs) = audio_secs else {
        return completeness;
    };
    if matches!(
        completeness,
        StreamingCompleteness::Complete | StreamingCompleteness::CompleteShapeDeficient
    ) && committed_density_starved(audio_secs, committed_chars)
    {
        return StreamingCompleteness::Incomplete {
            reason: DENSITY_STARVED_REASON,
        };
    }
    completeness
}

/// Single INFO line for a density-overridden structural verdict.
///
/// Carries both verdict labels and the numbers behind the override — never the
/// transcript. The guard fires precisely when the committed text is short
/// enough to fit comfortably in a log line, which is exactly when writing user
/// speech into `~/.codescribe/logs/codescribe.log` would be easiest to justify
/// and still wrong.
pub(crate) fn format_density_override_line(
    overridden: StreamingCompleteness,
    audio_secs: f32,
    committed_chars: usize,
) -> String {
    let overridden = match overridden {
        StreamingCompleteness::Complete => "complete_streaming_transcript",
        StreamingCompleteness::CompleteShapeDeficient => "shape_deficient",
        StreamingCompleteness::Incomplete { reason } => reason,
    };
    format!(
        "final_pass_density_guard overridden_verdict={overridden} new_verdict={reason} audio_secs={audio_secs:.3} committed_chars={committed_chars} density_chars_per_sec={density:.2} floor_chars_per_sec={floor:.1} min_audio_secs={min_secs:.1} route=tail_gap_fill",
        reason = DENSITY_STARVED_REASON,
        density = committed_density_chars_per_sec(audio_secs, committed_chars).unwrap_or(f32::NAN),
        floor = DENSITY_MIN_CHARS_PER_SEC,
        min_secs = DENSITY_MIN_AUDIO_SECS,
    )
}

/// Label from the actual engine verdict (not preference). Apple→Whisper fallback
/// reports Whisper. When final-pass was **Skipped**, label the **live** lane that
/// served (not a hardcode `streaming_whisper` — that laundered Apple live into a
/// Whisper label; report 2026-07-25 / footer-chip doctrine).
pub(crate) fn engine_label_from_verdict(
    engine: &codescribe_core::pipeline::contracts::TranscriptionEngineVerdict,
    final_pass_disposition: Option<FinalPassDisposition>,
) -> String {
    use codescribe_core::pipeline::contracts::TranscriptionEngine;
    if matches!(final_pass_disposition, Some(FinalPassDisposition::Skipped)) {
        return match engine.engine {
            TranscriptionEngine::Apple => "live_apple".to_string(),
            TranscriptionEngine::Whisper => "streaming_whisper".to_string(),
        };
    }
    match engine.engine {
        TranscriptionEngine::Apple => "local_apple".to_string(),
        // Fallback-vs-primary provenance travels in verdict mode/disposition;
        // the label states the engine that actually served.
        TranscriptionEngine::Whisper => "local_whisper".to_string(),
    }
}

/// Named stop-path phase timings (rec_stop → final_pass → postproc → format → delivery).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct StopPathBudget {
    /// Wall time of the whole stop path; the named phases must fit inside it.
    pub total_secs: f64,
    /// Recorder teardown: stopping capture and sealing the audio file.
    pub rec_stop_secs: f64,
    /// Whisper final pass, whatever `FinalPassAction` selected (0 when skipped).
    pub final_pass_secs: f64,
    /// Transcript post-processing (dictionary / lexicon cleanup).
    pub postproc_secs: f64,
    /// AI formatting pass, when enabled.
    pub format_secs: f64,
    /// Actual deliver_once / history+paste handoff span (not phase-4 cleanup).
    pub delivery_secs: f64,
}

impl StopPathBudget {
    /// Total of the five named phases, excluding unattributed wall time.
    pub(crate) fn named_sum_secs(self) -> f64 {
        self.rec_stop_secs
            + self.final_pass_secs
            + self.postproc_secs
            + self.format_secs
            + self.delivery_secs
    }

    /// Wall time not attributed to a named phase (adjudication overhead, cleanup, …).
    pub(crate) fn unclassified_remainder_secs(self) -> f64 {
        (self.total_secs - self.named_sum_secs()).max(0.0)
    }
}

/// Single INFO summary line for stop-path wall time (W11-B budget receipt).
pub(crate) fn format_stop_path_budget_line(budget: StopPathBudget) -> String {
    format!(
        "stop_path_budget: total={total:.3}s phases={{rec_stop={rec:.3}s,final_pass={fp:.3}s,postproc={pp:.3}s,format={fmt:.3}s,delivery={del:.3}s}} remainder={rem:.3}s",
        total = budget.total_secs,
        rec = budget.rec_stop_secs,
        fp = budget.final_pass_secs,
        pp = budget.postproc_secs,
        fmt = budget.format_secs,
        del = budget.delivery_secs,
        rem = budget.unclassified_remainder_secs(),
    )
}

/// Phase sum + remainder must cover total within tolerance (timing noise).
#[cfg(test)]
pub(crate) fn stop_path_budget_covers_total(budget: StopPathBudget, tolerance_secs: f64) -> bool {
    let covered = budget.named_sum_secs() + budget.unclassified_remainder_secs();
    (covered - budget.total_secs).abs() <= tolerance_secs
}

/// Correlated assistive-delivery receipt. Receipt boundary (audit F1): the
/// `stop_path_budget` closes when the stop pipeline returns; assistive overlay
/// submission is user-triggered *after* that, so its real send gets its own
/// wall-clock line instead of being folded into a budget that already ended.
pub(crate) fn format_assistive_delivery_budget_line(total_secs: f64, outcome: &str) -> String {
    format!("assistive_delivery_budget: total={total_secs:.3}s outcome={outcome}")
}

/// Per-stage split of the stop-path final pass (A2 latency truth).
///
/// `queue_ms` = spawn_blocking queue wait + engine mutex wait. The first three
/// stages plus `engine_overhead_ms` (audio load + VAD + silero + lexicon,
/// computed as remainder) cover `final_pass_total_ms` exactly; postprocess and
/// delivery are the downstream pipeline stages of the same stop.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct FinalPassStages {
    /// Stop-time progressive seal work. Healthy live sessions do this before
    /// controller stop, so the expected value here is zero.
    pub seal_ms: u64,
    /// Pure seals + session-partial residual composition at stop.
    pub residual_ms: u64,
    pub queue_ms: u64,
    pub model_load_ms: u64,
    pub cold_load: bool,
    pub inference_ms: u64,
    pub postprocess_ms: u64,
    pub delivery_ms: u64,
    pub final_pass_total_ms: u64,
}

impl FinalPassStages {
    /// Final-pass wall time not attributed to queue/load/inference: audio file
    /// load, VAD, silero tail filter, lexicon final pass.
    pub(crate) fn engine_overhead_ms(self) -> u64 {
        self.final_pass_total_ms.saturating_sub(
            self.seal_ms
                + self.residual_ms
                + self.queue_ms
                + self.model_load_ms
                + self.inference_ms,
        )
    }
}

/// Single INFO line carrying the final-pass stage split for one stop.
pub(crate) fn format_final_pass_stages_line(stages: FinalPassStages) -> String {
    format!(
        "final_pass_stages seal_ms={seal} residual_ms={residual} queue_ms={queue} model_load_ms={load} cold_load={cold} inference_ms={inf} engine_overhead_ms={overhead} postprocess_ms={pp} delivery_ms={del} final_pass_total_ms={total}",
        seal = stages.seal_ms,
        residual = stages.residual_ms,
        queue = stages.queue_ms,
        load = stages.model_load_ms,
        cold = stages.cold_load,
        inf = stages.inference_ms,
        overhead = stages.engine_overhead_ms(),
        pp = stages.postprocess_ms,
        del = stages.delivery_ms,
        total = stages.final_pass_total_ms,
    )
}

/// What the stop path must actually do with Whisper — typed, not a lossy bool.
///
/// The bool `should_skip_full_final_repass` could not distinguish "run Whisper over
/// the whole file" from "transcribe only the uncommitted tail and append it"; both
/// collapsed to `false`. That collapse is what let Smart drift into full-file
/// re-passes, replacing committed text. This enum makes the two paths distinct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FinalPassAction {
    /// Do not invoke Whisper at stop. Streaming (+ post-process) is final.
    SkipStreamingFinal,
    /// Re-transcribe the entire recorded file. **Always mode only.**
    FullFileRepass,
    /// Transcribe only the uncommitted audio tail (from the last committed
    /// utterance end) and APPEND it to the committed streaming text. Committed
    /// text is immutable.
    TailGapFill,
    /// Run Whisper over the whole file, but adopt ONLY punctuation and
    /// capitalization onto the committed words via word alignment
    /// (`codescribe_core::stt::punctuation_transplant`). The committed word
    /// sequence is invariant — this is shape transfer, not a re-pass; a weak
    /// alignment delivers the committed text untouched.
    PunctuationRepass,
}

/// Canonical stop-path routing decision. Settings / `FINAL_PASS_MODE` is law.
///
/// Hard mapping (operator 2026-08-05, shape row added 2026-08-09):
/// - `Always` → `FullFileRepass`, regardless of completeness.
/// - `Smart` + `Complete` → `SkipStreamingFinal`.
/// - `Smart` + `CompleteShapeDeficient` → `PunctuationRepass` (words stay
///   committed; only sentence shape is adopted — the operator's answer to a
///   1052-char delivery carrying one period while Whisper heard nine).
/// - `Smart` + `Incomplete{..}` → `TailGapFill` (per-utterance / tail only,
///   **never** a full-file re-pass).
/// - `Off` → `SkipStreamingFinal`, regardless of completeness.
///
/// `FullFileRepass` is reachable from `Always` and from nowhere else — the live
/// engine (Apple vs Whisper) must never rewrite the mode.
pub(crate) fn final_pass_action(
    mode: FinalPassRoutingMode,
    completeness: StreamingCompleteness,
) -> FinalPassAction {
    match mode {
        FinalPassRoutingMode::Always => FinalPassAction::FullFileRepass,
        FinalPassRoutingMode::Off => FinalPassAction::SkipStreamingFinal,
        FinalPassRoutingMode::Smart => match completeness {
            StreamingCompleteness::Complete => FinalPassAction::SkipStreamingFinal,
            StreamingCompleteness::CompleteShapeDeficient => FinalPassAction::PunctuationRepass,
            StreamingCompleteness::Incomplete { .. } => FinalPassAction::TailGapFill,
        },
    }
}

/// The append-only tail gap-fill composer now lives in core
/// (`codescribe_core::stt::append_tail_gap`) so **every** Smart-mode stop lane —
/// this controller and the composer voice-note lane in the UniFFI bridge —
/// shares one implementation instead of a per-lane twin. Re-exported at the
/// historic path so controller call sites and tests stay unchanged.
pub(crate) use codescribe_core::stt::append_tail_gap;

/// Progressive-seal residual stop path: consume session partials instead of
/// re-transcribing the file. Measured 2026-08-10 baseline for the hated
/// full-file final_pass was 8.458 s — residual composition is pure string work.
///
/// File-inference fallback (PunctuationRepass / TailGapFill audio cut) is
/// reserved for a dead live lane. See
/// [`codescribe_core::pipeline::streaming::progressive_seal`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StopPathResidualOutcome {
    pub text: String,
    pub sealed_prefix: String,
    pub residual_tail: String,
    pub used_file_decode_fallback: bool,
    /// Wall seconds of the residual phase (string composition, not Whisper).
    pub final_pass_phase_secs: f64,
}

/// Compose delivered text from progressive seals + residual session partials.
///
/// Never invokes Whisper. Returns `used_file_decode_fallback = false` always
/// — callers that need file inference must take the existing TailGapFill /
/// PunctuationRepass arms after [`residual_prefers_session_partials`] refuses.
pub(crate) fn compose_stop_path_residual_from_partials(
    sealed_spans: &[String],
    residual_partial: &str,
) -> StopPathResidualOutcome {
    let started = std::time::Instant::now();
    let sealed_prefix = sealed_spans
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let residual_tail = residual_partial.trim().to_string();
    let text = append_tail_gap(&sealed_prefix, &residual_tail);
    StopPathResidualOutcome {
        text,
        sealed_prefix,
        residual_tail,
        used_file_decode_fallback: false,
        final_pass_phase_secs: started.elapsed().as_secs_f64(),
    }
}

/// Prefer session-partial residual over file re-decode when the live lane is
/// healthy and partials cover the unsealed tail.
pub(crate) fn residual_prefers_session_partials(
    live_lane_alive: bool,
    has_residual_partials: bool,
) -> bool {
    codescribe_core::pipeline::streaming::progressive_seal::residual_prefers_session_partials(
        live_lane_alive,
        has_residual_partials,
    )
}

/// Concrete source selected for Smart-mode incomplete-tail completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SmartTailGapSource {
    /// The live session already produced progressive seals/residual text.
    SessionResidual,
    /// Bootstrap or dead-lane recovery may decode the retained WAV.
    FileDecodeFallback,
    /// No usable live text and no file safety net.
    Unavailable,
}

/// Select the Smart tail source before branching on the existence of a WAV.
/// This ordering is the contract: a normal recorder always has a WAV, so
/// checking the path first would make the live residual permanently dead.
pub(crate) fn smart_tail_gap_source(
    live_lane_alive: bool,
    live_session_text: &str,
    has_audio_path: bool,
) -> SmartTailGapSource {
    if residual_prefers_session_partials(live_lane_alive, !live_session_text.trim().is_empty()) {
        SmartTailGapSource::SessionResidual
    } else if has_audio_path {
        SmartTailGapSource::FileDecodeFallback
    } else {
        SmartTailGapSource::Unavailable
    }
}

#[cfg(test)]
mod stop_path_integration_tests {
    use super::*;

    /// Smart stop must consume the live session residual even when a WAV path
    /// exists. File decode is reserved for bootstrap/dead-lane recovery.
    #[test]
    fn smart_live_session_residual_preempts_wav_decode() {
        let source = smart_tail_gap_source(true, "Pierwsze. Otwarty ogon", true);
        assert_eq!(source, SmartTailGapSource::SessionResidual);

        let bootstrap = smart_tail_gap_source(false, "", true);
        assert_eq!(bootstrap, SmartTailGapSource::FileDecodeFallback);
    }
}
