//! Phase-2 correction pass: partial-pass trigger policy and telemetry,
//! correction scheduling against the Refine lane, and reconciliation of
//! corrected text with preview/boundary state.

use std::sync::Arc;

use tokio::time::Instant;
use tracing::{debug, error};

use crate::pipeline::contracts::{EngineEvent, EventSink};
use crate::stt::scheduler::{SttLane, SttScheduler, SttTaskHandle};

use super::pipeline::{PostprocessDrop, TranscriptionPipeline};
use super::quality_gate::silero_vad_samples_to_ms;

// Partial correction should feel "live" in overlay, not lag by multiple turns.
// Trigger earlier to improve retranscription visibility in hands-off sessions.
pub(crate) const PARTIAL_PASS_TRIGGER_UTTERANCE_FINALS: u32 = 1;
pub(crate) const PARTIAL_PASS_TRIGGER_SILERO_SPEECH_MS: u64 = 1_800;
pub(crate) const PARTIAL_PASS_TRIGGER_TIMER_MS: u64 = 3_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PartialPassTrigger {
    Utterance,
    Speech,
    Timer,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PartialPassTriggerFlags {
    pub(crate) utterance_finals: bool,
    pub(crate) silero_speech: bool,
    pub(crate) timer: bool,
}

impl PartialPassTriggerFlags {
    pub(crate) fn primary_reason(self) -> Option<PartialPassTrigger> {
        if self.utterance_finals {
            Some(PartialPassTrigger::Utterance)
        } else if self.silero_speech {
            Some(PartialPassTrigger::Speech)
        } else if self.timer {
            Some(PartialPassTrigger::Timer)
        } else {
            None
        }
    }
}

#[derive(Debug)]
pub(crate) struct PartialPassTriggerState {
    pub(crate) utterance_finals_since_partial: u32,
    pub(crate) silero_speech_ms_since_partial: u64,
    pub(crate) timer_baseline: Instant,
}

impl PartialPassTriggerState {
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            utterance_finals_since_partial: 0,
            silero_speech_ms_since_partial: 0,
            timer_baseline: now,
        }
    }

    pub(crate) fn observe_speech_event(&mut self, is_final: bool, silero_speech_vad_samples: u64) {
        if is_final {
            self.utterance_finals_since_partial =
                self.utterance_finals_since_partial.saturating_add(1);
        }
        self.silero_speech_ms_since_partial = self
            .silero_speech_ms_since_partial
            .saturating_add(silero_vad_samples_to_ms(silero_speech_vad_samples));
    }

    pub(crate) fn evaluate(&self, now: Instant) -> PartialPassTriggerFlags {
        let timer_elapsed_ms = now.duration_since(self.timer_baseline).as_millis() as u64;
        PartialPassTriggerFlags {
            utterance_finals: self.utterance_finals_since_partial
                >= PARTIAL_PASS_TRIGGER_UTTERANCE_FINALS,
            silero_speech: self.silero_speech_ms_since_partial
                >= PARTIAL_PASS_TRIGGER_SILERO_SPEECH_MS,
            timer: timer_elapsed_ms >= PARTIAL_PASS_TRIGGER_TIMER_MS,
        }
    }

    pub(crate) fn reset_after_success(&mut self, now: Instant) {
        self.utterance_finals_since_partial = 0;
        self.silero_speech_ms_since_partial = 0;
        self.timer_baseline = now;
    }
}

fn silero_speech_seconds(speech_ms: u64) -> f32 {
    speech_ms as f32 / 1_000.0
}

/// Run correction postprocess against a snapshot suffix without permanently
/// mutating pipeline suffix state on failure.
pub(crate) fn postprocess_correction_with_snapshot(
    pipeline: &mut TranscriptionPipeline,
    raw_text: &str,
    suffix_snapshot: &str,
) -> std::result::Result<String, PostprocessDrop> {
    let current_suffix = pipeline.last_suffix.clone();
    pipeline.last_suffix = suffix_snapshot.to_string();
    match pipeline.postprocess_with_reason(raw_text) {
        Ok(cleaned) => Ok(cleaned),
        Err(drop) => {
            pipeline.last_suffix = current_suffix;
            Err(drop)
        }
    }
}

pub(crate) fn correction_is_stale(
    expected_boundary_rev: u64,
    current_boundary_rev: u64,
    _expected_text: &str,
    _current_text: &str,
) -> bool {
    expected_boundary_rev != current_boundary_rev
}

/// Build correction baseline text for replacement semantics across boundaries.
///
/// Returns `(baseline_text, correction_after_final_boundary)` where
/// `correction_after_final_boundary` indicates that utterance-local preview state
/// was already cleared by a boundary commit.
pub(crate) fn correction_baseline_text(
    accumulated_text: &str,
    expected_text: &str,
    window_text: &str,
) -> (String, bool) {
    if !accumulated_text.trim().is_empty() {
        return (accumulated_text.to_string(), false);
    }
    if !expected_text.trim().is_empty() {
        return (expected_text.to_string(), true);
    }
    if !window_text.trim().is_empty() {
        return (window_text.to_string(), true);
    }
    (String::new(), true)
}

/// Apply final boundary text while preserving a non-empty preview fallback.
///
/// Returns `true` when a boundary has usable content after reconciliation.
pub(crate) fn apply_final_boundary_text(
    accumulated_text: &mut String,
    cleaned_final: &str,
) -> bool {
    let cleaned = cleaned_final.trim();
    if cleaned.is_empty() {
        !accumulated_text.trim().is_empty()
    } else {
        *accumulated_text = cleaned.to_string();
        true
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct PartialPassTelemetry {
    pub(crate) runs_total: u64,
    pub(crate) trigger_utterance_count: u64,
    pub(crate) trigger_speech_count: u64,
    pub(crate) trigger_timer_count: u64,
    pub(crate) stale_count: u64,
    pub(crate) coalesced_count: u64,
    pub(crate) dropped_count: u64,
}

impl PartialPassTelemetry {
    pub(crate) fn record_run(&mut self, trigger: PartialPassTrigger) {
        self.runs_total = self.runs_total.saturating_add(1);
        match trigger {
            PartialPassTrigger::Utterance => {
                self.trigger_utterance_count = self.trigger_utterance_count.saturating_add(1);
            }
            PartialPassTrigger::Speech => {
                self.trigger_speech_count = self.trigger_speech_count.saturating_add(1);
            }
            PartialPassTrigger::Timer => {
                self.trigger_timer_count = self.trigger_timer_count.saturating_add(1);
            }
        }
    }

    pub(crate) fn record_stale(&mut self) {
        self.stale_count = self.stale_count.saturating_add(1);
    }

    pub(crate) fn record_coalesced(&mut self) {
        self.coalesced_count = self.coalesced_count.saturating_add(1);
    }

    pub(crate) fn record_dropped(&mut self) {
        self.dropped_count = self.dropped_count.saturating_add(1);
    }
}

pub(crate) fn classify_partial_trigger(
    flags: PartialPassTriggerFlags,
) -> Option<PartialPassTrigger> {
    flags.primary_reason()
}

// allow(too_many_arguments): hot-path seam between the audio loop and the STT
// scheduler; 15 discrete knobs are threaded through by design today. The
// honest fix is a PartialPassCtx struct — deferred to the streaming.rs
// decomposition cut (tracked in prune report follow-ups).
#[allow(clippy::too_many_arguments)]
pub(crate) fn schedule_partial_pass(
    stt_scheduler: &SttScheduler,
    output_sample_rate: u32,
    pipeline_language: Option<String>,
    correction_audio_buf: &mut Vec<f32>,
    correction_in_flight: &mut Option<SttTaskHandle>,
    correction_expected_boundary_rev: &mut Option<u64>,
    correction_expected_text: &mut Option<String>,
    correction_suffix_snapshot: &mut Option<String>,
    suffix_snapshot: &str,
    boundary_rev: u64,
    baseline_text: &str,
    speech_ms_since_partial: u64,
    trigger: PartialPassTrigger,
    partial_telemetry: &mut PartialPassTelemetry,
    event_sink: &Arc<dyn EventSink>,
) -> bool {
    if correction_audio_buf.is_empty() {
        return false;
    }
    let audio = std::mem::take(correction_audio_buf);
    let audio_duration_s = audio.len() as f32 / output_sample_rate as f32;

    if let Some(old) = correction_in_flight.take() {
        partial_telemetry.record_coalesced();
        debug!(
            dropped_request_id = old.id(),
            dropped_lane = ?old.lane(),
            "Superseding tracked correction request"
        );
    }

    debug!(
        expected_boundary_rev = boundary_rev,
        baseline_len = baseline_text.chars().count(),
        audio_sec = audio_duration_s,
        silero_speech_sec = silero_speech_seconds(speech_ms_since_partial),
        trigger = ?trigger,
        runs_total = partial_telemetry.runs_total,
        "BOUNDARY correction_scheduled"
    );

    match stt_scheduler.submit(
        SttLane::Refine,
        audio,
        output_sample_rate,
        pipeline_language,
    ) {
        Ok(handle) => {
            partial_telemetry.record_run(trigger);
            *correction_expected_boundary_rev = Some(boundary_rev);
            *correction_expected_text = Some(baseline_text.to_string());
            *correction_suffix_snapshot = Some(suffix_snapshot.to_string());
            *correction_in_flight = Some(handle);
            true
        }
        Err(e) => {
            partial_telemetry.record_dropped();
            error!("Failed to submit correction request: {}", e);
            event_sink.on_event(&EngineEvent::Warning {
                code: "scheduler_submit_error".to_string(),
                message: format!("{}", e),
            });
            false
        }
    }
}
