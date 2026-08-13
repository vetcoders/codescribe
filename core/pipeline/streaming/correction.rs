//! Phase-2 correction pass: partial-pass trigger policy and telemetry,
//! correction scheduling against the Refine lane, and reconciliation of
//! corrected text with preview/boundary state.

use std::sync::Arc;

use tokio::time::Instant;
use tracing::{debug, error};

use crate::pipeline::contracts::{EngineEvent, EventSink};
use crate::stt::scheduler::{SttLane, SttScheduler, SttTaskHandle};
use crate::stt::whisper::{
    VadWindowPlanConfig, merge_chunk_transcripts, plan_vad_aligned_windows_with_config,
    silence_spans_from_vad_probabilities,
};

use super::pipeline::{PostprocessDrop, TranscriptionPipeline};
use super::quality_gate::silero_vad_samples_to_ms;

// Partial correction should feel "live" in overlay, not lag by multiple turns.
// Trigger earlier to improve retranscription visibility in hands-off sessions.
/// Utterance finals since last partial pass required to fire a Refine re-decode.
pub(crate) const PARTIAL_PASS_TRIGGER_UTTERANCE_FINALS: u32 = 1;
/// Silero-measured speech ms since last partial pass to fire a Refine re-decode.
pub(crate) const PARTIAL_PASS_TRIGGER_SILERO_SPEECH_MS: u64 = 1_800;
/// Wall-clock ms since last rearm before the timer watchdog fires a partial pass.
pub(crate) const PARTIAL_PASS_TRIGGER_TIMER_MS: u64 = 3_000;

/// Rolling live-Whisper window calibration from streaming-bridge-v2.
pub(crate) const ROLLING_WINDOW_TARGET_SECS: f32 = 8.0;
/// Fresh audio required between consecutive rolling windows.
pub(crate) const ROLLING_WINDOW_STEP_SECS: f32 = 4.0;
/// Shorter first windows are withheld because Whisper returns `No speech` on
/// the operator fixtures below this floor.
pub(crate) const ROLLING_WINDOW_MIN_SECS: f32 = 6.0;
/// Audio retained after a submission so boundary words are decoded whole in
/// both adjacent windows.
pub(crate) const ROLLING_WINDOW_OVERLAP_SECS: f32 = 4.0;
/// Look-ahead retained so the planner can observe the silence around the
/// target boundary before committing it.
pub(crate) const ROLLING_WINDOW_LOOKAHEAD_SECS: f32 = 2.0;

const ROLLING_WINDOW_PLAN: VadWindowPlanConfig = VadWindowPlanConfig {
    min_secs: ROLLING_WINDOW_MIN_SECS,
    target_secs: ROLLING_WINDOW_TARGET_SECS,
    // Slightly below target+lookahead so a full 10 s buffer enters the
    // planner instead of being treated as a single short terminal window.
    max_secs: ROLLING_WINDOW_TARGET_SECS + ROLLING_WINDOW_LOOKAHEAD_SECS - 0.01,
    overlap_secs: ROLLING_WINDOW_OVERLAP_SECS,
    boundary_tolerance_secs: ROLLING_WINDOW_LOOKAHEAD_SECS,
};

/// One VAD-planned live decode request plus the commit cursor it owns.
#[derive(Debug, Clone)]
pub(crate) struct RollingWindowCandidate {
    pub(crate) id: u64,
    pub(crate) audio: Vec<f32>,
    pub(crate) start_secs: f32,
    pub(crate) end_secs: f32,
    commit_through_samples: usize,
    overlap_end_secs: f32,
}

/// Monotonic scheduling state for the rolling Refine lane.
///
/// The PCM itself remains in the session-owned buffer. This state accounts for
/// fresh samples since the last accepted request, issues stable window ids, and
/// commits the overlap cut only after the scheduler accepts the request.
#[derive(Debug, Default)]
pub(crate) struct RollingCorrectionWindow {
    next_window_id: u64,
    fresh_samples: usize,
    submitted_once: bool,
    buffer_start_secs: f32,
    covered_through_secs: f32,
    pending_merge: Option<RollingWindowCandidate>,
    merged_context: crate::pipeline::contracts::RawTranscript,
}

impl RollingCorrectionWindow {
    pub(crate) fn observe_samples(&mut self, samples: usize) {
        self.fresh_samples = self.fresh_samples.saturating_add(samples);
    }

    pub(crate) fn candidate(
        &self,
        audio: &[f32],
        sample_rate: u32,
    ) -> Option<RollingWindowCandidate> {
        if sample_rate == 0 {
            return None;
        }
        let total_secs = audio.len() as f32 / sample_rate as f32;
        let (_, stats) = crate::vad::extract_speech(audio, sample_rate);
        let silences = silence_spans_from_vad_probabilities(
            &stats.probabilities,
            crate::vad::VadConfig::default().threshold,
            total_secs,
        );
        self.candidate_with_silences(audio, sample_rate, &silences)
    }

    pub(crate) fn candidate_with_silences(
        &self,
        audio: &[f32],
        sample_rate: u32,
        silences: &[(f32, f32)],
    ) -> Option<RollingWindowCandidate> {
        let rate = sample_rate as usize;
        if rate == 0 {
            return None;
        }
        let minimum = ((ROLLING_WINDOW_TARGET_SECS + ROLLING_WINDOW_LOOKAHEAD_SECS)
            * sample_rate as f32) as usize;
        let step = (ROLLING_WINDOW_STEP_SECS * sample_rate as f32) as usize;
        let fresh_required = if self.submitted_once { step } else { minimum };
        if audio.len() < minimum || self.fresh_samples < fresh_required {
            return None;
        }
        let total_secs = audio.len() as f32 / sample_rate as f32;
        let (start_secs, end_secs) =
            plan_vad_aligned_windows_with_config(silences, total_secs, ROLLING_WINDOW_PLAN)
                .into_iter()
                .next()?;
        let start = ((start_secs * sample_rate as f32).round() as usize).min(audio.len());
        let end = ((end_secs * sample_rate as f32).round() as usize).min(audio.len());
        if end <= start {
            return None;
        }
        let absolute_start = self.buffer_start_secs + start_secs;
        let absolute_end = self.buffer_start_secs + end_secs;
        Some(RollingWindowCandidate {
            id: self.next_window_id.saturating_add(1),
            audio: audio[start..end].to_vec(),
            start_secs: absolute_start,
            end_secs: absolute_end,
            commit_through_samples: end,
            overlap_end_secs: self.covered_through_secs.max(absolute_start),
        })
    }

    pub(crate) fn commit(
        &mut self,
        audio: &mut Vec<f32>,
        sample_rate: u32,
        candidate: &RollingWindowCandidate,
    ) {
        let overlap = (ROLLING_WINDOW_OVERLAP_SECS * sample_rate as f32) as usize;
        let drain = candidate.commit_through_samples.saturating_sub(overlap);
        audio.drain(..drain);
        self.buffer_start_secs += drain as f32 / sample_rate.max(1) as f32;
        self.next_window_id = self.next_window_id.saturating_add(1);
        self.fresh_samples = 0;
        self.submitted_once = true;
        self.pending_merge = Some(candidate.clone());
    }

    /// Feed the existing time-seam judge and return context for the next live
    /// window. Segment clocks are rebased to session time before merging.
    pub(crate) fn merge_context(
        &mut self,
        mut transcript: crate::pipeline::contracts::RawTranscript,
    ) -> Option<String> {
        let candidate = self.pending_merge.take()?;
        transcript.segments.iter_mut().for_each(|segment| {
            segment.start_ts += candidate.start_secs;
            segment.end_ts += candidate.start_secs;
        });
        merge_chunk_transcripts(
            &mut self.merged_context,
            transcript,
            candidate.overlap_end_secs,
        );
        self.covered_through_secs = self.covered_through_secs.max(candidate.end_secs);
        (!self.merged_context.text.trim().is_empty()).then(|| self.merged_context.text.clone())
    }

    /// Close one utterance without reusing its overlap as authority for the
    /// next committed span. Window ids stay monotonic across the session.
    pub(crate) fn seal_utterance(&mut self) {
        self.fresh_samples = 0;
        self.submitted_once = false;
        self.buffer_start_secs = 0.0;
        self.covered_through_secs = 0.0;
        self.pending_merge = None;
        self.merged_context = crate::pipeline::contracts::RawTranscript::default();
    }
}

#[cfg(test)]
mod rolling_window_tests {
    use super::*;

    #[test]
    fn rolling_window_ids_are_monotonic_and_keep_the_overlap() {
        const RATE: u32 = 10;
        let mut state = RollingCorrectionWindow::default();
        let mut audio = vec![1.0; 6 * RATE as usize];
        state.observe_samples(audio.len());

        audio.extend(vec![1.0; 4 * RATE as usize]);
        state.observe_samples(4 * RATE as usize);
        let first = state.candidate(&audio, RATE).expect("first planned window");
        assert_eq!(first.id, 1);
        assert!(first.audio.len() <= 10 * RATE as usize);
        state.commit(&mut audio, RATE, &first);
        assert!(audio.len() >= 4 * RATE as usize);

        audio.extend(vec![2.0; 4 * RATE as usize]);
        state.observe_samples(4 * RATE as usize);
        while audio.len() < 10 * RATE as usize {
            audio.extend(vec![2.0; RATE as usize]);
            state.observe_samples(RATE as usize);
        }
        let second = state.candidate(&audio, RATE).expect("next planned window");
        assert_eq!(second.id, 2);
        assert!(second.audio.len() <= 10 * RATE as usize);
    }

    #[test]
    fn rolling_window_decode_cost_is_bounded_through_150_seconds() {
        const RATE: u32 = 10;
        let mut state = RollingCorrectionWindow::default();
        let mut audio = Vec::new();
        let mut scheduled_at = Vec::new();
        let mut scheduled_samples = Vec::new();

        for second in 1..=150 {
            audio.extend(vec![second as f32; RATE as usize]);
            let cap = ((ROLLING_WINDOW_TARGET_SECS + ROLLING_WINDOW_LOOKAHEAD_SECS) * RATE as f32)
                as usize;
            if audio.len() > cap {
                audio.drain(..audio.len() - cap);
            }
            state.observe_samples(RATE as usize);
            if let Some(candidate) = state.candidate(&audio, RATE) {
                scheduled_at.push(second);
                scheduled_samples.push(candidate.audio.len());
                state.commit(&mut audio, RATE, &candidate);
            }
        }

        assert!(!scheduled_samples.is_empty());
        assert!(
            scheduled_samples
                .iter()
                .all(|samples| *samples <= 8 * RATE as usize),
            "every request must stay bounded by the 8s competence window"
        );
        let first_third = scheduled_at.iter().filter(|at| **at <= 50).count();
        let final_third = scheduled_at.iter().filter(|at| **at > 100).count();
        assert!(
            final_third >= first_third,
            "rolling coverage must not decay: first={first_third}, final={final_third}"
        );
    }

    #[test]
    fn rolling_window_coalesces_until_the_six_second_floor() {
        const RATE: u32 = 10;
        let mut state = RollingCorrectionWindow::default();
        let mut audio = vec![0.0; 5 * RATE as usize];
        state.observe_samples(audio.len());
        assert!(state.candidate(&audio, RATE).is_none());

        audio.extend(vec![0.0; RATE as usize]);
        state.observe_samples(RATE as usize);
        audio.extend(vec![0.0; 4 * RATE as usize]);
        state.observe_samples(4 * RATE as usize);
        assert!(state.candidate(&audio, RATE).is_some());
    }

    /// The live bridge takes its boundary from the shared VAD planner, not a
    /// fixed trailing slice. The chosen end lands inside the supplied silence
    /// around the 8-second target.
    #[test]
    fn rolling_window_boundary_is_vad_planned() {
        const RATE: u32 = 100;
        let mut state = RollingCorrectionWindow::default();
        let audio = vec![0.25; 10 * RATE as usize];
        state.observe_samples(audio.len());
        let silences = [(7.8, 8.2)];

        let candidate = state
            .candidate_with_silences(&audio, RATE, &silences)
            .expect("VAD-planned rolling window");

        assert!((7.8..=8.2).contains(&candidate.end_secs));
        assert_eq!(
            candidate.audio.len(),
            (candidate.end_secs * RATE as f32).round() as usize
        );
    }

    /// The live context carry uses the shared time-seam judge: overlap
    /// segments from the later window are discarded by timestamp while fresh
    /// segments survive into the prompt for the next request.
    #[test]
    fn rolling_window_context_uses_time_seam_merger() {
        use crate::pipeline::contracts::{RawTranscript, TranscriptSegment};

        const RATE: u32 = 100;
        let mut state = RollingCorrectionWindow::default();
        let mut audio = vec![0.25; 10 * RATE as usize];
        state.observe_samples(audio.len());
        let first = state
            .candidate_with_silences(&audio, RATE, &[(7.8, 8.2)])
            .expect("first window");
        state.commit(&mut audio, RATE, &first);
        let first_context = state
            .merge_context(RawTranscript {
                text: "head edge".into(),
                segments: vec![
                    TranscriptSegment {
                        text: "head".into(),
                        start_ts: 0.0,
                        end_ts: 4.0,
                    },
                    TranscriptSegment {
                        text: "edge".into(),
                        start_ts: 7.0,
                        end_ts: 8.0,
                    },
                ],
                ..Default::default()
            })
            .expect("first context");
        assert_eq!(first_context, "head edge");

        audio.extend(vec![0.5; 4 * RATE as usize]);
        state.observe_samples(4 * RATE as usize);
        let second = state
            .candidate_with_silences(&audio, RATE, &[(7.8, 8.2)])
            .expect("second window");
        state.commit(&mut audio, RATE, &second);
        let merged = state
            .merge_context(RawTranscript {
                text: "duplicate fresh".into(),
                segments: vec![
                    TranscriptSegment {
                        text: "duplicate".into(),
                        start_ts: 0.0,
                        end_ts: 3.0,
                    },
                    TranscriptSegment {
                        text: "fresh".into(),
                        start_ts: 4.5,
                        end_ts: 6.0,
                    },
                ],
                ..Default::default()
            })
            .expect("merged context");
        assert_eq!(merged, "head edge fresh");
    }
}

/// Why a partial correction pass fired. Recorded per run so the Stats event can
/// show which condition is actually driving corrections in a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PartialPassTrigger {
    Utterance,
    Speech,
    Timer,
}

/// Which trigger conditions currently hold. More than one can be true at once —
/// [`primary_reason`] resolves that to a single attributed cause.
///
/// [`primary_reason`]: PartialPassTriggerFlags::primary_reason
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PartialPassTriggerFlags {
    pub(crate) utterance_finals: bool,
    pub(crate) silero_speech: bool,
    pub(crate) timer: bool,
}

impl PartialPassTriggerFlags {
    /// The single trigger this run is attributed to, or `None` when nothing
    /// fired.
    ///
    /// Priority is utterance → speech → timer: the timer is the watchdog that
    /// only matters when neither speech signal reached its threshold, so
    /// crediting it while a real one also fired would misreport the cadence.
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

/// Accumulators behind the trigger policy, all measured *since the last
/// successful partial pass* rather than since session start.
#[derive(Debug)]
pub(crate) struct PartialPassTriggerState {
    pub(crate) utterance_finals_since_partial: u32,
    pub(crate) silero_speech_ms_since_partial: u64,
    pub(crate) timer_baseline: Instant,
}

impl PartialPassTriggerState {
    /// Fresh state with the watchdog clock starting at `now`.
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            utterance_finals_since_partial: 0,
            silero_speech_ms_since_partial: 0,
            timer_baseline: now,
        }
    }

    /// Fold one completed inference result into the accumulators.
    ///
    /// Speech time is counted from Silero's VAD samples, not from wall clock or
    /// audio length, so silence between words never advances the speech
    /// trigger.
    pub(crate) fn observe_speech_event(&mut self, is_final: bool, silero_speech_vad_samples: u64) {
        if is_final {
            self.utterance_finals_since_partial =
                self.utterance_finals_since_partial.saturating_add(1);
        }
        self.silero_speech_ms_since_partial = self
            .silero_speech_ms_since_partial
            .saturating_add(silero_vad_samples_to_ms(silero_speech_vad_samples));
    }

    /// Test the accumulators against their thresholds. Pure — firing a pass and
    /// clearing the counters is [`reset_after_success`]'s job.
    ///
    /// [`reset_after_success`]: Self::reset_after_success
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

    /// Rearm all three triggers. Called only when a pass was actually
    /// submitted — a failed submit leaves the accumulators standing so the next
    /// loop iteration retries instead of waiting out a fresh window.
    pub(crate) fn reset_after_success(&mut self, now: Instant) {
        self.utterance_finals_since_partial = 0;
        self.silero_speech_ms_since_partial = 0;
        self.timer_baseline = now;
    }
}

/// Milliseconds of measured speech as seconds, for log fields.
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

/// Whether a returning correction still describes the current transcript.
///
/// Boundary revision is the whole test: it advances only on a committed final,
/// which is the one event that can rewrite the text a correction was computed
/// against. The text arguments are kept in the signature so a future policy can
/// tighten this without touching every call site.
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

/// Merge a corrected re-transcription of the correction window back into the
/// full preview baseline.
///
/// `window_snapshot` is the text mirror of the exact audio slice that was
/// submitted to the Refine lane (taken in lockstep by `schedule_partial_pass`).
/// The correction therefore only has authority over that slice — everything
/// else in `baseline` must survive verbatim. Returns `None` when the snapshot
/// can no longer be anchored inside the baseline (boundary rewrote it, cap
/// trimmed it); the caller must then suppress the correction instead of
/// destroying accumulated text.
pub(crate) fn merge_corrected_window(
    baseline: &str,
    window_snapshot: &str,
    corrected: &str,
) -> Option<String> {
    let baseline_trim = baseline.trim();
    let snapshot = window_snapshot.trim();
    if snapshot.is_empty() {
        // No text mirror for the corrected audio (e.g. every partial in the
        // slice was dropped). Only safe when there is nothing to protect.
        return baseline_trim
            .is_empty()
            .then(|| corrected.trim().to_string());
    }
    if baseline_trim == snapshot {
        return Some(corrected.trim().to_string());
    }
    if let Some(pos) = baseline_trim.rfind(snapshot) {
        // The snapshot sits inside the baseline (typically the tail, or the
        // middle when newer previews appended while Refine ran) — splice the
        // corrected text into its place and keep the rest untouched.
        let prefix = baseline_trim[..pos].trim_end();
        let suffix = baseline_trim[pos + snapshot.len()..].trim_start();
        let corrected = corrected.trim();
        let mut merged = String::with_capacity(prefix.len() + corrected.len() + suffix.len() + 2);
        merged.push_str(prefix);
        if !merged.is_empty() && !corrected.is_empty() {
            merged.push(' ');
        }
        merged.push_str(corrected);
        if !suffix.is_empty() {
            if !merged.is_empty() {
                merged.push(' ');
            }
            merged.push_str(suffix);
        }
        return Some(merged);
    }
    if snapshot.ends_with(baseline_trim) {
        // The corrected audio covers a superset of the current baseline (a
        // final boundary replaced accumulated text mid-window) — the refine
        // pass legitimately supersedes the whole preview.
        return Some(corrected.trim().to_string());
    }
    None
}

/// Apply final-boundary commit text against the live preview assembly.
///
/// Product rule (core engine, 2026-07-24): the live/interim assembly is the
/// **stream floor of truth** for an utterance. Commit-lane re-transcription
/// (especially Apple SFSpeech on a long window) may collapse to a short tail.
/// When that happens we **keep the richer preview** so freezed seals do not
/// throw away speech that already landed in the open tail.
///
/// Returns `true` when a boundary has usable content after reconciliation.
pub(crate) fn apply_final_boundary_text(
    accumulated_text: &mut String,
    cleaned_final: &str,
) -> bool {
    let cleaned = cleaned_final.trim();
    let preview = accumulated_text.trim();
    if cleaned.is_empty() {
        return !preview.is_empty();
    }
    // Empty preview → take commit text.
    if preview.is_empty() {
        *accumulated_text = cleaned.to_string();
        return true;
    }
    // Commit shorter than ~40% of preview = collapse (same rule as file final).
    if crate::pipeline::contracts::final_pass_is_length_regression(cleaned, preview) {
        // Keep preview already stored in accumulated_text.
        return true;
    }
    *accumulated_text = cleaned.to_string();
    true
}

/// Counters for the partial-pass lane, drained into the session's final Stats
/// event.
///
/// The three failure counters are not interchangeable: `stale` means the
/// correction came back too late to apply, `coalesced` means a newer pass
/// superseded one still in flight, and `dropped` means the scheduler never
/// accepted or completed it. All saturate rather than wrap — telemetry must not
/// panic a live session.
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
    /// Count one submitted pass against both the total and its trigger.
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

    /// A correction returned but no longer applied to the current transcript.
    pub(crate) fn record_stale(&mut self) {
        self.stale_count = self.stale_count.saturating_add(1);
    }

    /// An in-flight correction was superseded by a newer one.
    pub(crate) fn record_coalesced(&mut self) {
        self.coalesced_count = self.coalesced_count.saturating_add(1);
    }

    /// A correction never produced a usable result — rejected at submit, or
    /// failed in the scheduler.
    pub(crate) fn record_dropped(&mut self) {
        self.dropped_count = self.dropped_count.saturating_add(1);
    }
}

/// Resolve trigger flags to the one cause a pass is attributed to.
///
/// Free-function seam over [`PartialPassTriggerFlags::primary_reason`] so the
/// session loop reads as policy rather than method dispatch.
pub(crate) fn classify_partial_trigger(
    flags: PartialPassTriggerFlags,
) -> Option<PartialPassTrigger> {
    flags.primary_reason()
}

/// Submit the buffered correction window to the Refine lane.
///
/// Audio and its text mirror are snapshotted **in lockstep**. The PCM request is
/// bounded to the rolling competence window while the text snapshot remains an
/// anchor inside the current uncommitted span; committed spans are excluded by
/// the boundary revision guard and utterance seal.
///
/// An already-in-flight request is superseded rather than queued: only the
/// freshest tail is worth correcting. Returns whether the submission was
/// accepted — `false` leaves the caller's trigger accumulators untouched so the
/// pass is retried.
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
    rolling_window: &mut RollingCorrectionWindow,
    correction_in_flight: &mut Option<SttTaskHandle>,
    correction_expected_window_id: &mut Option<u64>,
    correction_expected_boundary_rev: &mut Option<u64>,
    correction_expected_text: &mut Option<String>,
    correction_suffix_snapshot: &mut Option<String>,
    suffix_snapshot: &str,
    boundary_rev: u64,
    window_text: &str,
    previous_window_prompt: Option<String>,
    speech_ms_since_partial: u64,
    trigger: PartialPassTrigger,
    partial_telemetry: &mut PartialPassTelemetry,
    event_sink: &Arc<dyn EventSink>,
) -> bool {
    let Some(candidate) = rolling_window.candidate(correction_audio_buf, output_sample_rate) else {
        return false;
    };
    let window_id = candidate.id;
    let audio = candidate.audio.clone();
    // Take the text mirror in lockstep with the audio slice: expected_text
    // must describe exactly the audio submitted below, or the receive-side
    // merge (`merge_corrected_window`) has no anchor and the correction would
    // have to be suppressed.
    let baseline_text = window_text.to_string();
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
        window_id,
        baseline_len = baseline_text.chars().count(),
        audio_sec = audio_duration_s,
        silero_speech_sec = silero_speech_seconds(speech_ms_since_partial),
        trigger = ?trigger,
        runs_total = partial_telemetry.runs_total,
        "BOUNDARY correction_scheduled"
    );

    let window_prompt = crate::pipeline::stream_postprocess::compose_whisper_window_prompt(
        previous_window_prompt.as_deref(),
    );
    match stt_scheduler.submit_for_utterance_with_prompt(
        SttLane::Refine,
        audio,
        output_sample_rate,
        pipeline_language,
        window_id,
        window_prompt,
    ) {
        Ok(handle) => {
            rolling_window.commit(correction_audio_buf, output_sample_rate, &candidate);
            partial_telemetry.record_run(trigger);
            *correction_expected_window_id = Some(window_id);
            *correction_expected_boundary_rev = Some(boundary_rev);
            *correction_expected_text = Some(baseline_text);
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

/// Unit tests for [`merge_corrected_window`] splice / suppress semantics.
#[cfg(test)]
mod merge_tests {
    use super::merge_corrected_window;

    /// Tail-only refine: head of baseline must survive verbatim.
    #[test]
    fn corrected_tail_splices_onto_untouched_head() {
        // Hold-mode regression: 52s of speech, correction re-decodes only the
        // trailing window — the head must survive verbatim.
        let merged = merge_corrected_window(
            "ala ma kota i psa oraz chomika",
            "oraz chomika",
            "oraz chomika w klatce",
        );
        assert_eq!(
            merged.as_deref(),
            Some("ala ma kota i psa oraz chomika w klatce")
        );
    }

    /// Snapshot in the middle: newer preview tail appended while Refine ran stays.
    #[test]
    fn corrected_middle_keeps_previews_appended_while_refine_ran() {
        // Previews kept landing after the audio take: the snapshot sits in the
        // middle of the baseline, and the newer tail must survive.
        let merged = merge_corrected_window(
            "stara głowa środek okna nowy ogon",
            "środek okna",
            "środek OKNA poprawiony",
        );
        assert_eq!(
            merged.as_deref(),
            Some("stara głowa środek OKNA poprawiony nowy ogon")
        );
    }

    /// When snapshot equals baseline, the whole preview is replaced by corrected.
    #[test]
    fn full_window_replacement_when_snapshot_equals_baseline() {
        let merged = merge_corrected_window("cały tekst", "cały tekst", "cały tekst lepszy");
        assert_eq!(merged.as_deref(), Some("cały tekst lepszy"));
    }

    /// Snapshot covering a superset of the post-boundary baseline supersedes all.
    #[test]
    fn superset_window_supersedes_boundary_rewritten_baseline() {
        // Final boundary replaced accumulated text with the commit-lane final;
        // the refine window still covers partials + final, so it wins whole.
        let merged = merge_corrected_window(
            "finalny tekst",
            "czesc pierwsza finalny tekst",
            "część pierwsza finalny tekst",
        );
        assert_eq!(merged.as_deref(), Some("część pierwsza finalny tekst"));
    }

    /// Unanchorable snapshot returns `None` so callers suppress instead of destroy.
    #[test]
    fn unanchored_snapshot_suppresses_instead_of_destroying() {
        let merged = merge_corrected_window(
            "zupełnie inny tekst po granicy",
            "stare okno którego już nie ma",
            "poprawka starego okna",
        );
        assert_eq!(merged, None);
    }

    /// Empty snapshot is safe only when baseline is empty; otherwise suppress.
    #[test]
    fn empty_snapshot_only_replaces_empty_baseline() {
        assert_eq!(
            merge_corrected_window("", "", "świeży tekst").as_deref(),
            Some("świeży tekst")
        );
        assert_eq!(
            merge_corrected_window("istniejący tekst", "", "cokolwiek"),
            None
        );
    }

    /// Repeated tokens use `rfind` so the live window, not an earlier echo, anchors.
    #[test]
    fn repeated_phrase_anchors_to_last_occurrence() {
        // rfind: the most recent occurrence is the live window, not an echo
        // earlier in the session.
        let merged = merge_corrected_window("tak tak tak", "tak", "tak jest");
        assert_eq!(merged.as_deref(), Some("tak tak tak jest"));
    }
}
