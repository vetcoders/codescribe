//! Event-based transcription session: VAD-fed utterance ingestion, the
//! pipelined Whisper inference loop, boundary/final emission, and the
//! in-memory batch helpers built on the same runtime path.

use std::collections::VecDeque;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use futures_util::StreamExt;
use futures_util::stream::FuturesOrdered;
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

use crate::audio::chunker::{SpeechEvent, SpeechSession};
use crate::pipeline::contracts::{
    DropKind, EngineEvent, EventSink, TranscriptSegment, collect_confidence_flags,
};
use crate::stt::scheduler::{SttLane, SttScheduler, SttTaskHandle};
use crate::vad;

use super::correction::{
    PARTIAL_PASS_TRIGGER_TIMER_MS, PartialPassTelemetry, PartialPassTriggerState,
    apply_final_boundary_text, classify_partial_trigger, correction_baseline_text,
    correction_is_stale, postprocess_correction_with_snapshot, schedule_partial_pass,
};
use super::pipeline::{PostprocessDrop, TranscriptionPipeline};
use super::quality_gate::{
    MAX_WORDS_PER_SEC, MIN_SPEECH_RATIO_FOR_INFERENCE, emit_vad_warning,
    should_drop_short_utterance, should_drop_silence_chunk, text_words_per_second,
    utterance_vad_speech_pct,
};
use super::stream_log::append_to_stream_log;
use super::tuning::{inference_max_concurrency, interim_vad_accumulate_samples};

/// Maximum audio retained in the Refine correction buffer, in seconds.
///
/// The Refine lane re-transcribes `correction_audio_buf` to correct the recent
/// suffix of an utterance. Without a cap the buffer grows for the whole
/// utterance, so each Refine re-decodes from the very start (O(n) per pass).
/// Bounding it to a trailing window keeps Refine focused on the fresh tail —
/// which is all `strip_overlap` needs — at constant cost. Sized to comfortably
/// exceed the partial-pass cadence so no spoken tail is ever dropped before a
/// Refine consumes it.
const CORRECTION_WINDOW_SEC: f32 = 18.0;

/// Trim `buf` in place so it retains at most `window_sec` of trailing audio at
/// `sample_rate`. Returns the number of leading samples drained.
fn cap_correction_buffer(buf: &mut Vec<f32>, sample_rate: u32, window_sec: f32) -> usize {
    let cap = (window_sec * sample_rate as f32) as usize;
    if cap == 0 || buf.len() <= cap {
        return 0;
    }
    let drain_n = buf.len() - cap;
    buf.drain(..drain_n);
    drain_n
}

// ── Unified session config ───────────────────────────────────────────────────

/// Configuration for a transcription session.
///
/// No presentation parameters — this is pure engine config.
pub struct SessionConfig {
    pub sample_rate: u32,
    pub language: Option<String>,
    pub stream_log_path: Option<std::path::PathBuf>,
    /// VAD silence threshold for utterance boundary (None = use default).
    pub utterance_silence_sec: Option<f32>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct EnqueueOutcome {
    pub(crate) enqueued: bool,
    pub(crate) dropped: u64,
    pub(crate) evicted_final: bool,
}

pub(crate) fn enqueue_pending_utterance(
    pending: &mut VecDeque<PendingUtteranceWorkItem>,
    item: PendingUtteranceWorkItem,
    max_pending: usize,
) -> EnqueueOutcome {
    if max_pending == 0 {
        return EnqueueOutcome {
            enqueued: false,
            dropped: 1,
            evicted_final: false,
        };
    }

    if pending.len() < max_pending {
        pending.push_back(item);
        return EnqueueOutcome {
            enqueued: true,
            dropped: 0,
            evicted_final: false,
        };
    }

    if !item.is_final {
        return EnqueueOutcome {
            enqueued: false,
            dropped: 1,
            evicted_final: false,
        };
    }

    if let Some(pos) = pending.iter().position(|queued| !queued.is_final) {
        pending.remove(pos);
        pending.push_back(item);
        return EnqueueOutcome {
            enqueued: true,
            dropped: 1,
            evicted_final: false,
        };
    }

    let evicted_final = pending.pop_front().is_some();
    pending.push_back(item);
    EnqueueOutcome {
        enqueued: true,
        dropped: u64::from(evicted_final),
        evicted_final,
    }
}

// ── Unified transcription session (event-based) ─────────────────────────────

/// Unified transcription session exposed as a single event-emitting pipeline.
///
/// The engine processes audio → VAD → Whisper → PostProcess and emits
/// `EngineEvent`s. No presentation logic (typing animation, buffer delay,
/// etc.) — that's the consumer's responsibility.
pub(crate) async fn transcription_session(
    mut chunk_receiver: mpsc::Receiver<Vec<f32>>,
    event_sink: Arc<dyn EventSink>,
    config: SessionConfig,
) {
    let SessionConfig {
        sample_rate,
        language,
        stream_log_path,
        utterance_silence_sec,
    } = config;

    info!("Transcription session started (event-based pipeline)");

    let mut session = if let Some(sec) = utterance_silence_sec {
        SpeechSession::new_utterance_with_silence(sample_rate, sec)
    } else {
        SpeechSession::new_utterance(sample_rate)
    };
    let output_sample_rate = session.output_sample_rate();
    let stt_scheduler = SttScheduler::new();

    let mut pipeline = TranscriptionPipeline::new(language);
    let mut preview_rev: u64 = 0;
    let mut utterance_id: u64 = 0;
    let mut scheduler_utterance_id: u64 = 1;
    let mut total_utterances: u64 = 0;
    let semantic_gate_drops: u64 = 0;
    let mut filtered_empty_drops: u64 = 0;
    let mut corrections_applied: u64 = 0;
    let mut partial_telemetry = PartialPassTelemetry::default();
    let mut vad_started = false;
    let mut speech_activity_observed = false;

    // Accumulate text for the current "run" of utterances (between corrections).
    let mut accumulated_text = String::new();
    // Track last raw Whisper output for final flush UtteranceFinal.
    let mut last_raw_text = String::new();
    let mut last_segments: Vec<TranscriptSegment> = Vec::new();
    // Track per-utterance confidence metadata for UtteranceFinal.
    let mut utterance_vad_speech_samples: u64 = 0;
    let mut utterance_avg_logprob: Option<f32> = None;
    let mut utterance_compression_ratio: Option<f32> = None;
    let mut utterance_quality_gate_dropped = false;
    // Accumulate segment timestamps for the current utterance across interim slices.
    let mut utterance_segments: Vec<TranscriptSegment> = Vec::new();

    // Track audio position for UtteranceFinal timestamps (seconds).
    let mut utterance_start_s: f32 = 0.0;
    let mut utterance_audio_samples: usize = 0;

    // Phase 2 correction state
    let mut correction_audio_buf: Vec<f32> = Vec::new();
    let mut partial_trigger_state = PartialPassTriggerState::new(Instant::now());
    let mut suffix_snapshot = String::new();

    // Fix A: Snapshot pipeline.last_suffix at utterance boundary so FINAL
    // compares against previous utterance's tail, not intermediate non-final
    // chunk suffixes that advanced during Phase 1 preview processing.
    let mut utterance_boundary_suffix = String::new();

    // Fix D: Speech-window-scoped text/rev for partial-pass stale guard.
    // Unlike accumulated_text (cleared on UtteranceFinal), these track all text
    // emitted in the current correction window — giving schedule_partial_pass
    // a stable baseline that survives utterance boundaries.
    let mut window_text = String::new();
    let mut window_rev: u64 = 0;

    // Decouple audio ingestion from Whisper inference.
    const MAX_PENDING_UTTERANCES: usize = 64;
    let mut pending_utterances: VecDeque<PendingUtteranceWorkItem> = VecDeque::new();
    let mut dropped_utterances: u64 = 0;
    let mut audio_closed = false;
    // Full utterance audio buffer used for per-utterance commit requests.
    // Live slices still drive preview; final commit re-transcribes the utterance.
    // Scheduler enforces unconditional Commit-lane VAD prefilter before inference.
    let mut current_utterance_audio: Vec<f32> = Vec::new();

    // VAD-first accumulation buffer: collects interim audio chunks and only
    // submits to Whisper after running extract_speech on the accumulated buffer.
    // This eliminates hallucinations by never feeding silence to Whisper.
    let interim_vad_threshold = interim_vad_accumulate_samples(output_sample_rate);
    let mut interim_vad_buf: Vec<f32> = Vec::with_capacity(interim_vad_threshold);
    let mut interim_vad_speech_samples: u64 = 0;
    debug!(
        interim_vad_sec = interim_vad_threshold as f32 / output_sample_rate as f32,
        "VAD-first accumulation configured"
    );

    // Phase 1 (streaming preview/commit) — Pipelined execution using FuturesOrdered.
    // This allows submitting multiple chunks to the Scheduler (up to concurrency limit)
    // to utilize the worker queue and avoid backpressure on the VAD/Audio thread.
    // Results are guaranteed to be returned in submission order.
    let max_inference_concurrency = inference_max_concurrency();
    debug!(
        max_inference_concurrency,
        "Phase 1 inference pipeline configured"
    );
    let mut inference_pipeline = FuturesOrdered::new();

    // Phase 2 (buffered correction) — request tracked for stale guards.
    let mut correction_in_flight: Option<SttTaskHandle> = None;
    let mut correction_expected_preview_rev: Option<u64> = None;
    let mut correction_expected_text: Option<String> = None;
    let mut correction_suffix_snapshot: Option<String> = None;

    loop {
        // ── Fill the Pipe ────────────────────────────────────────────────────
        // Drain pending utterances into the scheduler up to the concurrency limit.
        // This decouples ingestion (Supervisor) from inference (Whisper).
        while inference_pipeline.len() < max_inference_concurrency {
            let Some(item) = pending_utterances.pop_front() else {
                break;
            };
            let PendingUtteranceWorkItem {
                audio,
                inference_audio,
                is_final,
                scheduler_utterance_id: work_utterance_id,
                max_speech_prob,
                speech_vad_samples,
            } = item;

            if should_drop_short_utterance(audio.len(), output_sample_rate, max_speech_prob) {
                pipeline.hallucination_drops = pipeline.hallucination_drops.saturating_add(1);
                event_sink.on_event(&EngineEvent::Drop {
                    kind: DropKind::Hallucination,
                    text: String::new(),
                    reason: format!(
                        "Short utterance dropped: {:.3}s with low VAD prob {:.2}",
                        audio.len() as f32 / output_sample_rate as f32,
                        max_speech_prob
                    ),
                });
                continue;
            }

            // Categorical speech-ratio gate (Silero as binary SoTA classifier).
            // Interim chunks with insufficient speech are pure silence — skip
            // Whisper inference entirely to prevent hallucinations.
            if should_drop_silence_chunk(
                audio.len(),
                output_sample_rate,
                speech_vad_samples,
                is_final,
            ) {
                let audio_16k = (audio.len() as f64 * f64::from(vad::VAD_SAMPLE_RATE)
                    / f64::from(output_sample_rate)) as u64;
                let ratio = if audio_16k > 0 {
                    speech_vad_samples as f32 / audio_16k as f32
                } else {
                    0.0
                };
                debug!(
                    "Silence gate: dropping {:.3}s chunk (speech_ratio={:.1}%, vad_samples={}, threshold={:.0}%)",
                    audio.len() as f32 / output_sample_rate as f32,
                    ratio * 100.0,
                    speech_vad_samples,
                    MIN_SPEECH_RATIO_FOR_INFERENCE * 100.0,
                );
                pipeline.hallucination_drops = pipeline.hallucination_drops.saturating_add(1);
                event_sink.on_event(&EngineEvent::Drop {
                    kind: DropKind::Hallucination,
                    text: String::new(),
                    reason: format!(
                        "Silence chunk dropped: speech_ratio={:.1}% < {:.0}% in {:.3}s",
                        ratio * 100.0,
                        MIN_SPEECH_RATIO_FOR_INFERENCE * 100.0,
                        audio.len() as f32 / output_sample_rate as f32,
                    ),
                });
                continue;
            }

            let lang = pipeline.language.clone();
            let lane = if is_final {
                SttLane::Commit
            } else {
                SttLane::Live
            };
            let item = UtteranceWorkItem {
                audio,
                inference_audio_len: inference_audio.len(),
                is_final,
                speech_vad_samples,
            };

            match stt_scheduler.submit_for_utterance(
                lane,
                inference_audio,
                output_sample_rate,
                lang,
                work_utterance_id,
            ) {
                Ok(mut handle) => {
                    // Wrap the handle and item into a future for FuturesOrdered.
                    // This preserves the item context (is_final, audio len) for the result.
                    inference_pipeline.push_back(async move {
                        let res = handle.recv().await;
                        (res, item)
                    });
                }
                Err(e) => {
                    error!("Failed to submit STT request to scheduler: {}", e);
                    event_sink.on_event(&EngineEvent::Warning {
                        code: "scheduler_submit_error".to_string(),
                        message: format!("{}", e),
                    });
                    // If submission fails, we break the fill loop.
                    // The item is lost (popped), but if the scheduler is broken, we have bigger problems.
                    break;
                }
            }
        }

        if correction_in_flight.is_none() && !correction_audio_buf.is_empty() {
            let now = Instant::now();
            let trigger_flags = partial_trigger_state.evaluate(now);
            if let Some(trigger) = classify_partial_trigger(trigger_flags)
                && schedule_partial_pass(
                    &stt_scheduler,
                    output_sample_rate,
                    pipeline.language.clone(),
                    &mut correction_audio_buf,
                    &mut correction_in_flight,
                    &mut correction_expected_preview_rev,
                    &mut correction_expected_text,
                    &mut correction_suffix_snapshot,
                    &suffix_snapshot,
                    window_rev,
                    &window_text,
                    partial_trigger_state.silero_speech_ms_since_partial,
                    trigger,
                    &mut partial_telemetry,
                    &event_sink,
                )
            {
                partial_trigger_state.reset_after_success(now);
            }
        }

        // If audio is closed and there is no work left, finish.
        if audio_closed
            && pending_utterances.is_empty()
            && inference_pipeline.is_empty()
            && correction_in_flight.is_none()
        {
            break;
        }

        tokio::select! {
            maybe_data = chunk_receiver.recv(), if !audio_closed => {
                match maybe_data {
                    Some(data) => {
                        for event in session.feed(&data, sample_rate) {
                            let speech_vad_samples = session.take_event_speech_vad_samples();
                            let max_speech_prob = session.segment_speech_prob();
                            match event {
                                SpeechEvent::Utterance(u) => {
                                    current_utterance_audio.extend_from_slice(&u);
                                    interim_vad_buf.extend_from_slice(&u);
                                    interim_vad_speech_samples += speech_vad_samples;
                                    speech_activity_observed = true;

                                    if !vad_started {
                                        event_sink.on_event(&EngineEvent::VadStart {
                                            speech_prob: session.boundary_prob(),
                                            ts_ms: session.session_elapsed_ms(),
                                        });
                                        vad_started = true;
                                    }

                                    // Accumulate until threshold, then extract_speech + submit.
                                    if interim_vad_buf.len() >= interim_vad_threshold {
                                        let buf = std::mem::take(&mut interim_vad_buf);
                                        let buf_vad = interim_vad_speech_samples;
                                        interim_vad_speech_samples = 0;
                                        let buf_len = buf.len();
                                        let (speech, stats) = vad::extract_speech(&buf, output_sample_rate);
                                        if speech.is_empty() {
                                            debug!(
                                                "VAD-first: dropping {:.1}s accumulated buffer (0% speech, {} windows)",
                                                buf_len as f32 / output_sample_rate as f32,
                                                stats.total_windows,
                                            );
                                            pipeline.hallucination_drops = pipeline.hallucination_drops.saturating_add(1);
                                            event_sink.on_event(&EngineEvent::Drop {
                                                kind: DropKind::Hallucination,
                                                text: String::new(),
                                                reason: format!(
                                                    "VAD-first: no speech in {:.1}s buffer ({} windows analysed)",
                                                    buf_len as f32 / output_sample_rate as f32,
                                                    stats.total_windows,
                                                ),
                                            });
                                            continue;
                                        }
                                        debug!(
                                            "VAD-first: {:.1}s speech / {:.1}s buffer ({:.0}% speech, {}/{} windows)",
                                            speech.len() as f32 / output_sample_rate as f32,
                                            buf_len as f32 / output_sample_rate as f32,
                                            stats.speech_pct,
                                            stats.speech_windows,
                                            stats.total_windows,
                                        );
                                        let outcome = enqueue_pending_utterance(
                                            &mut pending_utterances,
                                            PendingUtteranceWorkItem {
                                                audio: buf,
                                                inference_audio: speech,
                                                is_final: false,
                                                scheduler_utterance_id,
                                                max_speech_prob,
                                                speech_vad_samples: buf_vad,
                                            },
                                            MAX_PENDING_UTTERANCES,
                                        );
                                        if outcome.dropped > 0 {
                                            dropped_utterances = dropped_utterances.saturating_add(outcome.dropped);
                                            warn!(
                                                queue_len = pending_utterances.len(),
                                                enqueued = outcome.enqueued,
                                                dropped = outcome.dropped,
                                                "Pending utterance backpressure (interim VAD-first)"
                                            );
                                        }
                                    }
                                }
                                SpeechEvent::UtteranceFinal(u) => {
                                    current_utterance_audio.extend_from_slice(&u);
                                    // Flush any accumulated interim audio + this final chunk
                                    // into a single Commit-lane request (extract_speech in prefilter).
                                    let full = std::mem::take(&mut current_utterance_audio);
                                    interim_vad_buf.clear();
                                    interim_vad_speech_samples = 0;
                                    speech_activity_observed = true;

                                    if !vad_started {
                                        event_sink.on_event(&EngineEvent::VadStart {
                                            speech_prob: session.boundary_prob(),
                                            ts_ms: session.session_elapsed_ms(),
                                        });
                                        vad_started = true;
                                    }

                                    let outcome = enqueue_pending_utterance(
                                        &mut pending_utterances,
                                        PendingUtteranceWorkItem {
                                            audio: u,
                                            inference_audio: full,
                                            is_final: true,
                                            scheduler_utterance_id,
                                            max_speech_prob,
                                            speech_vad_samples,
                                        },
                                        MAX_PENDING_UTTERANCES,
                                    );
                                    scheduler_utterance_id =
                                        scheduler_utterance_id.saturating_add(1);
                                    if outcome.dropped > 0 {
                                        dropped_utterances = dropped_utterances.saturating_add(outcome.dropped);
                                        let message = if outcome.enqueued {
                                            if outcome.evicted_final {
                                                format!(
                                                    "Pending utterance queue full (limit={}): evicted an older final item to preserve latest final boundary",
                                                    MAX_PENDING_UTTERANCES
                                                )
                                            } else {
                                                format!(
                                                    "Pending utterance queue full (limit={}): evicted a non-final item to preserve latest final boundary",
                                                    MAX_PENDING_UTTERANCES
                                                )
                                            }
                                        } else {
                                            format!(
                                                "Pending utterance queue full (limit={}): dropped incoming non-final item",
                                                MAX_PENDING_UTTERANCES
                                            )
                                        };
                                        warn!(
                                            queue_len = pending_utterances.len(),
                                            is_final = true,
                                            enqueued = outcome.enqueued,
                                            evicted_final = outcome.evicted_final,
                                            dropped = outcome.dropped,
                                            "{}",
                                            message
                                        );
                                        event_sink.on_event(&EngineEvent::Warning {
                                            code: "pending_utterance_backpressure".to_string(),
                                            message,
                                        });
                                    }
                                }
                                _ => continue,
                            };
                        }
                        emit_vad_warning(&event_sink, &mut session);
                    }
                    None => {
                        audio_closed = true;
                        // Flush any remaining interim VAD buffer into final audio.
                        interim_vad_buf.clear();
                        interim_vad_speech_samples = 0;

                        if let Some(event) = session.flush() {
                            let speech_vad_samples = session.take_event_speech_vad_samples();
                            let max_speech_prob = session.segment_speech_prob();
                            // On flush, always treat as final (Commit lane with extract_speech).
                            let (utterance, inference_audio) = match event {
                                SpeechEvent::Utterance(u) | SpeechEvent::UtteranceFinal(u) => {
                                    current_utterance_audio.extend_from_slice(&u);
                                    let full = std::mem::take(&mut current_utterance_audio);
                                    (u, full)
                                }
                                _ => (Vec::new(), Vec::new()),
                            };

                            if !utterance.is_empty() {
                                speech_activity_observed = true;
                                if !vad_started {
                                    event_sink.on_event(&EngineEvent::VadStart {
                                        speech_prob: session.boundary_prob(),
                                        ts_ms: session.session_elapsed_ms(),
                                    });
                                    vad_started = true;
                                }
                                let outcome = enqueue_pending_utterance(
                                    &mut pending_utterances,
                                    PendingUtteranceWorkItem {
                                        audio: utterance,
                                        inference_audio,
                                        is_final: true,
                                        scheduler_utterance_id,
                                        max_speech_prob,
                                        speech_vad_samples,
                                    },
                                    MAX_PENDING_UTTERANCES,
                                );
                                scheduler_utterance_id =
                                    scheduler_utterance_id.saturating_add(1);
                                if outcome.dropped > 0 {
                                    dropped_utterances = dropped_utterances.saturating_add(outcome.dropped);
                                    let message = if outcome.enqueued {
                                        if outcome.evicted_final {
                                            format!(
                                                "Pending utterance queue full (limit={}): evicted an older final item to preserve flush-final boundary",
                                                MAX_PENDING_UTTERANCES
                                            )
                                        } else {
                                            format!(
                                                "Pending utterance queue full (limit={}): evicted a non-final item to preserve flush-final boundary",
                                                MAX_PENDING_UTTERANCES
                                            )
                                        }
                                    } else {
                                        format!(
                                            "Pending utterance queue full (limit={}): dropped flush-final boundary",
                                            MAX_PENDING_UTTERANCES
                                        )
                                    };
                                    warn!(
                                        queue_len = pending_utterances.len(),
                                        is_final = true,
                                        enqueued = outcome.enqueued,
                                        evicted_final = outcome.evicted_final,
                                        dropped = outcome.dropped,
                                        "{}",
                                        message
                                    );
                                    event_sink.on_event(&EngineEvent::Warning {
                                        code: "pending_utterance_backpressure".to_string(),
                                        message,
                                    });
                                }
                            }
                        }
                        emit_vad_warning(&event_sink, &mut session);
                    }
                }
            }
            _ = tokio::time::sleep_until(
                partial_trigger_state.timer_baseline
                    + Duration::from_millis(PARTIAL_PASS_TRIGGER_TIMER_MS)
            ), if correction_in_flight.is_none() && !correction_audio_buf.is_empty() => {
                let now = Instant::now();
                let trigger_flags = partial_trigger_state.evaluate(now);
                if let Some(trigger) = classify_partial_trigger(trigger_flags)
                    && schedule_partial_pass(
                        &stt_scheduler,
                        output_sample_rate,
                        pipeline.language.clone(),
                        &mut correction_audio_buf,
                        &mut correction_in_flight,
                        &mut correction_expected_preview_rev,
                        &mut correction_expected_text,
                        &mut correction_suffix_snapshot,
                        &suffix_snapshot,
                        window_rev,
                        &window_text,
                        partial_trigger_state.silero_speech_ms_since_partial,
                        trigger,
                        &mut partial_telemetry,
                        &event_sink,
                    )
                {
                    partial_trigger_state.reset_after_success(now);
                }
            }
            result = async {
                correction_in_flight.as_mut().unwrap().recv().await
            }, if correction_in_flight.is_some() => {
                // Fix D: Use window_rev as fallback (schedule_partial_pass now stores window_rev).
                let expected_preview_rev = correction_expected_preview_rev.take().unwrap_or(window_rev);
                let expected_text = correction_expected_text.take().unwrap_or_default();
                let suffix_snapshot = correction_suffix_snapshot.take().unwrap_or_default();
                match result {
                    Ok(raw) => {
                        // Fix D: Compare against window-scoped state (survives utterance boundaries).
                        if correction_is_stale(
                            expected_preview_rev,
                            window_rev,
                            &expected_text,
                            &window_text,
                        ) {
                            partial_telemetry.record_stale();
                            debug!(
                                expected_preview_rev,
                                window_rev,
                                expected_len = expected_text.chars().count(),
                                current_len = window_text.chars().count(),
                                "Suppressing stale correction (window advanced or text changed)"
                            );
                        } else {
                            match postprocess_correction_with_snapshot(
                                &mut pipeline,
                                &raw.text,
                                &suffix_snapshot,
                            ) {
                                Ok(cleaned) => {
                                    let (previous_text, correction_after_boundary) =
                                        correction_baseline_text(
                                            &accumulated_text,
                                            &expected_text,
                                            &window_text,
                                        );
                                    if cleaned != previous_text {
                                        preview_rev += 1;
                                        corrections_applied += 1;
                                        debug!(
                                            rev = preview_rev,
                                            previous_len = previous_text.chars().count(),
                                            corrected_len = cleaned.chars().count(),
                                            "BOUNDARY correction"
                                        );
                                        event_sink.on_event(&EngineEvent::Correction {
                                            rev: preview_rev,
                                            text: cleaned.clone(),
                                            previous_text,
                                        });
                                        if correction_after_boundary {
                                            debug!(
                                                "Applied correction after boundary without reopening utterance-local preview state"
                                            );
                                        } else {
                                            // Update accumulated text so next Preview builds from corrected state.
                                            accumulated_text = cleaned;
                                        }
                                    } else {
                                        debug!("Skipping correction emit: no text delta after postprocess");
                                    }
                                }
                                Err(PostprocessDrop::Hallucination) => {
                                    // Already counted in postprocess_with_reason.
                                    debug!("Correction dropped as hallucination");
                                }
                                Err(PostprocessDrop::OverlapEmpty) => {
                                    // Already counted in postprocess_with_reason.
                                    debug!("Correction dropped as overlap-empty");
                                }
                                Err(PostprocessDrop::FilteredEmpty) => {
                                    filtered_empty_drops += 1;
                                    debug!("Correction dropped as filtered-empty");
                                }
                            }
                        }
                    }
                    Err(e) => {
                        partial_telemetry.record_dropped();
                        if e.to_string().contains("superseded") {
                            debug!("Skipping superseded correction request: {}", e);
                        } else if e.to_string().contains("shutting down") {
                            debug!("Ignoring correction during scheduler shutdown: {}", e);
                        } else {
                            warn!("Re-transcription failed; keeping Phase 1 draft: {}", e);
                        }
                    }
                }
                correction_in_flight = None;
            }
            // Drain the pipeline. FuturesOrdered guarantees results arrive in the order submitted.
            // This is critical for timestamp calculation and text accumulation.
            Some((result, item)) = inference_pipeline.next() => {
                // Track audio duration for timestamp computation.
                let chunk_start_samples = utterance_audio_samples;
                utterance_audio_samples += item.audio.len();
                let chunk_start_ts =
                    utterance_start_s + chunk_start_samples as f32 / output_sample_rate as f32;
                if correction_audio_buf.is_empty() {
                    suffix_snapshot = pipeline.last_suffix.clone();
                }
                correction_audio_buf.extend_from_slice(&item.audio);
                // Bound the Refine buffer to a trailing window so corrections
                // re-decode the fresh suffix, not the whole utterance (P2.17).
                cap_correction_buffer(
                    &mut correction_audio_buf,
                    output_sample_rate,
                    CORRECTION_WINDOW_SEC,
                );
                partial_trigger_state.observe_speech_event(item.is_final, item.speech_vad_samples);
                utterance_vad_speech_samples = utterance_vad_speech_samples
                    .saturating_add(item.speech_vad_samples);

                match result {
                    Ok(raw_transcript) => {
                        if item.is_final {
                            utterance_avg_logprob = raw_transcript.avg_logprob;
                            utterance_compression_ratio = raw_transcript.compression_ratio;
                            utterance_quality_gate_dropped = raw_transcript.quality_gate_dropped;
                        } else if utterance_avg_logprob.is_none() {
                            utterance_avg_logprob = raw_transcript.avg_logprob;
                            utterance_compression_ratio = raw_transcript.compression_ratio;
                            if raw_transcript.quality_gate_dropped {
                                utterance_quality_gate_dropped = true;
                            }
                        }
                        let raw_text = raw_transcript.text;
                        let mut raw_segments = raw_transcript.segments;
                        let segment_offset_ts = if item.is_final {
                            // Commit lane for final boundary is always VAD-prefiltered by scheduler.
                            // Segment timestamps are still utterance-relative.
                            utterance_start_s
                        } else {
                            chunk_start_ts
                        };
                        if !raw_segments.is_empty() {
                            for segment in &mut raw_segments {
                                segment.start_ts += segment_offset_ts;
                                segment.end_ts += segment_offset_ts;
                            }
                        }
                        last_raw_text = raw_text.clone();
                        last_segments = raw_segments.clone();
                        if item.is_final {
                            if !raw_segments.is_empty() {
                                utterance_segments = raw_segments.clone();
                            }
                        } else {
                            utterance_segments.extend(raw_segments.clone());
                        }

                        // Fix A: Restore suffix to utterance-boundary snapshot before
                        // FINAL processing so strip_overlap sees the correct tail.
                        if item.is_final {
                            pipeline.last_suffix = utterance_boundary_suffix.clone();
                        }

                        if let Some(words_per_sec) =
                            text_words_per_second(&raw_text, item.inference_audio_len, output_sample_rate)
                                .filter(|wps| *wps > MAX_WORDS_PER_SEC)
                        {
                            pipeline.hallucination_drops =
                                pipeline.hallucination_drops.saturating_add(1);
                            event_sink.on_event(&EngineEvent::Drop {
                                kind: DropKind::Hallucination,
                                text: raw_text.clone(),
                                reason: format!(
                                    "Word-rate anomaly: {:.1} words/s exceeds {:.1} words/s limit",
                                    words_per_sec, MAX_WORDS_PER_SEC
                                ),
                            });
                        } else {
                            match pipeline.postprocess_with_reason_and_segments(
                                &raw_text,
                                &raw_segments,
                            ) {
                                Ok(cleaned) => {
                                    if item.is_final {
                                        let cleaned_final = cleaned.trim();
                                        if apply_final_boundary_text(&mut accumulated_text, cleaned_final) {
                                            if !cleaned_final.is_empty() {
                                                // Fix D: Append FINAL text to window-scoped state
                                                // (not replace — window spans multiple utterances).
                                                if !window_text.is_empty() {
                                                    window_text.push(' ');
                                                }
                                                window_text.push_str(cleaned_final);
                                                window_rev += 1;
                                            } else {
                                                // Keep the latest preview when FINAL postprocess is empty.
                                                // Otherwise silence boundary may never emit UtteranceFinal,
                                                // which breaks auto-send on pause in toggle mode.
                                                debug!(
                                                    preview_len = accumulated_text.chars().count(),
                                                    "Final cleaned text empty; preserving latest preview for boundary commit"
                                                );
                                            }
                                        }
                                    } else {
                                        preview_rev += 1;
                                        if !accumulated_text.is_empty() {
                                            accumulated_text.push(' ');
                                        }
                                        accumulated_text.push_str(cleaned.trim());

                                        // Fix D: Mirror into window-scoped state for partial-pass stale guard.
                                        if !window_text.is_empty() {
                                            window_text.push(' ');
                                        }
                                        window_text.push_str(cleaned.trim());
                                        window_rev += 1;

                                        debug!(
                                            rev = preview_rev,
                                            text_len = accumulated_text.chars().count(),
                                            "BOUNDARY preview"
                                        );
                                        event_sink.on_event(&EngineEvent::Preview {
                                            rev: preview_rev,
                                            text: accumulated_text.clone(),
                                        });

                                        if let Some(path) = stream_log_path.as_deref() {
                                            let _ = append_to_stream_log(path, cleaned.trim());
                                        }
                                    }
                                }
                                Err(PostprocessDrop::Hallucination) => {
                                    event_sink.on_event(&EngineEvent::Drop {
                                        kind: DropKind::Hallucination,
                                        text: raw_text.clone(),
                                        reason: format!(
                                            "Hallucination pattern: '{}'",
                                            raw_text.trim()
                                        ),
                                    });
                                }
                                Err(PostprocessDrop::OverlapEmpty) => {
                                    event_sink.on_event(&EngineEvent::Drop {
                                        kind: DropKind::OverlapEmpty,
                                        text: raw_text.clone(),
                                        reason: "Overlap dedup produced empty result".to_string(),
                                    });
                                }
                                Err(PostprocessDrop::FilteredEmpty) => {
                                    filtered_empty_drops += 1;
                                    event_sink.on_event(&EngineEvent::Drop {
                                        kind: DropKind::FilteredEmpty,
                                        text: raw_text.clone(),
                                        reason: "Empty after lexicon/cleanup (not semantic gate)".to_string(),
                                    });
                                }
                            }
                        }

                        if item.is_final {
                            utterance_id += 1;
                            total_utterances += 1;
                            let final_text = accumulated_text.trim().to_string();
                            let end_ts = utterance_start_s
                                + utterance_audio_samples as f32 / output_sample_rate as f32;
                            let had_content = !final_text.is_empty();
                            if had_content {
                                debug!(
                                    utterance_id,
                                    text_len = final_text.chars().count(),
                                    start_ts = utterance_start_s,
                                    end_ts,
                                    "BOUNDARY final"
                                );
                                let avg_logprob = utterance_avg_logprob.take();
                                let compression_ratio = utterance_compression_ratio.take();
                                let quality_gate_dropped =
                                    std::mem::take(&mut utterance_quality_gate_dropped);
                                let vad_speech_pct = utterance_vad_speech_pct(
                                    utterance_audio_samples,
                                    output_sample_rate,
                                    utterance_vad_speech_samples,
                                );
                                let confidence_flags = collect_confidence_flags(
                                    vad_speech_pct,
                                    avg_logprob,
                                    quality_gate_dropped,
                                );
                                event_sink.on_event(&EngineEvent::UtteranceFinal {
                                    utterance_id,
                                    text: final_text,
                                    raw_text: raw_text.clone(),
                                    start_ts: utterance_start_s,
                                    end_ts,
                                    segments: std::mem::take(&mut utterance_segments),
                                    vad_speech_pct,
                                    avg_logprob,
                                    compression_ratio,
                                    quality_gate_dropped,
                                    confidence_flags,
                                });
                            } else {
                                utterance_segments.clear();
                            }
                            accumulated_text.clear();
                            utterance_vad_speech_samples = 0;
                            utterance_avg_logprob = None;
                            utterance_compression_ratio = None;
                            utterance_quality_gate_dropped = false;
                            // Fix A: Save current suffix as utterance-boundary snapshot
                            // for the next FINAL to restore from.
                            utterance_boundary_suffix = pipeline.last_suffix.clone();
                            // Advance start_ts for next utterance.
                            utterance_start_s = end_ts;
                            utterance_audio_samples = 0;

                            // Only emit VadEnd if UtteranceFinal was emitted — avoids
                            // spurious VadEnd without preceding UtteranceFinal.
                            if vad_started && had_content {
                                event_sink.on_event(&EngineEvent::VadEnd {
                                    speech_prob: session.boundary_prob(),
                                    ts_ms: session.session_elapsed_ms(),
                                });
                                vad_started = false;
                            }
                        }
                        let now = Instant::now();
                        let trigger_flags = partial_trigger_state.evaluate(now);
                        if correction_in_flight.is_none()
                            && let Some(trigger) = classify_partial_trigger(trigger_flags)
                            && schedule_partial_pass(
                                &stt_scheduler,
                                output_sample_rate,
                                pipeline.language.clone(),
                                &mut correction_audio_buf,
                                &mut correction_in_flight,
                                &mut correction_expected_preview_rev,
                                &mut correction_expected_text,
                                &mut correction_suffix_snapshot,
                                &suffix_snapshot,
                                window_rev,
                                &window_text,
                                partial_trigger_state.silero_speech_ms_since_partial,
                                trigger,
                                &mut partial_telemetry,
                                &event_sink,
                            )
                        {
                            partial_trigger_state.reset_after_success(now);
                        }
                    }
                    Err(e) => {
                        error!("Transcription failed: {}", e);
                        event_sink.on_event(&EngineEvent::Warning {
                            code: "transcription_error".to_string(),
                            message: format!("{}", e),
                        });
                    }
                }
            }
            else => {
                if audio_closed
                    && !pending_utterances.is_empty()
                    && inference_pipeline.is_empty()
                    && correction_in_flight.is_none()
                {
                    let abandoned = pending_utterances.len() as u64;
                    dropped_utterances = dropped_utterances.saturating_add(abandoned);
                    pending_utterances.clear();
                    warn!(
                        abandoned,
                        "Dropping pending utterances after audio closed because inference pipeline is idle"
                    );
                }
            }
        }
    }

    if let Err(e) = stt_scheduler.shutdown().await {
        error!("Failed to shutdown STT scheduler: {}", e);
        event_sink.on_event(&EngineEvent::Warning {
            code: "scheduler_shutdown_error".to_string(),
            message: format!("{}", e),
        });
    }

    // Emit any remaining accumulated text as final utterance.
    let remaining = accumulated_text.trim().to_string();
    if !remaining.is_empty() {
        utterance_id += 1;
        total_utterances += 1;
        let end_ts = utterance_start_s + utterance_audio_samples as f32 / output_sample_rate as f32;
        let segments = if utterance_segments.is_empty() {
            last_segments
        } else {
            utterance_segments
        };
        debug!(
            utterance_id,
            text_len = remaining.chars().count(),
            start_ts = utterance_start_s,
            end_ts,
            "BOUNDARY final_flush"
        );
        let vad_speech_pct = utterance_vad_speech_pct(
            utterance_audio_samples,
            output_sample_rate,
            utterance_vad_speech_samples,
        );
        let confidence_flags = collect_confidence_flags(
            vad_speech_pct,
            utterance_avg_logprob,
            utterance_quality_gate_dropped,
        );
        event_sink.on_event(&EngineEvent::UtteranceFinal {
            utterance_id,
            text: remaining,
            raw_text: last_raw_text,
            start_ts: utterance_start_s,
            end_ts,
            segments,
            vad_speech_pct,
            avg_logprob: utterance_avg_logprob,
            compression_ratio: utterance_compression_ratio,
            quality_gate_dropped: utterance_quality_gate_dropped,
            confidence_flags,
        });
    }

    if total_utterances == 0 {
        if vad_started {
            event_sink.on_event(&EngineEvent::VadEnd {
                speech_prob: session.boundary_prob(),
                ts_ms: session.session_elapsed_ms(),
            });
        }
        let reason = if speech_activity_observed
            || pipeline.hallucination_drops > 0
            || filtered_empty_drops > 0
            || dropped_utterances > 0
        {
            "all_speech_rejected_by_quality_gate"
        } else {
            "vad_no_speech_detected"
        };
        event_sink.on_event(&EngineEvent::NoSpeech {
            reason: reason.to_string(),
        });
    }

    // Emit session stats.
    event_sink.on_event(&EngineEvent::Stats {
        dropped_audio_chunks: dropped_utterances,
        hallucination_drops: pipeline.hallucination_drops,
        semantic_gate_drops,
        filtered_empty_drops,
        corrections_applied,
        total_utterances,
        partial_runs_total: partial_telemetry.runs_total,
        trigger_utterance_count: partial_telemetry.trigger_utterance_count,
        trigger_speech_count: partial_telemetry.trigger_speech_count,
        trigger_timer_count: partial_telemetry.trigger_timer_count,
        partial_stale_count: partial_telemetry.stale_count,
        partial_coalesced_count: partial_telemetry.coalesced_count,
        partial_dropped_count: partial_telemetry.dropped_count,
    });

    if dropped_utterances > 0 {
        warn!(
            "Session dropped {} utterance(s) due to backpressure or scheduler stalls",
            dropped_utterances
        );
    }

    info!(
        "Transcription session finished: {} utterances, {} hallucination drops, {} semantic gate drops, {} filtered empty drops, partial_runs={} (utterance={}, speech={}, watchdog={}, stale={}, coalesced={}, dropped={})",
        total_utterances,
        pipeline.hallucination_drops,
        semantic_gate_drops,
        filtered_empty_drops,
        partial_telemetry.runs_total,
        partial_telemetry.trigger_utterance_count,
        partial_telemetry.trigger_speech_count,
        partial_telemetry.trigger_timer_count,
        partial_telemetry.stale_count,
        partial_telemetry.coalesced_count,
        partial_telemetry.dropped_count
    );
}

#[derive(Debug)]
pub(crate) struct PendingUtteranceWorkItem {
    pub(crate) audio: Vec<f32>,
    pub(crate) inference_audio: Vec<f32>,
    pub(crate) is_final: bool,
    pub(crate) scheduler_utterance_id: u64,
    pub(crate) max_speech_prob: f32,
    pub(crate) speech_vad_samples: u64,
}

#[derive(Debug)]
struct UtteranceWorkItem {
    audio: Vec<f32>,
    inference_audio_len: usize,
    is_final: bool,
    speech_vad_samples: u64,
}

struct SessionTranscriptCollector {
    transcript: std::sync::Mutex<String>,
}

impl SessionTranscriptCollector {
    fn new() -> Self {
        Self {
            transcript: std::sync::Mutex::new(String::new()),
        }
    }

    fn append_utterance(&self, text: &str) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        let mut transcript = self.transcript.lock().unwrap_or_else(|e| e.into_inner());
        if !transcript.is_empty() {
            transcript.push(' ');
        }
        transcript.push_str(trimmed);
    }

    fn transcript(&self) -> String {
        self.transcript
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl EventSink for SessionTranscriptCollector {
    fn on_event(&self, event: &EngineEvent) {
        if let EngineEvent::UtteranceFinal { text, .. } = event {
            self.append_utterance(text);
        }
    }
}

struct SessionEventCollector {
    events: std::sync::Mutex<Vec<EngineEvent>>,
}

impl SessionEventCollector {
    fn new() -> Self {
        Self {
            events: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn events(&self) -> Vec<EngineEvent> {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl EventSink for SessionEventCollector {
    fn on_event(&self, event: &EngineEvent) {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(event.clone());
    }
}

/// Public helper: run the event session pipeline on in-memory samples.
///
/// Uses the same runtime path as live recording (`transcription_session`) and
/// collects utterance finals into a session transcript for test/CLI comparisons.
pub async fn transcribe_buffered_samples(
    samples: &[f32],
    sample_rate: u32,
    language: Option<String>,
) -> Result<String> {
    if samples.is_empty() {
        return Ok(String::new());
    }

    // Simulate live callback cadence (~100ms) to keep VAD/utterance behavior realistic.
    let chunk_size = ((sample_rate as f32) * 0.1).round().max(1.0) as usize;

    let (tx, rx) = mpsc::channel::<Vec<f32>>(8);
    let collector = Arc::new(SessionTranscriptCollector::new());
    let event_sink: Arc<dyn EventSink> = collector.clone();
    let session = tokio::spawn(transcription_session(
        rx,
        event_sink,
        SessionConfig {
            sample_rate,
            language,
            stream_log_path: None,
            utterance_silence_sec: None,
        },
    ));

    for chunk in samples.chunks(chunk_size) {
        if tx.send(chunk.to_vec()).await.is_err() {
            return Err(anyhow!("Transcription session dropped channel"));
        }
    }
    drop(tx);

    session
        .await
        .map_err(|e| anyhow!("Transcription session join error: {}", e))?;

    Ok(collector.transcript())
}

/// Public helper: run the event session pipeline and return the emitted engine events.
///
/// This is the closest non-interactive test hook to the real live flow:
/// canonical audio samples enter the same `transcription_session` runtime used by
/// recording, and callers can replay the resulting `EngineEvent`s through
/// `PresentationEmitter`/overlay code without touching the microphone.
pub async fn collect_buffered_engine_events(
    samples: &[f32],
    sample_rate: u32,
    language: Option<String>,
) -> Result<Vec<EngineEvent>> {
    if samples.is_empty() {
        return Ok(Vec::new());
    }

    let chunk_size = ((sample_rate as f32) * 0.1).round().max(1.0) as usize;

    let (tx, rx) = mpsc::channel::<Vec<f32>>(8);
    let collector = Arc::new(SessionEventCollector::new());
    let event_sink: Arc<dyn EventSink> = collector.clone();
    let session = tokio::spawn(transcription_session(
        rx,
        event_sink,
        SessionConfig {
            sample_rate,
            language,
            stream_log_path: None,
            utterance_silence_sec: None,
        },
    ));

    for chunk in samples.chunks(chunk_size) {
        if tx.send(chunk.to_vec()).await.is_err() {
            return Err(anyhow!("Transcription session dropped channel"));
        }
    }
    drop(tx);

    session
        .await
        .map_err(|e| anyhow!("Transcription session join error: {}", e))?;

    Ok(collector.events())
}

#[cfg(test)]
mod session_tests {
    use super::*;

    #[test]
    fn correction_buffer_window_cap() {
        let sr = 16_000u32;
        let window = CORRECTION_WINDOW_SEC;
        let cap = (window * sr as f32) as usize;

        // Under cap: untouched, nothing drained.
        let mut buf: Vec<f32> = vec![0.0; cap / 2];
        let len_before = buf.len();
        assert_eq!(cap_correction_buffer(&mut buf, sr, window), 0);
        assert_eq!(buf.len(), len_before);

        // Grow well past the cap across several 1s extends (25s > 18s window):
        // buffer must never exceed cap.
        let chunks = 25u32;
        let mut buf: Vec<f32> = Vec::new();
        for chunk in 0..chunks {
            let chunk_samples: Vec<f32> = (0..sr).map(|i| (chunk * sr + i) as f32).collect();
            buf.extend_from_slice(&chunk_samples);
            cap_correction_buffer(&mut buf, sr, window);
            assert!(buf.len() <= cap, "buffer {} exceeds cap {}", buf.len(), cap);
        }
        // After overflow it is pinned to exactly the cap...
        assert_eq!(buf.len(), cap);
        // ...and holds the freshest tail (last sample is the most recent one).
        let last = *buf.last().unwrap();
        assert_eq!(last, (chunks * sr - 1) as f32);

        // Zero window disables capping (no panic, no drain).
        let mut buf: Vec<f32> = vec![1.0; 100];
        assert_eq!(cap_correction_buffer(&mut buf, sr, 0.0), 0);
        assert_eq!(buf.len(), 100);
    }
}
