//! Streaming transcription pipeline — orchestration, buffered emission, and policy.
//!
//! Extracted from `audio::streaming_recorder` to decouple pipeline logic
//! (hallucination filtering, overlap dedup, re-transcription, buffered "typing"
//! emission) from the audio capture layer.
//!
//! Created by M&K (c)2026 VetCoders

use crate::audio::chunker::{SpeechEvent, SpeechSession};
#[cfg(any(test, feature = "offline_eval"))]
use crate::pipeline::dedup::dedup_chunk_overlap;
use crate::pipeline::dedup::{strip_segment_overlap, strip_suffix_overlap_live};
use crate::pipeline::stream_postprocess::StreamPostProcessor;
use crate::stt::scheduler::{SttLane, SttScheduler, SttTaskHandle};
#[cfg(any(test, feature = "offline_eval"))]
use crate::stt::whisper::singleton::engine as get_engine;
use crate::vad;
use anyhow::{Result, anyhow};
use chrono::SecondsFormat;
use futures_util::StreamExt;
use futures_util::stream::FuturesOrdered;
use lazy_static::lazy_static;
use regex::Regex;
use std::collections::VecDeque;
use std::sync::Arc;
use std::{fs::OpenOptions, io::Write, path::Path};
use tokio::sync::{Mutex, mpsc};
use tokio::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

// ── Constants ────────────────────────────────────────────────────────────────

#[cfg(any(test, feature = "offline_eval"))]
const DEFAULT_CHUNK_DURATION_SEC: f32 = 4.0;
#[cfg(any(test, feature = "offline_eval"))]
const DEFAULT_OVERLAP_RATIO: f32 = 0.25; // 25% overlap for stronger context continuity
// Golden runtime profile (balanced for low-latency preview + stable quality).
const DEFAULT_BUFFER_DELAY_MS: u64 = 280;
const DEFAULT_TYPING_CPS: f32 = 90.0;
const DEFAULT_EMIT_WORDS_MAX: usize = 2;
const PARTIAL_PASS_TRIGGER_UTTERANCE_FINALS: u32 = 2;
const PARTIAL_PASS_TRIGGER_SILERO_SPEECH_MS: u64 = 3_500;
const PARTIAL_PASS_TRIGGER_WATCHDOG_MS: u64 = 6_500;

lazy_static! {
    static ref TOKEN_RE: Regex = Regex::new(r"\s+|\S+\s*").expect("token regex");
}

// ── Pipeline contracts ───────────────────────────────────────────────────────

use crate::pipeline::contracts::{
    DeltaSink, DropKind, EngineEvent, EventSink, TranscriptDelta, TranscriptSegment,
};

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

// ── Hallucination filter ─────────────────────────────────────────────────────

const WHISPER_HALLUCINATIONS_COMMON: &[&str] = &[
    "thank you",
    "thanks for watching",
    "thanks for listening",
    "dziękuję za uwagę",
    "do zobaczenia",
    "subscribe",
    "like and subscribe",
    ".com",
    "codescribe",
    "www.",
];

const WHISPER_HALLUCINATIONS_PL: &[&str] = &[
    "napisy stworzone przez społeczność",
    "tłumaczenie",
    "transkrypcja",
];

const SHORT_SPEECH_WHITELIST: &[&str] = &[
    "tak", "nie", "co?", "co", "dobra", "dobrze", "ok", "okej", "no", "no?", "mhm", "aha", "jasne",
    "pewnie", "super", "hej", "halo", "cześć", "siema", "dzięki", "proszę",
];

const MIN_UTTERANCE_SEC: f32 = 0.50;
const SHORT_UTTERANCE_LOW_CONFIDENCE: f32 = 0.55;
const MAX_WORDS_PER_SEC: f32 = 5.0;
const WORD_RATE_MIN_WORDS: usize = 6;

fn is_polish_language(language: Option<&str>) -> bool {
    language
        .map(|lang| {
            let normalized = lang.to_ascii_lowercase();
            normalized == "pl" || normalized.starts_with("pl-")
        })
        .unwrap_or(false)
}

fn text_words_per_second(text: &str, audio_samples: usize, sample_rate: u32) -> Option<f32> {
    if audio_samples == 0 || sample_rate == 0 {
        return None;
    }
    let words = text.split_whitespace().count();
    if words < WORD_RATE_MIN_WORDS {
        return None;
    }
    let duration_s = audio_samples as f32 / sample_rate as f32;
    if duration_s <= 0.0 {
        return None;
    }
    Some(words as f32 / duration_s)
}

fn emit_vad_warning(event_sink: &Arc<dyn EventSink>, session: &mut SpeechSession) {
    if let Some(stats) = session.take_vad_error_stats() {
        event_sink.on_event(&EngineEvent::Warning {
            code: "vad_degraded".to_string(),
            message: format!(
                "VAD degraded in current batch: predict_errors={} unavailable_frames={} (totals: predict_errors={} unavailable_frames={})",
                stats.predict_errors,
                stats.unavailable_frames,
                stats.total_predict_errors,
                stats.total_unavailable_frames
            ),
        });
    }
}

fn should_drop_short_utterance(audio_samples: usize, sample_rate: u32, speech_prob: f32) -> bool {
    let duration_s = audio_samples as f32 / sample_rate as f32;
    duration_s < MIN_UTTERANCE_SEC && speech_prob < SHORT_UTTERANCE_LOW_CONFIDENCE
}

fn silero_vad_samples_to_ms(samples: u64) -> u64 {
    samples.saturating_mul(1_000) / u64::from(vad::VAD_SAMPLE_RATE)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PartialPassTrigger {
    Utterance,
    Speech,
    Watchdog,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PartialPassTriggerFlags {
    utterance_finals: bool,
    silero_speech: bool,
    watchdog: bool,
}

impl PartialPassTriggerFlags {
    fn primary_reason(self) -> Option<PartialPassTrigger> {
        if self.utterance_finals {
            Some(PartialPassTrigger::Utterance)
        } else if self.silero_speech {
            Some(PartialPassTrigger::Speech)
        } else if self.watchdog {
            Some(PartialPassTrigger::Watchdog)
        } else {
            None
        }
    }
}

#[derive(Debug)]
struct PartialPassTriggerState {
    utterance_finals_since_partial: u32,
    silero_speech_ms_since_partial: u64,
    watchdog_baseline: Instant,
}

impl PartialPassTriggerState {
    fn new(now: Instant) -> Self {
        Self {
            utterance_finals_since_partial: 0,
            silero_speech_ms_since_partial: 0,
            watchdog_baseline: now,
        }
    }

    fn observe_speech_event(&mut self, is_final: bool, silero_speech_vad_samples: u64) {
        if is_final {
            self.utterance_finals_since_partial =
                self.utterance_finals_since_partial.saturating_add(1);
        }
        self.silero_speech_ms_since_partial = self
            .silero_speech_ms_since_partial
            .saturating_add(silero_vad_samples_to_ms(silero_speech_vad_samples));
    }

    fn evaluate(&self, now: Instant) -> PartialPassTriggerFlags {
        let watchdog_elapsed_ms = now.duration_since(self.watchdog_baseline).as_millis() as u64;
        PartialPassTriggerFlags {
            utterance_finals: self.utterance_finals_since_partial
                >= PARTIAL_PASS_TRIGGER_UTTERANCE_FINALS,
            silero_speech: self.silero_speech_ms_since_partial
                >= PARTIAL_PASS_TRIGGER_SILERO_SPEECH_MS,
            watchdog: watchdog_elapsed_ms >= PARTIAL_PASS_TRIGGER_WATCHDOG_MS,
        }
    }

    fn reset_after_success(&mut self, now: Instant) {
        self.utterance_finals_since_partial = 0;
        self.silero_speech_ms_since_partial = 0;
        self.watchdog_baseline = now;
    }
}

fn silero_speech_seconds(speech_ms: u64) -> f32 {
    speech_ms as f32 / 1_000.0
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct EnqueueOutcome {
    enqueued: bool,
    dropped: u64,
    evicted_final: bool,
}

fn enqueue_pending_utterance(
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

pub(crate) fn is_hallucination(text: &str, language: Option<&str>) -> bool {
    let lower = text.trim().to_lowercase();
    if SHORT_SPEECH_WHITELIST.iter().any(|w| lower == *w) {
        return false;
    }
    let is_pl = is_polish_language(language);
    if WHISPER_HALLUCINATIONS_COMMON.iter().any(|h| lower == *h)
        || (is_pl && WHISPER_HALLUCINATIONS_PL.iter().any(|h| lower == *h))
    {
        return true;
    }
    if lower.len() < 30
        && (WHISPER_HALLUCINATIONS_COMMON
            .iter()
            .any(|h| lower.contains(h))
            || (is_pl && WHISPER_HALLUCINATIONS_PL.iter().any(|h| lower.contains(h))))
        && lower.split_whitespace().count() <= 4
    {
        return true;
    }
    false
}

// ── TranscriptionPipeline ────────────────────────────────────────────────────

pub(crate) struct TranscriptionPipeline {
    pub(crate) language: Option<String>,
    pub(crate) postprocessor: StreamPostProcessor,
    pub(crate) last_suffix: String,
    pub(crate) last_segment_end_ts: Option<f32>,
    pub(crate) hallucination_drops: u64,
    pub(crate) overlap_strips: u64,
}

/// Reason a postprocess step dropped content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PostprocessDrop {
    Hallucination,
    OverlapEmpty,
    /// Text was empty after lexicon + cleanup (NOT semantic gate — utterance path
    /// never applies the embedding-based gate).
    FilteredEmpty,
}

impl TranscriptionPipeline {
    pub fn new(language: Option<String>) -> Self {
        Self {
            language,
            postprocessor: StreamPostProcessor::new(),
            last_suffix: String::new(),
            last_segment_end_ts: None,
            hallucination_drops: 0,
            overlap_strips: 0,
        }
    }

    pub(crate) fn strip_overlap(&self, text: &str) -> String {
        strip_suffix_overlap_live(&self.last_suffix, text)
    }

    fn strip_overlap_with_segments(
        &self,
        text: &str,
        segments: &[TranscriptSegment],
    ) -> (String, Option<f32>) {
        if let Some((stripped, newest_end_ts)) =
            strip_segment_overlap(self.last_segment_end_ts, segments)
        {
            return (stripped, newest_end_ts);
        }
        (self.strip_overlap(text), None)
    }

    /// Postprocess an utterance and return the drop reason on failure.
    pub(crate) fn postprocess_with_reason(
        &mut self,
        text: &str,
    ) -> Result<String, PostprocessDrop> {
        self.postprocess_with_reason_and_segments(text, &[])
    }

    /// Segment-aware postprocess: uses timestamp overlap dedup where segment
    /// metadata is present, otherwise falls back to text-only suffix dedup.
    pub(crate) fn postprocess_with_reason_and_segments(
        &mut self,
        text: &str,
        segments: &[TranscriptSegment],
    ) -> Result<String, PostprocessDrop> {
        if is_hallucination(text, self.language.as_deref()) {
            self.hallucination_drops += 1;
            return Err(PostprocessDrop::Hallucination);
        }

        let (stripped, newest_segment_end_ts) = self.strip_overlap_with_segments(text, segments);
        if stripped.is_empty() {
            self.overlap_strips += 1;
            return Err(PostprocessDrop::OverlapEmpty);
        }

        match self.postprocessor.process_utterance(&stripped) {
            Some(processed) => {
                self.update_suffix(&processed);
                if let Some(end_ts) = newest_segment_end_ts {
                    self.last_segment_end_ts = Some(end_ts);
                }
                Ok(processed)
            }
            None => Err(PostprocessDrop::FilteredEmpty),
        }
    }

    fn update_suffix(&mut self, processed: &str) {
        let suffix_len = 50;
        let mut start = processed.len();
        let mut iter = processed.char_indices().rev();
        for _ in 0..suffix_len {
            if let Some((idx, _)) = iter.next() {
                start = idx;
            } else {
                start = 0;
                break;
            }
        }
        self.last_suffix = processed.get(start..).unwrap_or("").to_string();
    }
}

/// Run correction postprocess against a snapshot suffix without permanently
/// mutating pipeline suffix state on failure.
fn postprocess_correction_with_snapshot(
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

fn correction_is_stale(
    expected_preview_rev: u64,
    current_preview_rev: u64,
    expected_text: &str,
    current_text: &str,
) -> bool {
    expected_preview_rev != current_preview_rev || expected_text != current_text
}

#[derive(Debug, Default, Clone, Copy)]
struct PartialPassTelemetry {
    runs_total: u64,
    trigger_utterance_count: u64,
    trigger_speech_count: u64,
    trigger_watchdog_count: u64,
    stale_count: u64,
    coalesced_count: u64,
    dropped_count: u64,
}

impl PartialPassTelemetry {
    fn record_run(&mut self, trigger: PartialPassTrigger) {
        self.runs_total = self.runs_total.saturating_add(1);
        match trigger {
            PartialPassTrigger::Utterance => {
                self.trigger_utterance_count = self.trigger_utterance_count.saturating_add(1);
            }
            PartialPassTrigger::Speech => {
                self.trigger_speech_count = self.trigger_speech_count.saturating_add(1);
            }
            PartialPassTrigger::Watchdog => {
                self.trigger_watchdog_count = self.trigger_watchdog_count.saturating_add(1);
            }
        }
    }

    fn record_stale(&mut self) {
        self.stale_count = self.stale_count.saturating_add(1);
    }

    fn record_coalesced(&mut self) {
        self.coalesced_count = self.coalesced_count.saturating_add(1);
    }

    fn record_dropped(&mut self) {
        self.dropped_count = self.dropped_count.saturating_add(1);
    }
}

fn classify_partial_trigger(flags: PartialPassTriggerFlags) -> Option<PartialPassTrigger> {
    flags.primary_reason()
}

#[allow(clippy::too_many_arguments)]
fn schedule_partial_pass(
    stt_scheduler: &SttScheduler,
    output_sample_rate: u32,
    pipeline_language: Option<String>,
    correction_audio_buf: &mut Vec<f32>,
    correction_in_flight: &mut Option<SttTaskHandle>,
    correction_expected_preview_rev: &mut Option<u64>,
    correction_expected_text: &mut Option<String>,
    correction_suffix_snapshot: &mut Option<String>,
    suffix_snapshot: &str,
    preview_rev: u64,
    accumulated_text: &str,
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
        expected_rev = preview_rev,
        baseline_len = accumulated_text.chars().count(),
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
            *correction_expected_preview_rev = Some(preview_rev);
            *correction_expected_text = Some(accumulated_text.to_string());
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

// ── BufferedEmitter ──────────────────────────────────────────────────────────

/// Typing-animation emitter for transcript segments.
///
/// Buffers incoming text and emits it character-by-character at a configurable
/// typing speed via `DeltaSink`. Used by `app::presentation::PresentationEmitter`.
pub struct BufferedEmitter {
    queue: VecDeque<String>,
    initial_delay_ms: u64,
    typing_speed_cps: f32,
    emit_words_max: usize,
    correction_prefix_ratio: f64,
    first_output_at: Option<Instant>,
    current_segment: Option<String>,
    current_tokens: Vec<String>,
    current_token_index: usize,
    delta_callback: Option<Arc<dyn DeltaSink>>,
    transcript_buffer: Arc<Mutex<String>>,
    stream_log_path: Option<std::path::PathBuf>,
    finished: bool,
    has_output: bool,
    emitted_text: String,
    correction_pending: Option<String>,
    last_correction_at: Option<Instant>,
    pub(crate) corrections_applied: u64,
}

impl BufferedEmitter {
    pub fn new(
        transcript_buffer: Arc<Mutex<String>>,
        delta_callback: Option<Arc<dyn DeltaSink>>,
        stream_log_path: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            queue: VecDeque::new(),
            initial_delay_ms: env_u64("CODESCRIBE_BUFFER_DELAY_MS", DEFAULT_BUFFER_DELAY_MS),
            typing_speed_cps: env_f32("CODESCRIBE_TYPING_CPS", DEFAULT_TYPING_CPS).max(5.0),
            emit_words_max: env_usize("CODESCRIBE_EMIT_WORDS_MAX", DEFAULT_EMIT_WORDS_MAX)
                .clamp(1, 10),
            correction_prefix_ratio: buffered_correction_prefix_ratio(),
            first_output_at: None,
            current_segment: None,
            current_tokens: Vec::new(),
            current_token_index: 0,
            delta_callback,
            transcript_buffer,
            stream_log_path,
            finished: false,
            has_output: false,
            emitted_text: String::new(),
            correction_pending: None,
            last_correction_at: None,
            corrections_applied: 0,
        }
    }

    pub fn push_correction(&mut self, corrected: String) {
        if self.emitted_text.is_empty() {
            return;
        }
        // Guard: reject corrections that would rewrite most of the text.
        // Common-prefix must cover >= ratio (default: 60%) of the shorter string.
        let prefix_len = self
            .emitted_text
            .chars()
            .zip(corrected.chars())
            .take_while(|(a, b)| a == b)
            .count();
        let min_len = self
            .emitted_text
            .chars()
            .count()
            .min(corrected.chars().count());
        if min_len > 0 && (prefix_len as f64 / min_len as f64) < self.correction_prefix_ratio {
            debug!(
                "Correction rejected: common prefix {}/{} ({:.0}%) < {:.0}%",
                prefix_len,
                min_len,
                prefix_len as f64 / min_len as f64 * 100.0,
                self.correction_prefix_ratio * 100.0,
            );
            return;
        }
        self.correction_pending = Some(corrected);
    }

    pub fn push_segment(&mut self, text: String) {
        if text.trim().is_empty() {
            return;
        }
        let mut segment = text;
        if !segment.starts_with(char::is_whitespace)
            && (self.has_output || self.current_segment.is_some() || !self.queue.is_empty())
        {
            segment = format!(" {}", segment);
        }
        self.queue.push_back(segment);
        if self.first_output_at.is_none() {
            self.first_output_at = Some(Instant::now());
        }
    }

    pub async fn tick(&mut self) -> bool {
        if self.finished && self.queue.is_empty() && self.current_segment.is_none() {
            return true;
        }

        // Skip initial delay for the very first emission — gives instant visual
        // feedback that recording is working. Subsequent emissions use normal buffering.
        if self.is_buffering() && self.has_output {
            return false;
        }

        const CORRECTION_COOLDOWN_MS: u64 = 120;
        if let Some(ref _corrected) = self.correction_pending {
            let can_correct = self
                .last_correction_at
                .map(|t| t.elapsed() >= Duration::from_millis(CORRECTION_COOLDOWN_MS))
                .unwrap_or(true);
            if can_correct
                && let Some(corrected) = self.correction_pending.take()
                && let Some(delta) = build_redacted_delta(&self.emitted_text, &corrected)
            {
                apply_delta_to_string(&mut self.emitted_text, &delta);
                {
                    let mut buffer = self.transcript_buffer.lock().await;
                    *buffer = self.emitted_text.clone();
                }
                if let Some(sink) = &self.delta_callback {
                    sink.apply(&TranscriptDelta::from_raw(&delta));
                }
                if let Some(path) = self.stream_log_path.as_deref() {
                    let _ = append_to_stream_log(path, &delta);
                }
                self.last_correction_at = Some(Instant::now());
                self.corrections_applied += 1;
                return false;
            }
        }

        if self.current_segment.is_none() {
            self.current_segment = self.queue.pop_front();
            self.current_tokens = self
                .current_segment
                .as_deref()
                .map(tokenize_for_emit)
                .unwrap_or_default();
            self.current_token_index = 0;
        }

        if let Some(delta) = self.next_emit_chunk() {
            self.has_output = true;
            self.emitted_text.push_str(&delta);
            {
                let mut buffer = self.transcript_buffer.lock().await;
                apply_delta_to_string(&mut buffer, &delta);
            }

            if let Some(sink) = &self.delta_callback {
                sink.apply(&TranscriptDelta::from_raw(&delta));
            }

            if let Some(path) = self.stream_log_path.as_deref() {
                let _ = append_to_stream_log(path, &delta);
            }
        }

        self.finished && self.queue.is_empty() && self.current_segment.is_none()
    }

    fn is_buffering(&self) -> bool {
        let Some(start) = self.first_output_at else {
            return true;
        };
        start.elapsed() < Duration::from_millis(self.initial_delay_ms)
    }

    pub fn finish(&mut self) {
        self.finished = true;
    }

    fn next_emit_chunk(&mut self) -> Option<String> {
        let _ = self.current_segment.as_ref()?;
        if self.current_token_index >= self.current_tokens.len() {
            self.current_segment = None;
            self.current_tokens.clear();
            self.current_token_index = 0;
            return None;
        }

        let mut chunk = String::new();
        let mut words = 0usize;

        while self.current_token_index < self.current_tokens.len() {
            let token = &self.current_tokens[self.current_token_index];
            chunk.push_str(token);
            if token.chars().any(|c| !c.is_whitespace()) {
                words += 1;
            }
            self.current_token_index += 1;

            if words >= self.emit_words_max {
                if self.current_token_index < self.current_tokens.len() {
                    let next = &self.current_tokens[self.current_token_index];
                    if next.chars().all(|c| c.is_whitespace()) {
                        chunk.push_str(next);
                        self.current_token_index += 1;
                    }
                }
                break;
            }
        }

        if self.current_token_index >= self.current_tokens.len() {
            self.current_segment = None;
            self.current_tokens.clear();
            self.current_token_index = 0;
        }

        if chunk.is_empty() { None } else { Some(chunk) }
    }
}

// ── Emitter tick loop ────────────────────────────────────────────────────────

/// Drives the `BufferedEmitter` tick loop at the configured typing speed.
pub async fn emitter_tick_loop(emitter: Arc<Mutex<BufferedEmitter>>) {
    let interval = {
        let guard = emitter.lock().await;
        Duration::from_secs_f32(1.0 / guard.typing_speed_cps)
    };
    let mut ticker = tokio::time::interval(interval);

    loop {
        ticker.tick().await;
        let should_stop = {
            let mut guard = emitter.lock().await;
            guard.tick().await
        };
        if should_stop {
            break;
        }
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

            match stt_scheduler.submit(lane, inference_audio, output_sample_rate, lang) {
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
                            let (utterance, inference_audio, is_final, max_speech_prob) = match event {
                                SpeechEvent::Utterance(u) => {
                                    current_utterance_audio.extend_from_slice(&u);
                                    (u.clone(), u, false, session.segment_speech_prob())
                                }
                                SpeechEvent::UtteranceFinal(u) => {
                                    current_utterance_audio.extend_from_slice(&u);
                                    let full = std::mem::take(&mut current_utterance_audio);
                                    (u, full, true, session.segment_speech_prob())
                                }
                                _ => continue,
                            };
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
                                    is_final,
                                    max_speech_prob,
                                    speech_vad_samples,
                                },
                                MAX_PENDING_UTTERANCES,
                            );
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
                                    is_final,
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
                            if !outcome.enqueued {
                                continue;
                            }
                        }
                        emit_vad_warning(&event_sink, &mut session);
                    }
                    None => {
                        audio_closed = true;
                        if let Some(event) = session.flush() {
                            let speech_vad_samples = session.take_event_speech_vad_samples();
                            let (utterance, inference_audio, is_final, max_speech_prob) = match event {
                                SpeechEvent::Utterance(u) => {
                                    current_utterance_audio.extend_from_slice(&u);
                                    (u.clone(), u, false, session.segment_speech_prob())
                                }
                                SpeechEvent::UtteranceFinal(u) => {
                                    current_utterance_audio.extend_from_slice(&u);
                                    let full = std::mem::take(&mut current_utterance_audio);
                                    (u, full, true, session.segment_speech_prob())
                                }
                                _ => (Vec::new(), Vec::new(), false, 0.0),
                            };

                            if !utterance.is_empty() {
                                speech_activity_observed = true;
                                // Emit VadStart if this is the first speech (e.g. from flush).
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
                                        is_final,
                                        max_speech_prob,
                                        speech_vad_samples,
                                    },
                                    MAX_PENDING_UTTERANCES,
                                );
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
                                        is_final,
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
                partial_trigger_state.watchdog_baseline
                    + Duration::from_millis(PARTIAL_PASS_TRIGGER_WATCHDOG_MS)
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
                        } else if accumulated_text.is_empty() {
                            // Guard: if accumulated_text was cleared by FINAL but stale
                            // guard passed (edge case: dropped FINAL), skip — the
                            // utterance was already committed.
                            debug!("Skipping correction: accumulated_text is empty after FINAL");
                        } else {
                            match postprocess_correction_with_snapshot(
                                &mut pipeline,
                                &raw.text,
                                &suffix_snapshot,
                            ) {
                                Ok(cleaned) => {
                                    if cleaned != accumulated_text {
                                        let previous_text = accumulated_text.clone();
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
                                        // Update accumulated text so next Preview builds from corrected state.
                                        accumulated_text = cleaned;
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
                partial_trigger_state.observe_speech_event(item.is_final, item.speech_vad_samples);

                match result {
                    Ok(raw_transcript) => {
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
                                        // Final boundary commit: use full-utterance cleaned text as source of truth.
                                        accumulated_text = cleaned.trim().to_string();
                                        // Fix D: Append FINAL text to window-scoped state
                                        // (not replace — window spans multiple utterances).
                                        if !window_text.is_empty() {
                                            window_text.push(' ');
                                        }
                                        window_text.push_str(cleaned.trim());
                                        window_rev += 1;
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
                                event_sink.on_event(&EngineEvent::UtteranceFinal {
                                    utterance_id,
                                    text: final_text,
                                    raw_text: raw_text.clone(),
                                    start_ts: utterance_start_s,
                                    end_ts,
                                    segments: std::mem::take(&mut utterance_segments),
                                });
                            } else {
                                utterance_segments.clear();
                            }
                            accumulated_text.clear();
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
        event_sink.on_event(&EngineEvent::UtteranceFinal {
            utterance_id,
            text: remaining,
            raw_text: last_raw_text,
            start_ts: utterance_start_s,
            end_ts,
            segments,
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
        trigger_watchdog_count: partial_telemetry.trigger_watchdog_count,
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
        partial_telemetry.trigger_watchdog_count,
        partial_telemetry.stale_count,
        partial_telemetry.coalesced_count,
        partial_telemetry.dropped_count
    );
}

#[derive(Debug)]
struct PendingUtteranceWorkItem {
    audio: Vec<f32>,
    inference_audio: Vec<f32>,
    is_final: bool,
    max_speech_prob: f32,
    speech_vad_samples: u64,
}

#[derive(Debug)]
struct UtteranceWorkItem {
    audio: Vec<f32>,
    inference_audio_len: usize,
    is_final: bool,
    speech_vad_samples: u64,
}

// ── Offline/test: batch streaming transcription ──────────────────────────────

/// Batch helper for offline evaluation on in-memory samples.
///
/// Not part of the runtime session path.
#[cfg(any(test, feature = "offline_eval"))]
pub fn transcribe_streaming_samples(
    samples: &[f32],
    sample_rate: u32,
    language: Option<&str>,
    mut postprocessor: Option<&mut StreamPostProcessor>,
) -> Result<String> {
    if samples.is_empty() {
        return Ok(String::new());
    }

    let chunk_duration_sec = stream_chunk_duration_sec();
    let overlap_sec = stream_overlap_sec(chunk_duration_sec);
    let chunk_limit = (sample_rate as f32 * chunk_duration_sec) as usize;
    let overlap_size = (sample_rate as f32 * overlap_sec) as usize;
    let step = chunk_limit.saturating_sub(overlap_size).max(1);

    let total_audio_sec = samples.len() as f32 / sample_rate as f32;
    let stride_sec = chunk_duration_sec - overlap_sec;
    let n_chunks =
        ((samples.len().saturating_sub(chunk_limit)) as f32 / step as f32).ceil() as usize + 1;
    let processing_factor = chunk_duration_sec / stride_sec;
    let effective_audio_sec = n_chunks as f32 * chunk_duration_sec;

    info!(
        "[STREAM_DIAG] chunk={:.1}s overlap={:.1}s stride={:.1}s | audio={:.1}s chunks={} factor={:.2}x effective={:.1}s",
        chunk_duration_sec,
        overlap_sec,
        stride_sec,
        total_audio_sec,
        n_chunks,
        processing_factor,
        effective_audio_sec
    );

    let engine_mutex = get_engine()?;
    let mut engine = engine_mutex
        .lock()
        .map_err(|e| anyhow!("Lock error: {}", e))?;

    let mut out = String::new();
    let mut offset = 0usize;
    let mut chunks_processed = 0usize;
    let t_start = std::time::Instant::now();

    while offset < samples.len() {
        let end = (offset + chunk_limit).min(samples.len());
        let chunk = &samples[offset..end];
        let chunk_sec = chunk.len() as f32 / sample_rate as f32;
        let t_chunk = std::time::Instant::now();
        let text = engine.transcribe_with_language(chunk, sample_rate, language)?;
        let chunk_ms = t_chunk.elapsed().as_millis();
        chunks_processed += 1;

        debug!(
            "[STREAM_CHUNK] #{} offset={:.1}s len={:.1}s transcribe={}ms words={}",
            chunks_processed,
            offset as f32 / sample_rate as f32,
            chunk_sec,
            chunk_ms,
            text.split_whitespace().count()
        );

        if let Some(processor) = postprocessor.as_mut() {
            if let Some(cleaned) = processor.process(&text) {
                dedup_chunk_overlap(&mut out, &cleaned);
            }
        } else {
            dedup_chunk_overlap(&mut out, &text);
        }

        if end == samples.len() {
            break;
        }
        offset = offset.saturating_add(step);
    }

    let total_ms = t_start.elapsed().as_millis();
    info!(
        "[STREAM_DONE] chunks_processed={} total_ms={} out_words={}",
        chunks_processed,
        total_ms,
        out.split_whitespace().count()
    );

    // Optional: apply lexicon post-processing to streaming output.
    // Disabled by default; enable explicitly for offline-eval comparisons.
    if env_bool_default("CODESCRIBE_STREAM_LEXICON", false) && !out.trim().is_empty() {
        let mut lex = StreamPostProcessor::new();
        if let Some(cleaned) = lex.process(&out) {
            out = cleaned;
        }
    }

    Ok(out)
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

// ── Delta helpers ────────────────────────────────────────────────────────────

pub(crate) fn build_redacted_delta(before: &str, after: &str) -> Option<String> {
    crate::pipeline::contracts::TranscriptDelta::from_diff(before, after).map(|td| td.delta)
}

pub(crate) fn apply_delta_to_string(target: &mut String, delta: &str) {
    crate::pipeline::contracts::TranscriptDelta::from_raw(delta).apply(target);
}

fn tokenize_for_emit(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for m in TOKEN_RE.find_iter(text) {
        tokens.push(m.as_str().to_string());
    }
    if tokens.is_empty() && !text.is_empty() {
        tokens.push(text.to_string());
    }
    tokens
}

// ── Logging ──────────────────────────────────────────────────────────────────

pub(crate) fn stream_log_path() -> Option<std::path::PathBuf> {
    if let Ok(path) = std::env::var("CODESCRIBE_STREAM_LOG_PATH") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return Some(std::path::PathBuf::from(trimmed));
        }
    }

    if env_bool("CODESCRIBE_STREAM_LOG") {
        let root = crate::config::Config::config_dir();
        return Some(root.join("stream.log"));
    }

    None
}

fn append_to_stream_log(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let ts = chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    let mut payload = text.replace('\n', "\\n").replace('\r', "\\r");
    payload = payload.replace('\u{0008}', "\\b");
    writeln!(file, "[{}] {}", ts, payload)?;
    Ok(())
}

// ── Env helpers ──────────────────────────────────────────────────────────────

pub(crate) fn env_bool(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

#[cfg(any(test, feature = "offline_eval"))]
pub(crate) fn env_bool_default(key: &str, default: bool) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(default)
}

fn env_f32(key: &str, default: f32) -> f32 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(default)
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(default)
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(default)
}

fn inference_max_concurrency() -> usize {
    // Whisper singleton uses a single engine lock; defaulting to 1 avoids queue churn
    // that looks like "parallelism" but mostly adds latency/jitter in preview.
    const DEFAULT_MAX_INFERENCE_CONCURRENCY: usize = 1;
    const HARD_MAX_INFERENCE_CONCURRENCY: usize = 4;
    env_usize(
        "CODESCRIBE_MAX_INFERENCE_CONCURRENCY",
        DEFAULT_MAX_INFERENCE_CONCURRENCY,
    )
    .clamp(1, HARD_MAX_INFERENCE_CONCURRENCY)
}

fn buffered_correction_prefix_ratio() -> f64 {
    env_f32("CODESCRIBE_BUFFERED_CORRECTION_PREFIX", 0.50).clamp(0.4, 0.9) as f64
}

#[cfg(any(test, feature = "offline_eval"))]
pub(crate) fn stream_chunk_duration_sec() -> f32 {
    env_f32("CODESCRIBE_STREAM_CHUNK_SEC", DEFAULT_CHUNK_DURATION_SEC).clamp(0.5, 30.0)
}

#[cfg(any(test, feature = "offline_eval"))]
pub(crate) fn stream_overlap_sec(chunk_duration_sec: f32) -> f32 {
    let ratio = env_f32("CODESCRIBE_STREAM_OVERLAP_RATIO", DEFAULT_OVERLAP_RATIO).clamp(0.05, 0.8);
    (chunk_duration_sec * ratio).min(chunk_duration_sec * 0.8)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::contracts::{RawTranscript, TranscriptSegment};
    use crate::pipeline::sinks::CollectorEventSink;
    use std::sync::{Condvar, Mutex as StdMutex};

    fn pending_item(is_final: bool) -> PendingUtteranceWorkItem {
        pending_item_with_marker(is_final, if is_final { 1.0 } else { 0.1 })
    }

    fn pending_item_with_marker(is_final: bool, marker: f32) -> PendingUtteranceWorkItem {
        PendingUtteranceWorkItem {
            audio: vec![marker; 32],
            inference_audio: vec![marker; 32],
            is_final,
            max_speech_prob: 0.9,
            speech_vad_samples: 512,
        }
    }

    #[test]
    fn test_postprocess_components() {
        // Hallucination
        assert!(is_hallucination("Thank you", None));
        assert!(is_hallucination("  Dziękuję za uwagę  ", Some("pl")));
        assert!(is_hallucination(
            "Napisy stworzone przez społeczność",
            Some("pl")
        ));
        assert!(!is_hallucination("Tak", Some("pl"))); // Whitelisted
        assert!(!is_hallucination("This is a normal sentence.", Some("en")));

        // Overlap
        let mut pipeline = TranscriptionPipeline::new(None);
        pipeline.last_suffix = "Alice has a cat.".to_string();

        let res = pipeline.strip_overlap("Alice has a cat. And a dog.");
        assert_eq!(res, "And a dog.");

        pipeline.last_suffix = "going to the park".to_string();
        let res = pipeline.strip_overlap("park tomorrow.");
        assert_eq!(res, "tomorrow.");

        let res = pipeline.strip_overlap("Hello world");
        assert_eq!(res, "Hello world");
    }

    #[test]
    fn test_strip_overlap_word_fallback_handles_punctuation_drift() {
        let mut pipeline = TranscriptionPipeline::new(None);
        pipeline.last_suffix = "Thank you.".to_string();

        let res = pipeline.strip_overlap("Thank you very much");
        assert_eq!(res, "very much");
    }

    #[test]
    fn test_strip_overlap_word_fallback_handles_polish_diacritic_drift() {
        let mut pipeline = TranscriptionPipeline::new(Some("pl".to_string()));
        pipeline.last_suffix = "pacjent czuje się już dobrze".to_string();

        let res = pipeline.strip_overlap("pacjent czuje się juz dobrze dzisiaj");
        assert_eq!(res, "dzisiaj");
    }

    #[test]
    fn test_postprocess_with_reason_uses_fuzzy_overlap_dedup() {
        let mut pipeline = TranscriptionPipeline::new(None);
        pipeline.last_suffix = "the patient is feeling much better".to_string();

        let result = pipeline.postprocess_with_reason("the patient is feelingg much better today");
        assert_eq!(
            result.expect("postprocess should keep non-overlap tail"),
            "today"
        );
    }

    #[test]
    fn test_postprocess_prefers_timestamp_overlap_when_segments_exist() {
        let mut pipeline = TranscriptionPipeline::new(None);
        pipeline.last_suffix = "unrelated suffix".to_string();
        pipeline.last_segment_end_ts = Some(1.0);

        let segments = vec![
            TranscriptSegment {
                text: "already emitted".to_string(),
                start_ts: 0.0,
                end_ts: 0.95,
            },
            TranscriptSegment {
                text: "fresh words".to_string(),
                start_ts: 1.0,
                end_ts: 1.50,
            },
        ];

        let cleaned = pipeline
            .postprocess_with_reason_and_segments("this text should not win", &segments)
            .expect("timestamp-aware strip should keep only fresh segment text");
        assert_eq!(cleaned, "fresh words");
        assert_eq!(pipeline.last_segment_end_ts, Some(1.50));
    }

    #[test]
    fn test_postprocess_with_segments_falls_back_to_text_path() {
        let mut pipeline = TranscriptionPipeline::new(None);
        pipeline.last_suffix = "hello world".to_string();
        pipeline.last_segment_end_ts = Some(7.0);

        let cleaned = pipeline
            .postprocess_with_reason_and_segments("world again", &[])
            .expect("empty segments should use suffix overlap fallback");
        assert_eq!(cleaned, "again");
        assert_eq!(
            pipeline.last_segment_end_ts,
            Some(7.0),
            "text fallback should not mutate timestamp overlap cursor"
        );
    }

    #[test]
    fn test_short_utterance_gate_requires_low_confidence() {
        let sample_rate = 16_000;
        let short = (0.2 * sample_rate as f32) as usize;
        assert!(should_drop_short_utterance(short, sample_rate, 0.40));
        assert!(!should_drop_short_utterance(short, sample_rate, 0.80));
    }

    #[test]
    fn test_enqueue_pending_utterance_preserves_final_boundary_when_full() {
        let mut pending = VecDeque::new();
        pending.push_back(pending_item(false));
        pending.push_back(pending_item(false));

        let outcome = enqueue_pending_utterance(&mut pending, pending_item(true), 2);
        assert!(outcome.enqueued, "final item should be admitted");
        assert_eq!(outcome.dropped, 1, "one older non-final should be evicted");
        assert!(
            !outcome.evicted_final,
            "non-final eviction should be preferred for final boundaries"
        );
        assert_eq!(pending.len(), 2);
        assert!(
            pending.back().is_some_and(|item| item.is_final),
            "latest queued item should be final boundary"
        );
    }

    #[test]
    fn test_enqueue_pending_utterance_drops_non_final_when_full() {
        let mut pending = VecDeque::new();
        pending.push_back(pending_item_with_marker(false, 1.0));
        pending.push_back(pending_item_with_marker(true, 2.0));

        let outcome =
            enqueue_pending_utterance(&mut pending, pending_item_with_marker(false, 3.0), 2);
        assert!(!outcome.enqueued);
        assert_eq!(outcome.dropped, 1);
        assert_eq!(pending.len(), 2, "queue should stay unchanged");
        let markers: Vec<f32> = pending.iter().map(|item| item.audio[0]).collect();
        assert_eq!(
            markers,
            vec![1.0, 2.0],
            "dropping a non-final under pressure must preserve queued work order"
        );
    }

    #[test]
    fn test_enqueue_pending_utterance_still_admits_final_when_only_finals_queued() {
        let mut pending = VecDeque::new();
        pending.push_back(pending_item(true));
        pending.push_back(pending_item(true));

        let outcome = enqueue_pending_utterance(&mut pending, pending_item(true), 2);
        assert!(outcome.enqueued, "latest final should still be admitted");
        assert_eq!(outcome.dropped, 1, "one older final should be evicted");
        assert!(outcome.evicted_final);
        assert_eq!(pending.len(), 2);
        assert!(pending.back().is_some_and(|item| item.is_final));
    }

    #[test]
    fn test_enqueue_pending_utterance_zero_capacity_drops_all_items() {
        let mut pending = VecDeque::new();

        let non_final = enqueue_pending_utterance(&mut pending, pending_item(false), 0);
        assert!(!non_final.enqueued);
        assert_eq!(non_final.dropped, 1);
        assert!(!non_final.evicted_final);

        let final_item = enqueue_pending_utterance(&mut pending, pending_item(true), 0);
        assert!(!final_item.enqueued);
        assert_eq!(final_item.dropped, 1);
        assert!(!final_item.evicted_final);
        assert!(
            pending.is_empty(),
            "zero-capacity queue should never retain pending work"
        );
    }

    #[test]
    fn test_enqueue_pending_utterance_final_evicts_oldest_non_final_in_mixed_queue() {
        let mut pending = VecDeque::new();
        pending.push_back(pending_item_with_marker(true, 1.0));
        pending.push_back(pending_item_with_marker(false, 2.0));
        pending.push_back(pending_item_with_marker(true, 3.0));

        let outcome =
            enqueue_pending_utterance(&mut pending, pending_item_with_marker(true, 4.0), 3);
        assert!(outcome.enqueued, "incoming final should be admitted");
        assert_eq!(outcome.dropped, 1);
        assert!(
            !outcome.evicted_final,
            "queue policy should evict a non-final before any final boundary"
        );
        let markers: Vec<f32> = pending.iter().map(|item| item.audio[0]).collect();
        assert_eq!(
            markers,
            vec![1.0, 3.0, 4.0],
            "oldest non-final should be removed while preserving final boundaries"
        );
        assert!(pending.iter().all(|item| item.is_final));
    }

    #[test]
    fn test_enqueue_pending_utterance_pressure_sequence_preserves_final_boundaries() {
        let mut pending = VecDeque::new();
        pending.push_back(pending_item_with_marker(false, 1.0));
        pending.push_back(pending_item_with_marker(false, 2.0));
        pending.push_back(pending_item_with_marker(false, 3.0));

        let drop_non_final =
            enqueue_pending_utterance(&mut pending, pending_item_with_marker(false, 4.0), 3);
        assert!(!drop_non_final.enqueued);
        assert_eq!(drop_non_final.dropped, 1);
        assert!(!drop_non_final.evicted_final);
        assert_eq!(
            pending
                .iter()
                .map(|item| item.audio[0])
                .collect::<Vec<f32>>(),
            vec![1.0, 2.0, 3.0]
        );

        let admit_final_a =
            enqueue_pending_utterance(&mut pending, pending_item_with_marker(true, 5.0), 3);
        assert!(admit_final_a.enqueued);
        assert_eq!(admit_final_a.dropped, 1);
        assert!(!admit_final_a.evicted_final);
        assert_eq!(
            pending
                .iter()
                .map(|item| item.audio[0])
                .collect::<Vec<f32>>(),
            vec![2.0, 3.0, 5.0]
        );

        let admit_final_b =
            enqueue_pending_utterance(&mut pending, pending_item_with_marker(true, 6.0), 3);
        assert!(admit_final_b.enqueued);
        assert_eq!(admit_final_b.dropped, 1);
        assert!(!admit_final_b.evicted_final);
        assert_eq!(
            pending
                .iter()
                .map(|item| item.audio[0])
                .collect::<Vec<f32>>(),
            vec![3.0, 5.0, 6.0]
        );

        let admit_final_c =
            enqueue_pending_utterance(&mut pending, pending_item_with_marker(true, 7.0), 3);
        assert!(admit_final_c.enqueued);
        assert_eq!(admit_final_c.dropped, 1);
        assert!(!admit_final_c.evicted_final);
        assert!(pending.iter().all(|item| item.is_final));
        assert_eq!(
            pending
                .iter()
                .map(|item| item.audio[0])
                .collect::<Vec<f32>>(),
            vec![5.0, 6.0, 7.0]
        );

        let drop_non_final_again =
            enqueue_pending_utterance(&mut pending, pending_item_with_marker(false, 8.0), 3);
        assert!(!drop_non_final_again.enqueued);
        assert_eq!(drop_non_final_again.dropped, 1);
        assert!(!drop_non_final_again.evicted_final);
        assert_eq!(
            pending
                .iter()
                .map(|item| item.audio[0])
                .collect::<Vec<f32>>(),
            vec![5.0, 6.0, 7.0]
        );

        let admit_final_d =
            enqueue_pending_utterance(&mut pending, pending_item_with_marker(true, 9.0), 3);
        assert!(admit_final_d.enqueued);
        assert_eq!(admit_final_d.dropped, 1);
        assert!(
            admit_final_d.evicted_final,
            "when only finals are queued, oldest final should be evicted"
        );
        assert_eq!(
            pending
                .iter()
                .map(|item| item.audio[0])
                .collect::<Vec<f32>>(),
            vec![6.0, 7.0, 9.0]
        );
        assert!(pending.iter().all(|item| item.is_final));
    }

    #[tokio::test]
    async fn test_enqueue_pending_utterance_pressure_sequence_under_async_saturated_load() {
        let mut pending = VecDeque::new();
        let mut dropped_total = 0u64;
        let mut dropped_non_finals = 0u64;
        let mut final_evictions = 0u64;

        let (tx, mut rx) = mpsc::channel::<PendingUtteranceWorkItem>(32);
        let producer = tokio::spawn(async move {
            let sequence = [
                (false, 1.0),
                (false, 2.0),
                (false, 3.0),
                (false, 4.0),
                (true, 5.0),
                (false, 6.0),
                (true, 7.0),
                (true, 8.0),
                (true, 9.0),
                (false, 10.0),
                (true, 11.0),
            ];
            for (is_final, marker) in sequence {
                tx.send(pending_item_with_marker(is_final, marker))
                    .await
                    .expect("async pressure sequence send should succeed");
                tokio::task::yield_now().await;
            }
        });

        while let Some(item) = rx.recv().await {
            // Simulate saturated inference slots by not draining the pending queue.
            let item_is_final = item.is_final;
            let outcome = enqueue_pending_utterance(&mut pending, item, 4);
            dropped_total = dropped_total.saturating_add(outcome.dropped);
            if !item_is_final && !outcome.enqueued {
                dropped_non_finals = dropped_non_finals.saturating_add(outcome.dropped);
            }
            if outcome.evicted_final {
                final_evictions = final_evictions.saturating_add(1);
            }
        }
        producer
            .await
            .expect("async pressure producer should finish");

        assert_eq!(
            pending
                .iter()
                .map(|item| item.audio[0])
                .collect::<Vec<f32>>(),
            vec![7.0, 8.0, 9.0, 11.0],
            "saturated async ingress should preserve newest final boundaries"
        );
        assert_eq!(pending.len(), 4);
        assert!(pending.iter().all(|item| item.is_final));
        assert_eq!(dropped_total, 7);
        assert_eq!(dropped_non_finals, 2);
        assert_eq!(final_evictions, 1);
    }

    #[tokio::test]
    async fn test_enqueue_pending_utterance_async_backpressure_recovers_after_drain() {
        let mut pending = VecDeque::new();
        let mut outcomes = Vec::new();
        let mut drained_marker = None;

        let (tx, mut rx) = mpsc::channel::<PendingUtteranceWorkItem>(16);
        let producer = tokio::spawn(async move {
            let sequence = [
                (true, 1.0),
                (true, 2.0),
                (true, 3.0),
                (false, 4.0),
                (true, 5.0),
                (false, 6.0),
                (true, 7.0),
            ];
            for (is_final, marker) in sequence {
                tx.send(pending_item_with_marker(is_final, marker))
                    .await
                    .expect("async queue-recovery sequence send should succeed");
                tokio::task::yield_now().await;
            }
        });

        while let Some(item) = rx.recv().await {
            let marker = item.audio[0] as u32;
            let outcome = enqueue_pending_utterance(&mut pending, item, 3);
            outcomes.push((
                marker,
                outcome.enqueued,
                outcome.dropped,
                outcome.evicted_final,
            ));

            // Simulate one inference slot freeing after the queue saturated with finals.
            if marker == 5 {
                let drained = pending
                    .pop_front()
                    .expect("simulated inference drain should pop one queued item");
                drained_marker = Some(drained.audio[0] as u32);
            }

            tokio::task::yield_now().await;
        }
        producer
            .await
            .expect("async queue-recovery producer should finish");

        assert_eq!(
            outcomes,
            vec![
                (1, true, 0, false),
                (2, true, 0, false),
                (3, true, 0, false),
                (4, false, 1, false),
                (5, true, 1, true),
                (6, true, 0, false),
                (7, true, 1, false),
            ],
            "backpressure policy should drop non-finals when saturated, recover after drain, and keep final precedence"
        );
        assert_eq!(
            drained_marker,
            Some(2),
            "drain should remove the current oldest final after a final-only eviction cycle"
        );
        assert_eq!(
            pending
                .iter()
                .map(|item| item.audio[0] as u32)
                .collect::<Vec<u32>>(),
            vec![3, 5, 7],
            "final enqueue after recovery should evict queued non-final first"
        );
        assert!(
            pending.iter().all(|item| item.is_final),
            "final boundaries should remain intact at the tail of async pressure+drain sequence"
        );
    }

    #[test]
    fn test_partial_trigger_contract_utterance_path() {
        let now = Instant::now();
        let mut state = PartialPassTriggerState::new(now);

        state.observe_speech_event(true, 0);
        state.observe_speech_event(true, 0);
        assert_eq!(
            classify_partial_trigger(state.evaluate(now + Duration::from_secs(1))),
            None
        );

        state.observe_speech_event(true, 0);
        assert_eq!(
            classify_partial_trigger(state.evaluate(now + Duration::from_secs(1))),
            Some(PartialPassTrigger::Utterance),
            "3 UtteranceFinal events should trigger partial pass"
        );
    }

    #[test]
    fn test_partial_trigger_contract_silero_speech_path() {
        let now = Instant::now();
        let mut state = PartialPassTriggerState::new(now);
        let one_second = u64::from(vad::VAD_SAMPLE_RATE);

        for _ in 0..5 {
            state.observe_speech_event(false, one_second);
        }
        assert_eq!(
            classify_partial_trigger(state.evaluate(now + Duration::from_secs(1))),
            None
        );

        state.observe_speech_event(false, one_second);
        assert_eq!(
            classify_partial_trigger(state.evaluate(now + Duration::from_secs(1))),
            Some(PartialPassTrigger::Speech),
            "6s of Silero-positive speech should trigger partial pass"
        );
    }

    #[test]
    fn test_partial_trigger_contract_watchdog_path() {
        let now = Instant::now();
        let state = PartialPassTriggerState::new(now);

        assert_eq!(
            classify_partial_trigger(state.evaluate(now + Duration::from_millis(11_999))),
            None
        );
        assert_eq!(
            classify_partial_trigger(state.evaluate(now + Duration::from_millis(12_000))),
            Some(PartialPassTrigger::Watchdog),
            "12s watchdog should trigger partial pass"
        );
    }

    #[test]
    fn test_partial_trigger_precedence_prefers_speech_over_watchdog_without_utterance_trigger() {
        let now = Instant::now();
        let mut state = PartialPassTriggerState::new(now);

        state.observe_speech_event(false, u64::from(vad::VAD_SAMPLE_RATE) * 6);
        let flags = state.evaluate(now + Duration::from_millis(PARTIAL_PASS_TRIGGER_WATCHDOG_MS));
        assert!(!flags.utterance_finals);
        assert!(flags.silero_speech);
        assert!(flags.watchdog);
        assert_eq!(
            classify_partial_trigger(flags),
            Some(PartialPassTrigger::Speech),
            "speech trigger should outrank watchdog when utterance-count threshold is not met"
        );
    }

    #[test]
    fn test_partial_trigger_precedence_matrix_is_explicit() {
        assert_eq!(
            classify_partial_trigger(PartialPassTriggerFlags {
                utterance_finals: true,
                silero_speech: true,
                watchdog: true,
            }),
            Some(PartialPassTrigger::Utterance),
            "utterance-count trigger should dominate when multiple trigger paths are true"
        );
        assert_eq!(
            classify_partial_trigger(PartialPassTriggerFlags {
                utterance_finals: false,
                silero_speech: true,
                watchdog: true,
            }),
            Some(PartialPassTrigger::Speech),
            "speech trigger should outrank watchdog when utterance threshold is not met"
        );
        assert_eq!(
            classify_partial_trigger(PartialPassTriggerFlags {
                utterance_finals: false,
                silero_speech: false,
                watchdog: true,
            }),
            Some(PartialPassTrigger::Watchdog),
            "watchdog should be selected when it is the only triggered path"
        );
    }

    #[test]
    fn test_partial_trigger_precedence_matrix_covers_all_flag_combinations() {
        let cases = [
            (false, false, false, None),
            (false, false, true, Some(PartialPassTrigger::Watchdog)),
            (false, true, false, Some(PartialPassTrigger::Speech)),
            (false, true, true, Some(PartialPassTrigger::Speech)),
            (true, false, false, Some(PartialPassTrigger::Utterance)),
            (true, false, true, Some(PartialPassTrigger::Utterance)),
            (true, true, false, Some(PartialPassTrigger::Utterance)),
            (true, true, true, Some(PartialPassTrigger::Utterance)),
        ];

        for (utterance_finals, silero_speech, watchdog, expected) in cases {
            assert_eq!(
                classify_partial_trigger(PartialPassTriggerFlags {
                    utterance_finals,
                    silero_speech,
                    watchdog,
                }),
                expected,
                "trigger precedence mismatch for flags: utterance_finals={utterance_finals}, silero_speech={silero_speech}, watchdog={watchdog}"
            );
        }
    }

    #[test]
    fn test_partial_trigger_coalesces_and_reset_clears_watchdog_baseline() {
        let now = Instant::now();
        let mut state = PartialPassTriggerState::new(now);
        let two_seconds = u64::from(vad::VAD_SAMPLE_RATE) * 2;

        for _ in 0..3 {
            state.observe_speech_event(true, two_seconds);
        }
        let due_at = now + Duration::from_millis(12_000);
        let flags = state.evaluate(due_at);
        assert!(flags.utterance_finals);
        assert!(flags.silero_speech);
        assert!(flags.watchdog);
        assert_eq!(
            classify_partial_trigger(flags),
            Some(PartialPassTrigger::Utterance),
            "simultaneous triggers should coalesce into one deterministic run"
        );

        state.reset_after_success(due_at);
        assert_eq!(
            classify_partial_trigger(state.evaluate(due_at + Duration::from_millis(1))),
            None,
            "successful partial pass must reset watchdog baseline"
        );
    }

    #[test]
    fn test_partial_trigger_reset_clears_utterance_and_speech_accumulators() {
        let now = Instant::now();
        let mut state = PartialPassTriggerState::new(now);
        let two_seconds = u64::from(vad::VAD_SAMPLE_RATE) * 2;

        for _ in 0..3 {
            state.observe_speech_event(true, two_seconds);
        }
        let due_at = now + Duration::from_millis(PARTIAL_PASS_TRIGGER_WATCHDOG_MS);
        assert_eq!(
            classify_partial_trigger(state.evaluate(due_at)),
            Some(PartialPassTrigger::Utterance)
        );

        state.reset_after_success(due_at);
        assert_eq!(
            state.evaluate(due_at + Duration::from_millis(1)),
            PartialPassTriggerFlags::default(),
            "reset should clear all trigger counters and watchdog elapsed time"
        );

        for _ in 0..2 {
            state.observe_speech_event(true, two_seconds);
        }
        assert_eq!(
            classify_partial_trigger(state.evaluate(due_at + Duration::from_millis(10))),
            None,
            "post-reset counters should require fresh accumulation before triggering again"
        );
    }

    #[tokio::test]
    async fn test_partial_trigger_paths_stay_stable_under_async_interleaving() {
        #[derive(Clone, Copy)]
        enum TriggerStep {
            Observe {
                is_final: bool,
                speech_samples: u64,
                advance_ms: u64,
            },
            Evaluate {
                advance_ms: u64,
                expected: Option<PartialPassTrigger>,
                reset_after_success: bool,
            },
        }

        let one_second = u64::from(vad::VAD_SAMPLE_RATE);
        let sequence = [
            TriggerStep::Observe {
                is_final: true,
                speech_samples: one_second,
                advance_ms: 100,
            },
            TriggerStep::Observe {
                is_final: true,
                speech_samples: one_second,
                advance_ms: 100,
            },
            TriggerStep::Observe {
                is_final: true,
                speech_samples: one_second,
                advance_ms: 100,
            },
            TriggerStep::Evaluate {
                advance_ms: 100,
                expected: Some(PartialPassTrigger::Utterance),
                reset_after_success: true,
            },
            TriggerStep::Observe {
                is_final: false,
                speech_samples: one_second,
                advance_ms: 100,
            },
            TriggerStep::Observe {
                is_final: false,
                speech_samples: one_second,
                advance_ms: 100,
            },
            TriggerStep::Observe {
                is_final: false,
                speech_samples: one_second,
                advance_ms: 100,
            },
            TriggerStep::Observe {
                is_final: false,
                speech_samples: one_second,
                advance_ms: 100,
            },
            TriggerStep::Observe {
                is_final: false,
                speech_samples: one_second,
                advance_ms: 100,
            },
            TriggerStep::Observe {
                is_final: false,
                speech_samples: one_second,
                advance_ms: 100,
            },
            TriggerStep::Evaluate {
                advance_ms: 100,
                expected: Some(PartialPassTrigger::Speech),
                reset_after_success: true,
            },
            TriggerStep::Evaluate {
                advance_ms: PARTIAL_PASS_TRIGGER_WATCHDOG_MS - 1,
                expected: None,
                reset_after_success: false,
            },
            TriggerStep::Evaluate {
                advance_ms: 1,
                expected: Some(PartialPassTrigger::Watchdog),
                reset_after_success: true,
            },
            TriggerStep::Evaluate {
                advance_ms: 1,
                expected: None,
                reset_after_success: false,
            },
        ];

        let (tx, mut rx) = mpsc::channel::<TriggerStep>(sequence.len());
        let producer = tokio::spawn(async move {
            for step in sequence {
                tx.send(step)
                    .await
                    .expect("trigger-step sequence send should succeed");
                tokio::task::yield_now().await;
            }
        });

        let start = Instant::now();
        let mut now = start;
        let mut state = PartialPassTriggerState::new(start);
        let mut telemetry = PartialPassTelemetry::default();

        while let Some(step) = rx.recv().await {
            match step {
                TriggerStep::Observe {
                    is_final,
                    speech_samples,
                    advance_ms,
                } => {
                    now += Duration::from_millis(advance_ms);
                    state.observe_speech_event(is_final, speech_samples);
                }
                TriggerStep::Evaluate {
                    advance_ms,
                    expected,
                    reset_after_success,
                } => {
                    now += Duration::from_millis(advance_ms);
                    let observed = classify_partial_trigger(state.evaluate(now));
                    assert_eq!(
                        observed, expected,
                        "trigger classification drifted under async interleaving"
                    );
                    if let Some(trigger) = observed {
                        telemetry.record_run(trigger);
                        if reset_after_success {
                            state.reset_after_success(now);
                        }
                    }
                }
            }
            tokio::task::yield_now().await;
        }
        producer
            .await
            .expect("async trigger-step producer should finish");

        assert_eq!(telemetry.runs_total, 3);
        assert_eq!(telemetry.trigger_utterance_count, 1);
        assert_eq!(telemetry.trigger_speech_count, 1);
        assert_eq!(telemetry.trigger_watchdog_count, 1);
    }

    #[test]
    fn test_word_rate_detection() {
        let sample_rate = 16_000;
        let half_second = (0.5 * sample_rate as f32) as usize;
        let wps = text_words_per_second("raz dwa trzy cztery pięć sześć", half_second, sample_rate)
            .expect("should compute words/s");
        assert!(wps > MAX_WORDS_PER_SEC);

        let normal = text_words_per_second(
            "to jest normalna fraza z kilkoma słowami",
            (sample_rate * 2) as usize,
            sample_rate,
        )
        .expect("should compute words/s");
        assert!(normal < MAX_WORDS_PER_SEC);
    }

    #[test]
    fn test_suffix_preserved_when_postprocess_filters() {
        // Simulates the re-transcription scenario: if postprocess drops content
        // (e.g. hallucination), last_suffix must stay at the pre-snapshot value.
        let mut pipeline = TranscriptionPipeline::new(None);
        pipeline.last_suffix = "original suffix".to_string();

        // "Thank you" is a hallucination — postprocess returns a drop reason.
        let result = pipeline.postprocess_with_reason("Thank you");
        assert!(matches!(result, Err(PostprocessDrop::Hallucination)));
        // last_suffix unchanged (strip_overlap was never reached)
        assert_eq!(pipeline.last_suffix, "original suffix");
    }

    #[test]
    fn test_suffix_updated_after_successful_postprocess() {
        let mut pipeline = TranscriptionPipeline::new(None);
        pipeline.last_suffix = "old tail".to_string();

        let result = pipeline.postprocess_with_reason("This is a brand new sentence.");
        assert!(result.is_ok());
        // last_suffix should now reflect the new text's suffix
        assert_ne!(pipeline.last_suffix, "old tail");
        assert!(pipeline.last_suffix.contains("sentence"));
    }

    #[test]
    fn test_correction_guard_rejects_low_prefix() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let buf = Arc::new(Mutex::new(String::new()));
            let mut emitter = BufferedEmitter::new(buf, None, None);
            emitter.emitted_text = "Hello world, this is a test.".to_string();

            // Completely different text — should be rejected (<70% prefix)
            emitter.push_correction("Goodbye universe, nothing alike.".to_string());
            assert!(emitter.correction_pending.is_none());

            // Similar text with minor tail fix — should be accepted (>70% prefix)
            emitter.push_correction("Hello world, this is a test!".to_string());
            assert!(emitter.correction_pending.is_some());
        });
    }

    #[test]
    fn test_correction_delta() {
        let before = "This is a dratf.";
        let after = "This is a draft.";
        let delta = build_redacted_delta(before, after).expect("should produce delta");

        assert!(delta.contains("\u{0008}\u{0008}\u{0008}"));
        assert!(delta.ends_with("ft."));

        let mut target = before.to_string();
        apply_delta_to_string(&mut target, &delta);
        assert_eq!(target, after);
    }

    #[test]
    fn test_correction_delta_polish_diacritics() {
        let before = "chciałbym zostać weterynarzem.";
        let after = "chciałbym zostać weterynarzem!";
        let delta = build_redacted_delta(before, after).expect("should produce delta");

        let mut target = before.to_string();
        apply_delta_to_string(&mut target, &delta);
        assert_eq!(target, after);
    }

    #[test]
    fn test_correction_stale_guard_detects_preview_rev_drift() {
        assert!(correction_is_stale(7, 8, "draft", "draft"));
        assert!(!correction_is_stale(7, 7, "draft", "draft"));
    }

    #[test]
    fn test_correction_stale_guard_detects_text_drift() {
        assert!(correction_is_stale(9, 9, "ala ma", "ala ma kota"));
    }

    #[test]
    fn test_partial_telemetry_counters_accumulate() {
        let mut telemetry = PartialPassTelemetry::default();
        telemetry.record_run(PartialPassTrigger::Utterance);
        telemetry.record_run(PartialPassTrigger::Speech);
        telemetry.record_run(PartialPassTrigger::Watchdog);
        telemetry.record_stale();
        telemetry.record_coalesced();
        telemetry.record_dropped();

        assert_eq!(telemetry.runs_total, 3);
        assert_eq!(telemetry.trigger_utterance_count, 1);
        assert_eq!(telemetry.trigger_speech_count, 1);
        assert_eq!(telemetry.trigger_watchdog_count, 1);
        assert_eq!(telemetry.stale_count, 1);
        assert_eq!(telemetry.coalesced_count, 1);
        assert_eq!(telemetry.dropped_count, 1);
    }

    #[tokio::test]
    async fn test_schedule_partial_pass_coalesces_under_async_scheduler_pressure() {
        let started = Arc::new(StdMutex::new(Vec::<u32>::new()));
        let gate = Arc::new((StdMutex::new(false), Condvar::new()));
        let started_ref = Arc::clone(&started);
        let gate_ref = Arc::clone(&gate);

        let infer = Arc::new(
            move |samples: Vec<f32>,
                  _sample_rate: u32,
                  _language: Option<String>|
                  -> Result<RawTranscript> {
                let id = samples.first().copied().unwrap_or_default() as u32;
                started_ref
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(id);
                if id == 100 {
                    let (lock, cvar) = &*gate_ref;
                    let mut released = lock.lock().unwrap_or_else(|e| e.into_inner());
                    while !*released {
                        released = cvar.wait(released).unwrap_or_else(|e| e.into_inner());
                    }
                }
                Ok(RawTranscript {
                    text: format!("job-{id}"),
                    segments: Vec::new(),
                })
            },
        );

        let scheduler = SttScheduler::with_infer_fn(infer);
        let mut blocker = scheduler
            .submit(SttLane::Live, vec![100.0], 16_000, None)
            .expect("submit blocking live request");

        let collector = Arc::new(CollectorEventSink::new());
        let event_sink: Arc<dyn EventSink> = collector.clone();
        let mut correction_in_flight: Option<SttTaskHandle> = None;
        let mut correction_expected_preview_rev: Option<u64> = None;
        let mut correction_expected_text: Option<String> = None;
        let mut correction_suffix_snapshot: Option<String> = None;
        let mut partial_telemetry = PartialPassTelemetry::default();

        let mut first_audio = vec![21.0];
        assert!(schedule_partial_pass(
            &scheduler,
            16_000,
            Some("en".to_string()),
            &mut first_audio,
            &mut correction_in_flight,
            &mut correction_expected_preview_rev,
            &mut correction_expected_text,
            &mut correction_suffix_snapshot,
            "suffix-a",
            7,
            "draft-a",
            PARTIAL_PASS_TRIGGER_SILERO_SPEECH_MS,
            PartialPassTrigger::Watchdog,
            &mut partial_telemetry,
            &event_sink,
        ));
        assert!(
            first_audio.is_empty(),
            "correction audio buffer should be consumed on schedule"
        );
        assert_eq!(
            correction_expected_preview_rev,
            Some(7),
            "tracked preview revision should match first scheduled correction"
        );
        assert_eq!(
            correction_expected_text.as_deref(),
            Some("draft-a"),
            "tracked expected text should match first scheduled correction"
        );
        assert_eq!(
            correction_suffix_snapshot.as_deref(),
            Some("suffix-a"),
            "tracked suffix snapshot should match first scheduled correction"
        );
        let first_id = correction_in_flight
            .as_ref()
            .expect("first correction handle should be tracked")
            .id();

        let mut second_audio = vec![22.0];
        assert!(schedule_partial_pass(
            &scheduler,
            16_000,
            Some("en".to_string()),
            &mut second_audio,
            &mut correction_in_flight,
            &mut correction_expected_preview_rev,
            &mut correction_expected_text,
            &mut correction_suffix_snapshot,
            "suffix-b",
            8,
            "draft-b",
            PARTIAL_PASS_TRIGGER_SILERO_SPEECH_MS,
            PartialPassTrigger::Speech,
            &mut partial_telemetry,
            &event_sink,
        ));
        let second_id = correction_in_flight
            .as_ref()
            .expect("latest correction handle should replace old in-flight handle")
            .id();
        assert!(
            second_id > first_id,
            "newly scheduled correction should replace the previous tracked handle"
        );
        assert_eq!(partial_telemetry.runs_total, 2);
        assert_eq!(partial_telemetry.trigger_watchdog_count, 1);
        assert_eq!(partial_telemetry.trigger_speech_count, 1);
        assert_eq!(partial_telemetry.trigger_utterance_count, 0);
        assert_eq!(partial_telemetry.coalesced_count, 1);
        assert_eq!(partial_telemetry.stale_count, 0);
        assert_eq!(partial_telemetry.dropped_count, 0);
        assert_eq!(
            correction_expected_preview_rev,
            Some(8),
            "new schedule should overwrite tracked preview revision"
        );
        assert_eq!(
            correction_expected_text.as_deref(),
            Some("draft-b"),
            "new schedule should overwrite tracked expected text"
        );
        assert_eq!(
            correction_suffix_snapshot.as_deref(),
            Some("suffix-b"),
            "new schedule should overwrite tracked suffix snapshot"
        );

        {
            let (lock, cvar) = &*gate;
            let mut released = lock.lock().unwrap_or_else(|e| e.into_inner());
            *released = true;
            cvar.notify_all();
        }

        let blocking_result = tokio::time::timeout(Duration::from_secs(2), blocker.recv())
            .await
            .expect("blocking live request timed out")
            .expect("blocking live request should finish");
        assert_eq!(blocking_result.text, "job-100");
        assert!(blocking_result.segments.is_empty());

        let mut correction_handle = correction_in_flight
            .take()
            .expect("latest correction handle should remain in-flight");
        let correction_result =
            tokio::time::timeout(Duration::from_secs(2), correction_handle.recv())
                .await
                .expect("latest correction request timed out")
                .expect("latest correction request should complete");
        assert_eq!(correction_result.text, "job-22");
        assert!(correction_result.segments.is_empty());

        tokio::time::timeout(Duration::from_secs(2), scheduler.shutdown())
            .await
            .expect("scheduler shutdown timed out")
            .expect("scheduler shutdown");

        assert_eq!(
            started.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            vec![100, 22],
            "superseded correction should not execute when scheduler is saturated"
        );
        assert!(
            collector
                .events()
                .iter()
                .all(|event| !matches!(event, EngineEvent::Warning { .. })),
            "successful partial scheduling should not emit warning events"
        );
    }

    #[tokio::test]
    async fn test_schedule_partial_pass_repeated_coalescing_under_async_pressure() {
        let started = Arc::new(StdMutex::new(Vec::<u32>::new()));
        let gate = Arc::new((StdMutex::new(false), Condvar::new()));
        let started_ref = Arc::clone(&started);
        let gate_ref = Arc::clone(&gate);

        let infer = Arc::new(
            move |samples: Vec<f32>,
                  _sample_rate: u32,
                  _language: Option<String>|
                  -> Result<RawTranscript> {
                let id = samples.first().copied().unwrap_or_default() as u32;
                started_ref
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(id);
                if id == 100 {
                    let (lock, cvar) = &*gate_ref;
                    let mut released = lock.lock().unwrap_or_else(|e| e.into_inner());
                    while !*released {
                        released = cvar.wait(released).unwrap_or_else(|e| e.into_inner());
                    }
                }
                Ok(RawTranscript {
                    text: format!("job-{id}"),
                    segments: Vec::new(),
                })
            },
        );

        let scheduler = SttScheduler::with_infer_fn(infer);
        let mut blocker = scheduler
            .submit(SttLane::Live, vec![100.0], 16_000, None)
            .expect("submit blocking live request");

        let collector = Arc::new(CollectorEventSink::new());
        let event_sink: Arc<dyn EventSink> = collector.clone();
        let mut correction_in_flight: Option<SttTaskHandle> = None;
        let mut correction_expected_preview_rev: Option<u64> = None;
        let mut correction_expected_text: Option<String> = None;
        let mut correction_suffix_snapshot: Option<String> = None;
        let mut partial_telemetry = PartialPassTelemetry::default();
        let trigger_sequence = [
            PartialPassTrigger::Utterance,
            PartialPassTrigger::Speech,
            PartialPassTrigger::Watchdog,
            PartialPassTrigger::Speech,
            PartialPassTrigger::Watchdog,
        ];
        let first_marker = 31u32;
        let expected_last_id = first_marker + trigger_sequence.len() as u32 - 1;

        for (index, trigger) in trigger_sequence.iter().copied().enumerate() {
            let marker = 31.0 + index as f32;
            let expected_rev = 21 + index as u64;
            let expected_text = format!("draft-{index}");
            let expected_suffix = format!("suffix-{index}");
            let mut audio = vec![marker];

            assert!(schedule_partial_pass(
                &scheduler,
                16_000,
                Some("en".to_string()),
                &mut audio,
                &mut correction_in_flight,
                &mut correction_expected_preview_rev,
                &mut correction_expected_text,
                &mut correction_suffix_snapshot,
                &expected_suffix,
                expected_rev,
                &expected_text,
                PARTIAL_PASS_TRIGGER_SILERO_SPEECH_MS + index as u64,
                trigger,
                &mut partial_telemetry,
                &event_sink,
            ));
            assert!(
                audio.is_empty(),
                "schedule should consume correction audio buffer"
            );
            assert_eq!(correction_expected_preview_rev, Some(expected_rev));
            assert_eq!(
                correction_expected_text.as_deref(),
                Some(expected_text.as_str())
            );
            assert_eq!(
                correction_suffix_snapshot.as_deref(),
                Some(expected_suffix.as_str())
            );
        }

        {
            let (lock, cvar) = &*gate;
            let mut released = lock.lock().unwrap_or_else(|e| e.into_inner());
            *released = true;
            cvar.notify_all();
        }

        let blocking_result = tokio::time::timeout(Duration::from_secs(2), blocker.recv())
            .await
            .expect("blocking live request timed out")
            .expect("blocking live request should finish");
        assert_eq!(blocking_result.text, "job-100");

        let mut correction_handle = correction_in_flight
            .take()
            .expect("latest correction handle should remain in-flight");
        let correction_result =
            tokio::time::timeout(Duration::from_secs(2), correction_handle.recv())
                .await
                .expect("latest correction request timed out")
                .expect("latest correction request should complete");
        assert_eq!(correction_result.text, format!("job-{expected_last_id}"));
        assert!(correction_result.segments.is_empty());

        tokio::time::timeout(Duration::from_secs(2), scheduler.shutdown())
            .await
            .expect("scheduler shutdown timed out")
            .expect("scheduler shutdown");

        assert_eq!(
            started.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            vec![100, expected_last_id],
            "coalescing under pressure should execute only the latest correction"
        );
        assert_eq!(partial_telemetry.runs_total, 5);
        assert_eq!(partial_telemetry.trigger_utterance_count, 1);
        assert_eq!(partial_telemetry.trigger_speech_count, 2);
        assert_eq!(partial_telemetry.trigger_watchdog_count, 2);
        assert_eq!(partial_telemetry.coalesced_count, 4);
        assert_eq!(partial_telemetry.stale_count, 0);
        assert_eq!(partial_telemetry.dropped_count, 0);
        assert!(
            collector
                .events()
                .iter()
                .all(|event| !matches!(event, EngineEvent::Warning { .. })),
            "successful coalescing should not emit warnings"
        );
    }

    #[tokio::test]
    async fn transcription_session_emits_no_speech_and_stats_for_empty_input() {
        let (tx, rx) = mpsc::channel::<Vec<f32>>(1);
        drop(tx);
        let sink = Arc::new(CollectorEventSink::new());
        transcription_session(
            rx,
            sink.clone(),
            SessionConfig {
                sample_rate: 16_000,
                language: Some("pl".to_string()),
                stream_log_path: None,
                utterance_silence_sec: None,
            },
        )
        .await;

        let events = sink.events();

        let no_speech_pos = events
            .iter()
            .position(|event| matches!(event, EngineEvent::NoSpeech { .. }))
            .expect("session should emit NoSpeech for empty input");
        let stats_pos = events
            .iter()
            .position(|event| matches!(event, EngineEvent::Stats { .. }))
            .expect("session should emit Stats for empty input");
        assert!(
            no_speech_pos < stats_pos,
            "NoSpeech should be emitted before final Stats"
        );

        let mut no_speech_reason = None;
        let mut stats_count = 0u32;
        for event in &events {
            match event {
                EngineEvent::NoSpeech { reason } => {
                    no_speech_reason = Some(reason.clone());
                }
                EngineEvent::Stats {
                    dropped_audio_chunks,
                    hallucination_drops,
                    semantic_gate_drops,
                    filtered_empty_drops,
                    corrections_applied,
                    total_utterances,
                    partial_runs_total,
                    trigger_utterance_count,
                    trigger_speech_count,
                    trigger_watchdog_count,
                    partial_stale_count,
                    partial_coalesced_count,
                    partial_dropped_count,
                } => {
                    stats_count += 1;
                    assert_eq!(*dropped_audio_chunks, 0);
                    assert_eq!(*hallucination_drops, 0);
                    assert_eq!(*semantic_gate_drops, 0);
                    assert_eq!(*filtered_empty_drops, 0);
                    assert_eq!(*corrections_applied, 0);
                    assert_eq!(*total_utterances, 0);
                    assert_eq!(*partial_runs_total, 0);
                    assert_eq!(*trigger_utterance_count, 0);
                    assert_eq!(*trigger_speech_count, 0);
                    assert_eq!(*trigger_watchdog_count, 0);
                    assert_eq!(*partial_stale_count, 0);
                    assert_eq!(*partial_coalesced_count, 0);
                    assert_eq!(*partial_dropped_count, 0);
                }
                _ => {}
            }
        }

        assert_eq!(
            no_speech_reason.as_deref(),
            Some("vad_no_speech_detected"),
            "empty session should report VAD no-speech reason"
        );
        assert_eq!(stats_count, 1, "expected exactly one Stats event");
    }

    #[tokio::test]
    async fn transcription_session_silent_callbacks_keep_no_speech_stats_coherent() {
        let (tx, rx) = mpsc::channel::<Vec<f32>>(1);
        let sender = tokio::spawn(async move {
            for i in 0..96usize {
                let len = if i % 2 == 0 { 371 } else { 1024 };
                tx.send(vec![0.0; len])
                    .await
                    .expect("silent callback send should succeed");
                tokio::task::yield_now().await;
            }
        });

        let sink = Arc::new(CollectorEventSink::new());
        transcription_session(
            rx,
            sink.clone(),
            SessionConfig {
                sample_rate: 48_000,
                language: Some("pl".to_string()),
                stream_log_path: None,
                utterance_silence_sec: None,
            },
        )
        .await;
        sender
            .await
            .expect("silent callback sender task should finish");

        let events = sink.events();
        let no_speech_pos = events
            .iter()
            .position(|event| matches!(event, EngineEvent::NoSpeech { .. }))
            .expect("session should emit NoSpeech for silence-only callbacks");
        let stats_pos = events
            .iter()
            .position(|event| matches!(event, EngineEvent::Stats { .. }))
            .expect("session should emit Stats for silence-only callbacks");
        assert!(
            no_speech_pos < stats_pos,
            "NoSpeech should be emitted before final Stats"
        );

        let mut no_speech_count = 0u32;
        let mut stats_count = 0u32;
        for event in &events {
            match event {
                EngineEvent::NoSpeech { reason } => {
                    no_speech_count = no_speech_count.saturating_add(1);
                    assert_eq!(reason, "vad_no_speech_detected");
                }
                EngineEvent::Stats {
                    dropped_audio_chunks,
                    hallucination_drops,
                    semantic_gate_drops,
                    filtered_empty_drops,
                    corrections_applied,
                    total_utterances,
                    partial_runs_total,
                    trigger_utterance_count,
                    trigger_speech_count,
                    trigger_watchdog_count,
                    partial_stale_count,
                    partial_coalesced_count,
                    partial_dropped_count,
                } => {
                    stats_count = stats_count.saturating_add(1);
                    assert_eq!(*dropped_audio_chunks, 0);
                    assert_eq!(*hallucination_drops, 0);
                    assert_eq!(*semantic_gate_drops, 0);
                    assert_eq!(*filtered_empty_drops, 0);
                    assert_eq!(*corrections_applied, 0);
                    assert_eq!(*total_utterances, 0);
                    assert_eq!(*partial_runs_total, 0);
                    assert_eq!(*trigger_utterance_count, 0);
                    assert_eq!(*trigger_speech_count, 0);
                    assert_eq!(*trigger_watchdog_count, 0);
                    assert_eq!(*partial_stale_count, 0);
                    assert_eq!(*partial_coalesced_count, 0);
                    assert_eq!(*partial_dropped_count, 0);
                }
                _ => {}
            }
        }

        assert_eq!(no_speech_count, 1, "expected exactly one NoSpeech event");
        assert_eq!(stats_count, 1, "expected exactly one Stats event");
    }

    #[test]
    fn test_postprocess_correction_with_snapshot_restores_suffix_on_drop() {
        let mut pipeline = TranscriptionPipeline::new(None);
        pipeline.last_suffix = "current-tail".to_string();

        let result =
            postprocess_correction_with_snapshot(&mut pipeline, "Thank you", "snapshot-tail");
        assert!(matches!(result, Err(PostprocessDrop::Hallucination)));
        assert_eq!(pipeline.last_suffix, "current-tail");
    }

    #[test]
    fn test_postprocess_correction_with_snapshot_updates_suffix_on_success() {
        let mut pipeline = TranscriptionPipeline::new(None);
        pipeline.last_suffix = "old-tail".to_string();

        let corrected = postprocess_correction_with_snapshot(
            &mut pipeline,
            "to jest poprawny tekst",
            "snapshot-tail",
        )
        .expect("correction should pass");
        assert!(!corrected.is_empty());
        assert_ne!(pipeline.last_suffix, "old-tail");
    }

    #[test]
    fn test_correction_postprocess_remains_text_centric_with_timestamp_state() {
        let mut pipeline = TranscriptionPipeline::new(None);
        pipeline.last_suffix = "alpha beta".to_string();
        pipeline.last_segment_end_ts = Some(42.0);

        let corrected =
            postprocess_correction_with_snapshot(&mut pipeline, "beta gamma", "alpha beta")
                .expect("text-based correction path should remain active");
        assert_eq!(corrected, "gamma");
        assert_eq!(
            pipeline.last_segment_end_ts,
            Some(42.0),
            "correction flow should not depend on or mutate timestamp overlap state"
        );
    }

    // ── Fix A contract: FINAL must not inherit corrupted suffix from non-final ──

    #[test]
    fn test_fix_a_final_uses_boundary_suffix_not_nonfinal_suffix() {
        // Simulate: utterance boundary leaves suffix "abc". Then non-final
        // chunks advance pipeline.last_suffix to "xyz". When FINAL arrives,
        // it should see "abc" (the boundary snapshot), not "xyz".
        let mut pipeline = TranscriptionPipeline::new(None);

        // Initial utterance boundary — suffix is empty (session start).
        let utterance_boundary_suffix = pipeline.last_suffix.clone();
        assert_eq!(utterance_boundary_suffix, "");

        // Non-final chunk processing advances pipeline.last_suffix.
        let _ = pipeline.postprocess_with_reason("hello world");
        assert_ne!(
            pipeline.last_suffix, utterance_boundary_suffix,
            "non-final should advance last_suffix"
        );
        let corrupted_suffix = pipeline.last_suffix.clone();

        // Fix A: Restore boundary suffix before FINAL processing.
        pipeline.last_suffix = utterance_boundary_suffix.clone();
        let result = pipeline.postprocess_with_reason("hello world final version");
        assert!(result.is_ok());

        // Verify FINAL did NOT use the corrupted non-final suffix.
        // The boundary suffix was empty, so no overlap should be stripped.
        let cleaned = result.unwrap();
        assert!(
            cleaned.contains("hello"),
            "FINAL with restored boundary suffix should not aggressively strip: got '{}'",
            cleaned
        );

        // Without Fix A, pipeline.last_suffix would have been "corrupted_suffix"
        // causing strip_overlap to incorrectly remove matching text.
        assert_ne!(
            pipeline.last_suffix, corrupted_suffix,
            "after Fix A, pipeline.last_suffix should be updated from FINAL, not stuck on non-final's suffix"
        );
    }

    // ── Fix D contract: window-scoped stale guard survives utterance boundaries ──

    #[test]
    fn test_fix_d_stale_guard_with_window_rev_survives_final() {
        // Before Fix D: schedule_partial_pass stored preview_rev / accumulated_text.
        // After FINAL: accumulated_text.clear() → correction_is_stale could pass
        // when it shouldn't (empty == empty).
        //
        // After Fix D: schedule_partial_pass stores window_rev / window_text.
        // FINAL increments window_rev → correction_is_stale correctly detects staleness.

        let window_rev_at_schedule: u64 = 5;
        let window_text_at_schedule = "cześć jak się masz";

        // Simulate FINAL boundary advancing window state.
        let window_rev_after_final: u64 = 6; // FINAL incremented it
        let window_text_after_final = "cześć jak się masz dobrze";

        assert!(
            correction_is_stale(
                window_rev_at_schedule,
                window_rev_after_final,
                window_text_at_schedule,
                window_text_after_final,
            ),
            "correction scheduled before FINAL should be stale after FINAL"
        );
    }

    #[test]
    fn test_fix_d_stale_guard_passes_when_window_unchanged() {
        // When no FINAL or new text arrives between schedule and correction result,
        // the window state matches and correction should apply.
        let window_rev: u64 = 5;
        let window_text = "cześć jak się masz";

        assert!(
            !correction_is_stale(window_rev, window_rev, window_text, window_text),
            "correction should not be stale when window state unchanged"
        );
    }

    #[test]
    fn test_fix_d_empty_accumulated_text_after_final_detected_by_window_rev() {
        // Edge case: FINAL clears accumulated_text. Before Fix D, stale guard
        // compared "" vs "" → not stale → correction applies to empty text.
        // After Fix D, window_rev incremented by FINAL → stale.

        // Old behavior (broken): accumulated_text scope — expected "hello world"
        // vs current "" (cleared by FINAL). This would pass if revs matched
        // (both based on preview_rev which didn't increment for FINAL).

        // New behavior: window scope
        let window_rev_at_schedule: u64 = 3;
        let window_text_at_schedule = "hello world";
        let window_rev_after_final: u64 = 4; // FINAL bumped it
        let window_text_after_final = "hello world and more"; // FINAL appended

        assert!(
            correction_is_stale(
                window_rev_at_schedule,
                window_rev_after_final,
                window_text_at_schedule,
                window_text_after_final,
            ),
            "window-scoped stale guard must detect FINAL boundary crossing"
        );
    }
}
