//! Auto-paste policy, quality-commit probes, automatic delivery fence, and
//! transcript wrapping for delivery / truth status composition.

use std::collections::VecDeque;
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use tokio::sync::Mutex;

use crate::config::Config;
use crate::os::clipboard;
use crate::os::hotkeys::HoldMode;

use super::types::RecordingTruthMetadata;

/// Ring size of the exactly-once delivery fence — bounds memory while still
/// covering late duplicate callbacks for recent recordings.
const AUTOMATIC_DELIVERY_HISTORY_CAPACITY: usize = 32;

/// Below this transcript length the quality gate stays silent: short utterances
/// produce noisy ratios that would fire the commit prompt on nothing.
const QUALITY_GATE_MIN_CHARS: usize = 24;
/// Relaxed floor for AI-formatted output, where even short text is worth
/// gating because formatting can rewrite it wholesale.
const SHORT_AI_QUALITY_GATE_MIN_CHARS: usize = 10;
/// Fraction of dropped stream chunks that marks the transcript as lossy.
const QUALITY_GATE_DROP_RATIO: f32 = 0.35;
/// Fraction of the transcript rewritten between raw and final that reads as a
/// rewrite rather than a correction.
const QUALITY_GATE_DIFF_RATIO: f32 = 0.62;
/// Fraction of raw characters removed by backspaces that reads as heavy
/// correction pressure.
const QUALITY_GATE_CORRECTION_RATIO: f32 = 0.40;

/// Measured distance between the raw transcript and what is about to be
/// delivered. Feeds [`evaluate_quality_commit_trigger`].
#[derive(Debug, Clone, Copy)]
pub(super) struct ActionQualityProbe {
    // pub(crate): controller impl + controller::tests (child) both read these.
    pub(crate) raw_chars: usize,
    pub(crate) final_chars: usize,
    pub(crate) raw_final_diff_ratio: f32,
    pub(crate) correction_ratio: f32,
    pub(crate) drop_ratio: f32,
    /// MiniLM cosine between the raw transcript and the AI-formatted output.
    /// `None` = no verdict (guard fail-open, raw lane, or text too short) —
    /// character ratios alone cannot see an LLM that changed the meaning while
    /// keeping the length (`core::pipeline::semantic_guard`, calibrated
    /// 2026-08-09). Filled after construction because embedding is model work,
    /// not arithmetic.
    pub(crate) semantic_cosine: Option<f32>,
}

/// Normalize a transcript before diffing so leading whitespace and a
/// capitalized opening letter do not register as real edits.
fn normalize_for_diff(s: &str) -> String {
    let trimmed = s.trim_start();
    // Lowercase first char only (preserving rest of original case)
    let mut chars = trimmed.chars();
    match chars.next() {
        Some(c) => c.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

impl ActionQualityProbe {
    /// Measure raw-vs-final distance plus stream drop rate into a single probe.
    ///
    /// The diff runs over normalized text and is split into backspaces versus
    /// inserted characters so a rewrite and a correction can be told apart.
    pub(super) fn from_transcripts(
        raw_text: &str,
        final_text: &str,
        post_stats: &crate::stream_postprocess::StreamPostProcessStats,
    ) -> Self {
        let raw_chars = raw_text.chars().count();
        let final_chars = final_text.chars().count();

        let (backspaces, inserted_chars) =
            codescribe_core::pipeline::contracts::TranscriptDelta::from_diff(
                &normalize_for_diff(raw_text),
                &normalize_for_diff(final_text),
            )
            .map(|delta| {
                let backspaces = delta
                    .delta
                    .chars()
                    .filter(|c| *c == codescribe_core::pipeline::contracts::BACKSPACE)
                    .count();
                let inserted = delta.delta.chars().count().saturating_sub(backspaces);
                (backspaces, inserted)
            })
            .unwrap_or((0, 0));

        let span = raw_chars.max(final_chars).max(1);
        let raw_final_diff_ratio = ((backspaces + inserted_chars) as f32 / span as f32).min(1.0);
        let correction_ratio = (backspaces as f32 / raw_chars.max(1) as f32).min(1.0);
        let drop_ratio = if post_stats.input_chunks == 0 {
            0.0
        } else {
            post_stats.dropped_chunks as f32 / post_stats.input_chunks as f32
        };

        Self {
            raw_chars,
            final_chars,
            raw_final_diff_ratio,
            correction_ratio,
            drop_ratio,
            semantic_cosine: None,
        }
    }
}

/// Which gesture ended the recording that is now up for auto-paste.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AutoPasteTrigger {
    /// Push-to-talk hold released.
    Hold,
    /// Double-tap of the left Option key.
    DoubleLeftOption,
}

/// Everything the auto-paste decision is allowed to depend on, gathered in one
/// place so the policy stays a pure function of explicit state.
#[derive(Debug, Clone, Copy)]
pub(super) struct AutoPastePolicyContext {
    pub trigger: AutoPasteTrigger,
    pub persisted_enabled: bool,
    pub overlay_enabled: bool,
    pub assistive: bool,
    pub no_speech: bool,
    pub empty_output: bool,
    pub notes_save_only: bool,
    pub live_stream_session: bool,
    pub commit_required: bool,
}

/// Decide whether the delivered transcript may be pasted automatically.
///
/// Every veto is independent and fail-closed: assistive sessions, no-speech and
/// empty results, notes-only saves, live streaming sessions, and pending quality
/// commits each suppress the paste on their own.
pub(super) fn resolve_auto_paste_policy(context: AutoPastePolicyContext) -> bool {
    // Trigger and presentation state deliberately do not fork policy. Keeping
    // the explicit matrix here makes that parity reviewable and testable.
    let persisted_policy = match (context.trigger, context.overlay_enabled) {
        (AutoPasteTrigger::Hold, true | false)
        | (AutoPasteTrigger::DoubleLeftOption, true | false) => context.persisted_enabled,
    };

    persisted_policy
        && !context.assistive
        && !context.no_speech
        && !context.empty_output
        && !context.notes_save_only
        && !context.live_stream_session
        && !context.commit_required
}

/// Delivery target for automatic paste, injected so the fence can be tested
/// without touching the real pasteboard.
pub(super) trait AutomaticDeliverySink: Send + Sync {
    /// Deliver `text` to the active application. Errors stay retryable.
    fn paste(&self, text: &str) -> Result<()>;
}

/// Production sink: pastes through the system clipboard.
pub(super) struct ClipboardDeliverySink;

impl AutomaticDeliverySink for ClipboardDeliverySink {
    /// Paste delivery path: type/paste corrected text into the frontmost app.
    fn paste(&self, text: &str) -> Result<()> {
        clipboard::paste_text(text).context("Failed to paste text")
    }
}

/// Controller-owned, bounded exactly-once fence. A small ring retains recent
/// recording timestamps so late duplicate callbacks remain fenced without
/// process-global or unbounded growth. A failed sink call remains retryable.
pub(super) struct AutomaticDeliveryOwner {
    delivered_timestamps: Mutex<VecDeque<DateTime<Local>>>,
    sink: Arc<dyn AutomaticDeliverySink>,
}

impl AutomaticDeliveryOwner {
    /// Build an owner delivering through `sink`, with an empty fence history.
    pub(super) fn new(sink: Arc<dyn AutomaticDeliverySink>) -> Self {
        Self {
            delivered_timestamps: Mutex::new(VecDeque::with_capacity(
                AUTOMATIC_DELIVERY_HISTORY_CAPACITY,
            )),
            sink,
        }
    }

    /// Deliver `text` at most once per recording `timestamp`.
    ///
    /// Returns `Ok(false)` when the timestamp was already delivered. A sink
    /// failure propagates and leaves the timestamp unfenced, so the caller may
    /// retry the same recording.
    pub(super) async fn deliver_once(
        &self,
        timestamp: DateTime<Local>,
        text: &str,
    ) -> Result<bool> {
        let mut delivered = self.delivered_timestamps.lock().await;
        if delivered.contains(&timestamp) {
            return Ok(false);
        }

        self.sink.paste(text)?;
        if delivered.len() == AUTOMATIC_DELIVERY_HISTORY_CAPACITY {
            delivered.pop_front();
        }
        delivered.push_back(timestamp);
        Ok(true)
    }
}

/// Short label describing how a recording was made, for history and UI.
pub(super) fn recording_mode_label(
    assistive: bool,
    hold_mode: HoldMode,
    force_raw: bool,
    force_ai: bool,
) -> &'static str {
    if assistive {
        match hold_mode {
            HoldMode::Chat => "chat",
            HoldMode::Selection => "selection",
            HoldMode::Raw => "assistive",
        }
    } else if force_raw {
        "raw"
    } else if force_ai {
        "format"
    } else {
        "toggle"
    }
}

/// Mode label for the recording-truth record, where every assistive lane
/// collapses to `assistive` regardless of the hold gesture beneath it.
pub(super) fn truth_recording_mode_label(
    assistive: bool,
    hold_mode: HoldMode,
    force_raw: bool,
    force_ai: bool,
) -> &'static str {
    if assistive {
        "assistive"
    } else {
        recording_mode_label(assistive, hold_mode, force_raw, force_ai)
    }
}

/// Whether AI formatting runs for this session: an explicit force wins, then
/// the persisted config default.
pub(super) fn session_auto_format_enabled(
    config: &Config,
    _assistive: bool,
    force_raw: bool,
    force_ai: bool,
) -> bool {
    force_ai || (!force_raw && config.ai_formatting_enabled)
}

/// Wrap a transcript in the configured tag template, without quality metadata.
pub(super) fn maybe_wrap_transcript_for_delivery(
    text: &str,
    config: &Config,
    mode: &str,
) -> String {
    maybe_wrap_transcript_for_delivery_with_quality(text, config, mode, None)
}

/// Wrap a transcript in the configured tag template, folding in confidence
/// flags and `avg_logprob` when recording-truth metadata is available.
///
/// Returns the text untouched when tagging is disabled.
pub(super) fn maybe_wrap_transcript_for_delivery_with_quality(
    text: &str,
    config: &Config,
    mode: &str,
    metadata: Option<&RecordingTruthMetadata>,
) -> String {
    if !config.transcript_tagging_enabled {
        return text.to_string();
    }

    let confidence_flags = metadata
        .map(|metadata| {
            metadata
                .confidence_flags
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    codescribe_core::transcript_tagging::wrap_transcript_with_quality(
        text,
        &config.transcript_tag_template,
        mode,
        config.whisper_language.as_str(),
        metadata.and_then(|metadata| metadata.avg_logprob),
        &confidence_flags,
    )
}

/// Human-readable category for a transcript kind, shown in delivery status.
pub(super) fn transcript_output_category(
    output_kind: crate::state::history::TranscriptKind,
) -> &'static str {
    match output_kind {
        crate::state::history::TranscriptKind::Raw => "Transcript",
        crate::state::history::TranscriptKind::Cloud => "Cloud transcript",
        crate::state::history::TranscriptKind::FormattedTranscript => "Formatted transcript",
        crate::state::history::TranscriptKind::AssistantInterpretation => {
            "Assistant interpretation"
        }
        crate::state::history::TranscriptKind::FormattingFailed => {
            "Formatting failed, raw preserved"
        }
        crate::state::history::TranscriptKind::Failed => "Failed transcript",
    }
}

/// Join the display status with the output category for the final status line.
///
/// Failed transcripts keep the bare display status — appending "Failed
/// transcript" to an already-explicit error reads as a double report.
pub(super) fn compose_final_status(
    display_status: &str,
    output_kind: crate::state::history::TranscriptKind,
) -> String {
    if display_status.trim().is_empty() {
        return transcript_output_category(output_kind).to_string();
    }

    match output_kind {
        crate::state::history::TranscriptKind::Failed => display_status.to_string(),
        _ => format!(
            "{} • {}",
            display_status,
            transcript_output_category(output_kind)
        ),
    }
}

/// Decide whether this delivery should ask the user to commit a correction.
///
/// Returns the trigger reason, or `None` when the transcript is too short to
/// judge, raw output was forced, or every ratio stayed under its threshold.
pub(super) fn evaluate_quality_commit_trigger(
    force_raw: bool,
    quality_probe: &ActionQualityProbe,
    output_kind: crate::state::history::TranscriptKind,
) -> Option<&'static str> {
    let short_ai_formatted = output_kind
        == crate::state::history::TranscriptKind::FormattedTranscript
        && quality_probe.raw_chars.max(quality_probe.final_chars)
            >= SHORT_AI_QUALITY_GATE_MIN_CHARS;
    if force_raw {
        return None;
    }
    if output_kind == crate::state::history::TranscriptKind::FormattingFailed {
        return Some("ai_failed_fallback");
    }
    if quality_probe.raw_chars < QUALITY_GATE_MIN_CHARS
        && quality_probe.final_chars < QUALITY_GATE_MIN_CHARS
        && !short_ai_formatted
    {
        return None;
    }
    // Meaning axis first — it is the sharpest betrayal the ratios cannot see.
    // The floor is calibrated (core::pipeline::semantic_guard): formatting that
    // keeps the meaning scores ≥0.950, catchable betrayals ≤0.776. Same-word
    // inversions are a documented blindspot, not a promise.
    if let Some(cosine) = quality_probe.semantic_cosine
        && codescribe_core::pipeline::semantic_guard::formatting_meaning_diverged(cosine)
    {
        return Some("semantic_divergence");
    }
    if quality_probe.drop_ratio >= QUALITY_GATE_DROP_RATIO {
        return Some("high_drop_ratio");
    }
    if quality_probe.raw_final_diff_ratio >= QUALITY_GATE_DIFF_RATIO {
        return Some("high_rewrite_ratio");
    }
    if quality_probe.correction_ratio >= QUALITY_GATE_CORRECTION_RATIO {
        return Some("high_correction_ratio");
    }
    None
}
