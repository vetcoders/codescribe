//! Stop-path final-pass routing, streaming completeness, and latency receipts.
//!
//! Settings / `FINAL_PASS_MODE` is law (operator 2026-08-05):
//! - **Always** (`on`): full Whisper file re-pass after release. The **only**
//!   mode in which a full-file re-pass is permitted.
//! - **Smart**: Whisper may final-pass **individual utterances only** — never the
//!   whole file. When streaming completeness is adjudicated Complete, nothing runs
//!   at stop. When incomplete, only the uncommitted audio tail (from the last
//!   committed utterance end) is transcribed and **appended** to the committed
//!   streaming text; committed text stays immutable (append-only doctrine).
//!   Pair with `CODESCRIBE_LAYERED_TRANSCRIPTION` (orthogonal toggle) for live
//!   Whisper tail-patches during hold — Smart does not force layered on.
//! - **Off**: hard off at stop — zero Whisper invocation on the stop path;
//!   streaming (+ post-process) is final.
//!
//! Dictionary / lexicon cleanup is **not** gated by this mode — it always runs
//! in the transcript post-process pipeline (`StreamPostProcessor`), in all modes.

use codescribe_core::pipeline::contracts::FinalPassDisposition;

use super::helpers::{CompletenessCommitSource, SessionTelemetrySnapshot};

/// Canonical stop-path final-pass routing (Settings → Final pass / `FINAL_PASS_MODE`).
/// Distinct from `pipeline::contracts::FinalPassMode` (lexicon cleanup request).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FinalPassRoutingMode {
    /// Always: full Whisper file re-pass after release (on).
    Always,
    /// Smart: tail-patch / layered live path; full re-pass only when incomplete.
    Smart,
    /// Off: no full file re-pass; streaming is final (dictionary still applies later).
    Off,
}

impl FinalPassRoutingMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Smart => "smart",
            Self::Off => "off",
        }
    }

    pub(crate) fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "always" | "on" | "1" | "true" | "yes" => Some(Self::Always),
            "smart" | "auto" => Some(Self::Smart),
            "off" | "0" | "false" | "no" => Some(Self::Off),
            _ => None,
        }
    }
}

/// Resolve final-pass routing from env/settings. Default: Smart.
///
/// Precedence:
/// 1. `FINAL_PASS_MODE` / `CODESCRIBE_FINAL_PASS_MODE` (`always|smart|off`)
/// 2. Legacy `CODESCRIBE_LOCAL_STT_FINAL_PASS` falsey → Off, truthy → Always
/// 3. Smart
pub(crate) fn final_pass_routing_mode() -> FinalPassRoutingMode {
    for key in ["FINAL_PASS_MODE", "CODESCRIBE_FINAL_PASS_MODE"] {
        if let Ok(raw) = std::env::var(key)
            && let Some(mode) = FinalPassRoutingMode::parse(&raw)
        {
            return mode;
        }
    }
    if let Ok(raw) = std::env::var("CODESCRIBE_LOCAL_STT_FINAL_PASS") {
        let v = raw.trim().to_ascii_lowercase();
        if matches!(v.as_str(), "" | "0" | "false" | "no" | "off") {
            return FinalPassRoutingMode::Off;
        }
        if matches!(v.as_str(), "1" | "true" | "yes" | "on") {
            return FinalPassRoutingMode::Always;
        }
    }
    FinalPassRoutingMode::Smart
}

/// Typed streaming-completeness decision for Smart skip (never "non-empty ⇒ complete").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamingCompleteness {
    Complete,
    Incomplete { reason: &'static str },
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
    pub total_secs: f64,
    pub rec_stop_secs: f64,
    pub final_pass_secs: f64,
    pub postproc_secs: f64,
    pub format_secs: f64,
    /// Actual deliver_once / history+paste handoff span (not phase-4 cleanup).
    pub delivery_secs: f64,
}

impl StopPathBudget {
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
        self.final_pass_total_ms
            .saturating_sub(self.queue_ms + self.model_load_ms + self.inference_ms)
    }
}

/// Single INFO line carrying the final-pass stage split for one stop.
pub(crate) fn format_final_pass_stages_line(stages: FinalPassStages) -> String {
    format!(
        "final_pass_stages queue_ms={queue} model_load_ms={load} cold_load={cold} inference_ms={inf} engine_overhead_ms={overhead} postprocess_ms={pp} delivery_ms={del} final_pass_total_ms={total}",
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
}

/// Canonical stop-path routing decision. Settings / `FINAL_PASS_MODE` is law.
///
/// Hard mapping (operator 2026-08-05):
/// - `Always` → `FullFileRepass`, regardless of completeness.
/// - `Smart` + `Complete` → `SkipStreamingFinal`.
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
            StreamingCompleteness::Incomplete { .. } => FinalPassAction::TailGapFill,
        },
    }
}

/// Comparison key for overlap detection: lowercased, edge punctuation stripped.
fn overlap_key(word: &str) -> String {
    word.trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase()
}

/// Number of leading `tail` words that repeat the trailing `committed` words.
///
/// Returns the LONGEST such overlap. Comparison is case-insensitive and ignores
/// leading/trailing punctuation; only the count is returned — the caller keeps
/// the tail's original words for whatever remains.
fn leading_overlap_words(committed: &[String], tail: &[String]) -> usize {
    let max = committed.len().min(tail.len());
    for len in (1..=max).rev() {
        let c_start = committed.len() - len;
        if committed[c_start..] == tail[..len] {
            return len;
        }
    }
    0
}

/// Append a tail gap-fill to committed streaming text — **append only**.
///
/// The overlay doctrine (CLAUDE.md, THE ONE RULE): committed text is immutable.
/// Whatever Whisper produced for the uncommitted tail is joined onto the end with
/// exactly one space; the trimmed streaming text is always an untouched prefix of
/// the result. Empty/blank tail → streaming unchanged. Empty/blank streaming → the
/// trimmed tail alone.
///
/// **Overlap dedup**: streaming text can already contain uncommitted preview
/// words for the same audio the tail-gap re-transcribes (`pending_tail`). Before
/// joining, the longest leading word-run of the tail that repeats the trailing
/// words of the streaming text is dropped **from the tail**. The streaming side
/// is never touched — dedup only ever shortens what gets appended.
pub(crate) fn append_tail_gap(streaming: &str, tail: &str) -> String {
    let committed = streaming.trim();
    let addition = tail.trim();
    if addition.is_empty() {
        return committed.to_string();
    }
    if committed.is_empty() {
        return addition.to_string();
    }

    let tail_words: Vec<&str> = addition.split_whitespace().collect();
    let committed_keys: Vec<String> = committed.split_whitespace().map(overlap_key).collect();
    let tail_keys: Vec<String> = tail_words.iter().map(|w| overlap_key(w)).collect();
    let overlap = leading_overlap_words(&committed_keys, &tail_keys);

    let remaining = &tail_words[overlap..];
    if remaining.is_empty() {
        // The tail is entirely contained in the committed suffix: nothing new.
        return committed.to_string();
    }
    format!("{committed} {}", remaining.join(" "))
}
