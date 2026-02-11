//! Streaming transcription pipeline — orchestration, buffered emission, and policy.
//!
//! Extracted from `audio::streaming_recorder` to decouple pipeline logic
//! (hallucination filtering, overlap dedup, re-transcription, buffered "typing"
//! emission) from the audio capture layer.
//!
//! Created by M&K (c)2026 VetCoders

use crate::audio::chunker::{SpeechEvent, SpeechSession};
use crate::pipeline::dedup::{dedup_chunk_overlap, strip_suffix_overlap};
use crate::pipeline::stream_postprocess::StreamPostProcessor;
use crate::stt::whisper::singleton::engine as get_engine;
use anyhow::{Result, anyhow};
use chrono::SecondsFormat;
use lazy_static::lazy_static;
use regex::Regex;
use std::collections::VecDeque;
use std::sync::Arc;
use std::{fs::OpenOptions, io::Write, path::Path};
use tokio::sync::{Mutex, mpsc};
use tokio::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

// ── Constants ────────────────────────────────────────────────────────────────

const DEFAULT_CHUNK_DURATION_SEC: f32 = 3.0;
const DEFAULT_OVERLAP_RATIO: f32 = 0.2; // 20% overlap for context
const DEFAULT_BUFFER_DELAY_MS: u64 = 3000;
const DEFAULT_TYPING_CPS: f32 = 30.0;
const DEFAULT_EMIT_WORDS_MAX: usize = 3;

lazy_static! {
    static ref TOKEN_RE: Regex = Regex::new(r"\s+|\S+\s*").expect("token regex");
}

// ── Public type alias ────────────────────────────────────────────────────────

use crate::pipeline::contracts::{DeltaSink, DropKind, EngineEvent, EventSink, TranscriptDelta};

/// Legacy alias — now backed by `DeltaSink` trait instead of bare `Fn(&str)`.
/// Consumers should migrate to `Arc<dyn DeltaSink>` directly.
#[deprecated(note = "Use Arc<dyn DeltaSink> directly")]
pub type StreamDeltaCallback = Arc<dyn DeltaSink>;

// ── Buffered worker parameters ───────────────────────────────────────────────

/// Groups optional configuration for [`buffered_transcription_worker`]
/// so the function signature stays under clippy's argument limit.
pub(crate) struct BufferedWorkerConfig {
    pub sample_rate: u32,
    pub language: Option<String>,
    pub delta_callback: Option<Arc<dyn DeltaSink>>,
    pub utterance_callback: Option<Arc<dyn Fn(String) + Send + Sync>>,
    pub utterance_silence_sec: Option<f32>,
    pub vad_start_callback: Option<Arc<dyn Fn() + Send + Sync>>,
    pub stream_log_path: Option<std::path::PathBuf>,
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

// ── Hallucination filter ─────────────────────────────────────────────────────

const WHISPER_HALLUCINATIONS: &[&str] = &[
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

const SHORT_SPEECH_WHITELIST: &[&str] = &[
    "tak", "nie", "co?", "co", "dobra", "dobrze", "ok", "okej", "no", "no?", "mhm", "aha", "jasne",
    "pewnie", "super", "hej", "halo", "cześć", "siema", "dzięki", "proszę",
];

pub(crate) fn is_hallucination(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    if SHORT_SPEECH_WHITELIST.iter().any(|w| lower == *w) {
        return false;
    }
    if WHISPER_HALLUCINATIONS.iter().any(|h| lower == *h) {
        return true;
    }
    if lower.len() < 30
        && WHISPER_HALLUCINATIONS.iter().any(|h| lower.contains(h))
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
    pub(crate) hallucination_drops: u64,
    pub(crate) overlap_strips: u64,
}

/// Reason a postprocess step dropped content.
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
            hallucination_drops: 0,
            overlap_strips: 0,
        }
    }

    pub(crate) fn strip_overlap(&mut self, text: &str) -> String {
        strip_suffix_overlap(&self.last_suffix, text)
    }

    pub(crate) fn postprocess(&mut self, text: &str) -> Option<String> {
        if is_hallucination(text) {
            self.hallucination_drops += 1;
            return None;
        }

        let stripped = self.strip_overlap(text);
        if stripped.is_empty() {
            self.overlap_strips += 1;
            return None;
        }

        // Utterance path must not apply the semantic gate; utterances are
        // VAD-bounded by definition and should not be dropped for "novelty".
        let processed = self.postprocessor.process_utterance(&stripped)?;

        self.update_suffix(&processed);
        Some(processed)
    }

    /// Like `postprocess`, but returns the drop reason on failure.
    pub(crate) fn postprocess_with_reason(
        &mut self,
        text: &str,
    ) -> Result<String, PostprocessDrop> {
        if is_hallucination(text) {
            self.hallucination_drops += 1;
            return Err(PostprocessDrop::Hallucination);
        }

        let stripped = self.strip_overlap(text);
        if stripped.is_empty() {
            self.overlap_strips += 1;
            return Err(PostprocessDrop::OverlapEmpty);
        }

        match self.postprocessor.process_utterance(&stripped) {
            Some(processed) => {
                self.update_suffix(&processed);
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

// ── BufferedEmitter ──────────────────────────────────────────────────────────

/// Typing-animation emitter for transcript segments.
///
/// Buffers incoming text and emits it character-by-character at a configurable
/// typing speed via `DeltaSink`. Used by the deprecated `buffered_transcription_worker`
/// and by `app::presentation::PresentationEmitter`.
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

        const CORRECTION_COOLDOWN_MS: u64 = 500;
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

/// Unified transcription session — replaces both `transcription_worker` and
/// `buffered_transcription_worker` with a single event-emitting pipeline.
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

    let correction_min_utterances = buffered_correction_min_utterances();
    let correction_min_sec = buffered_correction_min_sec();

    let mut session = if let Some(sec) = utterance_silence_sec {
        SpeechSession::new_utterance_with_silence(sample_rate, sec)
    } else {
        SpeechSession::new_utterance(sample_rate)
    };
    let output_sample_rate = session.output_sample_rate();

    let mut pipeline = TranscriptionPipeline::new(language);
    let mut preview_rev: u64 = 0;
    let mut utterance_id: u64 = 0;
    let mut total_utterances: u64 = 0;
    let semantic_gate_drops: u64 = 0;
    let mut filtered_empty_drops: u64 = 0;
    let mut corrections_applied: u64 = 0;
    let mut vad_started = false;

    // Accumulate text for the current "run" of utterances (between corrections).
    let mut accumulated_text = String::new();
    // Track last raw Whisper output for final flush UtteranceFinal.
    let mut last_raw_text = String::new();

    // Track audio position for UtteranceFinal timestamps (seconds).
    let mut utterance_start_s: f32 = 0.0;
    let mut utterance_audio_samples: usize = 0;

    // Phase 2 correction state
    let mut correction_audio_buf: Vec<f32> = Vec::new();
    let mut utterance_count: usize = 0;
    let mut suffix_snapshot = String::new();

    // Decouple audio ingestion from Whisper inference.
    const MAX_PENDING_UTTERANCES: usize = 64;
    let mut pending_utterances: VecDeque<UtteranceWorkItem> = VecDeque::new();
    let mut dropped_utterances: u64 = 0;
    let mut audio_closed = false;

    // Phase 1 (streaming preview) — one utterance transcription in flight.
    let mut utterance_in_flight: Option<tokio::task::JoinHandle<Result<String>>> = None;
    let mut utterance_active: Option<UtteranceWorkItem> = None;

    // Phase 2 (buffered correction) — re-transcription in flight.
    let mut correction_in_flight: Option<tokio::task::JoinHandle<Result<String>>> = None;
    let mut correction_current_suffix: Option<String> = None;

    loop {
        // Start next utterance transcription if possible.
        if utterance_in_flight.is_none()
            && correction_in_flight.is_none()
            && let Some(item) = pending_utterances.pop_front()
        {
            let lang = pipeline.language.clone();
            let handle =
                spawn_utterance_transcription(item.audio.clone(), output_sample_rate, lang);
            utterance_in_flight = Some(handle);
            utterance_active = Some(item);
        }

        // If audio is closed and there is no work left, finish.
        if audio_closed
            && pending_utterances.is_empty()
            && utterance_in_flight.is_none()
            && correction_in_flight.is_none()
        {
            break;
        }

        tokio::select! {
            maybe_data = chunk_receiver.recv(), if !audio_closed => {
                match maybe_data {
                    Some(data) => {
                        for event in session.feed(&data, sample_rate) {
                            let (utterance, is_final) = match event {
                                SpeechEvent::Utterance(u) => (u, false),
                                SpeechEvent::UtteranceFinal(u) => (u, true),
                                _ => continue,
                            };

                            if !vad_started {
                                event_sink.on_event(&EngineEvent::VadStart {
                                    speech_prob: session.boundary_prob(),
                                    ts_ms: session.session_elapsed_ms(),
                                });
                                vad_started = true;
                            }

                            if pending_utterances.len() >= MAX_PENDING_UTTERANCES {
                                dropped_utterances = dropped_utterances.saturating_add(1);
                                continue;
                            }

                            pending_utterances.push_back(UtteranceWorkItem {
                                audio: utterance,
                                is_final,
                            });
                        }
                    }
                    None => {
                        audio_closed = true;
                        if let Some(event) = session.flush() {
                            // Emit VadFallback if flush used degraded path (VAD never fired Start).
                            if session.was_flush_fallback() {
                                event_sink.on_event(&EngineEvent::VadFallback {
                                    max_prob: session.peak_speech_prob(),
                                    samples: match &event {
                                        SpeechEvent::UtteranceFinal(u)
                                        | SpeechEvent::Utterance(u) => u.len(),
                                        _ => 0,
                                    },
                                });
                            }

                            let (utterance, is_final) = match event {
                                SpeechEvent::Utterance(u) => (u, false),
                                SpeechEvent::UtteranceFinal(u) => (u, true),
                                _ => (Vec::new(), false),
                            };

                            if !utterance.is_empty() {
                                // Emit VadStart if this is the first speech (e.g. from flush).
                                if !vad_started {
                                    event_sink.on_event(&EngineEvent::VadStart {
                                        speech_prob: session.boundary_prob(),
                                        ts_ms: session.session_elapsed_ms(),
                                    });
                                    vad_started = true;
                                }
                                if pending_utterances.len() < MAX_PENDING_UTTERANCES {
                                    pending_utterances.push_back(UtteranceWorkItem { audio: utterance, is_final });
                                } else {
                                    dropped_utterances = dropped_utterances.saturating_add(1);
                                }
                            }
                        }
                    }
                }
            }
            result = async {
                correction_in_flight.as_mut().unwrap().await
            }, if correction_in_flight.is_some() => {
                let current_suffix = correction_current_suffix.take().unwrap_or_default();
                match result {
                    Ok(Ok(raw)) => {
                        // Suppress stale corrections that arrive after UtteranceFinal
                        // already cleared accumulated_text. Without this guard, the
                        // corrected text would appear as phantom content in the next
                        // utterance window.
                        if accumulated_text.is_empty() {
                            debug!("Suppressing stale correction (utterance already finalized)");
                            if !current_suffix.is_empty() {
                                pipeline.last_suffix = current_suffix;
                            }
                        } else if let Some(cleaned) = pipeline.postprocess(&raw) {
                            let previous_text = accumulated_text.clone();
                            preview_rev += 1;
                            corrections_applied += 1;
                            event_sink.on_event(&EngineEvent::Correction {
                                rev: preview_rev,
                                text: cleaned.clone(),
                                previous_text,
                            });
                            // Update accumulated_text so next Preview builds on corrected state.
                            accumulated_text = cleaned;
                        } else if !current_suffix.is_empty() {
                            pipeline.last_suffix = current_suffix;
                        }
                    }
                    _ => {
                        warn!("Re-transcription failed; keeping Phase 1 draft");
                        if !current_suffix.is_empty() {
                            pipeline.last_suffix = current_suffix;
                        }
                    }
                }

                utterance_count = 0;
                correction_in_flight = None;
            }
            result = async {
                utterance_in_flight.as_mut().unwrap().await
            }, if utterance_in_flight.is_some() => {
                let item = utterance_active.take().unwrap_or_else(|| UtteranceWorkItem { audio: Vec::new(), is_final: false });
                // Track audio duration for timestamp computation.
                utterance_audio_samples += item.audio.len();

                match result {
                    Ok(Ok(raw_text)) => {
                        last_raw_text = raw_text.clone();
                        if utterance_count == 0 && correction_audio_buf.is_empty() {
                            suffix_snapshot = pipeline.last_suffix.clone();
                        }

                        match pipeline.postprocess_with_reason(&raw_text) {
                            Ok(cleaned) => {
                                preview_rev += 1;
                                if !accumulated_text.is_empty() {
                                    accumulated_text.push(' ');
                                }
                                accumulated_text.push_str(cleaned.trim());

                                event_sink.on_event(&EngineEvent::Preview {
                                    rev: preview_rev,
                                    text: accumulated_text.clone(),
                                });

                                if let Some(path) = stream_log_path.as_deref() {
                                    let _ = append_to_stream_log(path, cleaned.trim());
                                }
                            }
                            Err(PostprocessDrop::Hallucination) => {
                                event_sink.on_event(&EngineEvent::Drop {
                                    kind: DropKind::Hallucination,
                                    text: raw_text.clone(),
                                    reason: format!("Hallucination pattern: '{}'", raw_text.trim()),
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

                        if item.is_final {
                            utterance_id += 1;
                            total_utterances += 1;
                            let final_text = accumulated_text.trim().to_string();
                            let end_ts = utterance_start_s
                                + utterance_audio_samples as f32 / output_sample_rate as f32;
                            let had_content = !final_text.is_empty();
                            if had_content {
                                event_sink.on_event(&EngineEvent::UtteranceFinal {
                                    utterance_id,
                                    text: final_text,
                                    raw_text: raw_text.clone(),
                                    start_ts: utterance_start_s,
                                    end_ts,
                                });
                            }
                            accumulated_text.clear();
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

                            // Reset Phase 2 correction state on utterance boundary.
                            // Any in-flight correction will be suppressed by the
                            // accumulated_text.is_empty() guard in the correction handler.
                            correction_audio_buf.clear();
                            utterance_count = 0;
                        } else {
                            // Phase 2 correction accumulation — only for non-final items.
                            // Spawning correction on a final item would produce a stale
                            // Correction event after UtteranceFinal has already fired.
                            correction_audio_buf.extend_from_slice(&item.audio);
                            utterance_count += 1;

                            let audio_duration_s =
                                correction_audio_buf.len() as f32 / output_sample_rate as f32;
                            if utterance_count >= correction_min_utterances || audio_duration_s >= correction_min_sec {
                                let audio = std::mem::take(&mut correction_audio_buf);
                                let lang = pipeline.language.clone();

                                let current_suffix = pipeline.last_suffix.clone();
                                pipeline.last_suffix = suffix_snapshot.clone();
                                correction_current_suffix = Some(current_suffix);

                                // Abort stale correction task to prevent task leak + suffix corruption.
                                if let Some(old) = correction_in_flight.take() {
                                    old.abort();
                                }
                                correction_in_flight = Some(spawn_utterance_transcription(
                                    audio,
                                    output_sample_rate,
                                    lang,
                                ));
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        error!("Transcription failed: {}", e);
                        event_sink.on_event(&EngineEvent::Warning {
                            code: "transcription_error".to_string(),
                            message: format!("{}", e),
                        });
                    }
                    Err(e) => {
                        error!("Transcription task join error: {}", e);
                        event_sink.on_event(&EngineEvent::Warning {
                            code: "task_join_error".to_string(),
                            message: format!("{}", e),
                        });
                    }
                }

                utterance_in_flight = None;
            }
        }
    }

    // Emit any remaining accumulated text as final utterance.
    let remaining = accumulated_text.trim().to_string();
    if !remaining.is_empty() {
        utterance_id += 1;
        total_utterances += 1;
        let end_ts = utterance_start_s + utterance_audio_samples as f32 / output_sample_rate as f32;
        event_sink.on_event(&EngineEvent::UtteranceFinal {
            utterance_id,
            text: remaining,
            raw_text: last_raw_text,
            start_ts: utterance_start_s,
            end_ts,
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
    });

    if dropped_utterances > 0 {
        warn!(
            "Session dropped {} utterance(s) due to backpressure",
            dropped_utterances
        );
    }

    info!(
        "Transcription session finished: {} utterances, {} hallucination drops, {} semantic gate drops, {} filtered empty drops",
        total_utterances, pipeline.hallucination_drops, semantic_gate_drops, filtered_empty_drops
    );
}

// ── Legacy worker functions (deprecated) ────────────────────────────────────

#[deprecated(note = "Use transcription_session with EventSink instead")]
pub(crate) async fn transcription_worker(
    mut chunk_receiver: mpsc::Receiver<Vec<f32>>,
    transcript_buffer: Arc<Mutex<String>>,
    sample_rate: u32,
    language: Option<String>,
    mut postprocessor: Option<StreamPostProcessor>,
    delta_callback: Option<Arc<dyn DeltaSink>>,
    stream_log_path: Option<std::path::PathBuf>,
) {
    info!("Transcription worker started");

    let chunk_duration_sec = stream_chunk_duration_sec();
    let overlap_sec = stream_overlap_sec(chunk_duration_sec);
    let mut session = SpeechSession::new_stream(sample_rate, chunk_duration_sec, overlap_sec);
    let output_sample_rate = session.output_sample_rate();

    // Decouple audio ingestion (chunk_receiver + VAD/session.feed) from Whisper inference.
    // The key property: we never await inference while draining the audio channel.
    const MAX_PENDING_CHUNKS: usize = 64;
    let mut pending_chunks: VecDeque<Vec<f32>> = VecDeque::new();
    let mut dropped_chunks: u64 = 0;
    let mut audio_closed = false;
    let mut in_flight: Option<tokio::task::JoinHandle<Result<String>>> = None;

    loop {
        // Kick off the next transcription job if nothing is running.
        if in_flight.is_none() {
            if let Some(samples) = pending_chunks.pop_front() {
                let lang = language.clone();
                let handle = spawn_chunk_transcription(samples, output_sample_rate, lang);
                in_flight = Some(handle);
            } else if audio_closed {
                break;
            }
        }

        tokio::select! {
            maybe_data = chunk_receiver.recv(), if !audio_closed => {
                match maybe_data {
                    Some(data) => {
                        for event in session.feed(&data, sample_rate) {
                            if let SpeechEvent::Chunk(samples) = event {
                                if pending_chunks.len() >= MAX_PENDING_CHUNKS {
                                    dropped_chunks = dropped_chunks.saturating_add(1);
                                    continue;
                                }
                                pending_chunks.push_back(samples);
                            }
                        }
                    }
                    None => {
                        audio_closed = true;
                        if let Some(SpeechEvent::Chunk(samples)) = session.flush() {
                            if pending_chunks.len() < MAX_PENDING_CHUNKS {
                                pending_chunks.push_back(samples);
                            } else {
                                dropped_chunks = dropped_chunks.saturating_add(1);
                            }
                        }
                    }
                }
            }
            result = async {
                // Safe: guarded by `if in_flight.is_some()` below.
                in_flight.as_mut().unwrap().await
            }, if in_flight.is_some() => {
                match result {
                    Ok(Ok(text)) => {
                        if !text.trim().is_empty() {
                            debug!("Chunk transcribed: '{}'", text.trim());

                            // Post-process (lexicon + cleanup + semantic gate).
                            let cleaned = {
                                if let Some(processor) = postprocessor.as_mut() {
                                    processor.process(&text)
                                } else {
                                    let cleaned = text.split_whitespace().collect::<Vec<_>>().join(" ");
                                    if cleaned.trim().is_empty() { None } else { Some(cleaned) }
                                }
                            };

                            if let Some(cleaned) = cleaned {
                                // Update transcript buffer and compute delta while holding the lock,
                                // but do not call sink/log under that lock.
                                let delta = {
                                    let mut buffer = transcript_buffer.lock().await;
                                    let before = buffer.clone();
                                    dedup_chunk_overlap(&mut buffer, &cleaned);
                                    build_redacted_delta(&before, &buffer)
                                };

                                if let Some(delta) = delta {
                                    let has_effect =
                                        delta.chars().any(|c| c == '\u{0008}' || !c.is_whitespace());
                                    if has_effect {
                                        if let Some(sink) = delta_callback.as_ref() {
                                            sink.apply(&TranscriptDelta::from_raw(&delta));
                                        }
                                        if let Some(path) = stream_log_path.as_deref() {
                                            let _ = append_to_stream_log(path, &delta);
                                        }
                                    }
                                }
                            } else {
                                debug!("Stream postprocessor dropped chunk");
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        error!("Chunk transcription failed: {}", e);
                    }
                    Err(e) => {
                        error!("Transcription task join error: {}", e);
                    }
                }
                in_flight = None;
            }
        }
    }

    if dropped_chunks > 0 {
        warn!(
            "Streaming worker dropped {} transcription chunk(s) due to backpressure (audio was still ingested)",
            dropped_chunks
        );
    }

    info!("Transcription worker finished");
}

#[deprecated(note = "Use transcription_session with EventSink instead")]
pub(crate) async fn buffered_transcription_worker(
    mut chunk_receiver: mpsc::Receiver<Vec<f32>>,
    transcript_buffer: Arc<Mutex<String>>,
    config: BufferedWorkerConfig,
) {
    let BufferedWorkerConfig {
        sample_rate,
        language,
        delta_callback,
        utterance_callback,
        utterance_silence_sec,
        vad_start_callback,
        stream_log_path,
    } = config;
    info!("Buffered transcription worker started");

    let correction_min_utterances = buffered_correction_min_utterances();
    let correction_min_sec = buffered_correction_min_sec();
    let mut session = if let Some(sec) = utterance_silence_sec {
        SpeechSession::new_utterance_with_silence(sample_rate, sec)
    } else {
        SpeechSession::new_utterance(sample_rate)
    };
    let output_sample_rate = session.output_sample_rate();
    let mut vad_start_emitted = false;
    let mut pipeline = TranscriptionPipeline::new(language);
    let emitter = Arc::new(Mutex::new(BufferedEmitter::new(
        transcript_buffer.clone(),
        delta_callback,
        stream_log_path,
    )));

    let emitter_handle = tokio::spawn(emitter_tick_loop(emitter.clone()));

    let mut correction_audio_buf: Vec<f32> = Vec::new();
    let mut utterance_count: usize = 0;
    let mut suffix_snapshot = String::new();
    // Accumulate interim segments into an utterance-sized payload for the caller.
    // This avoids "sending" on every interim emit (which exists purely for UX),
    // while still allowing frequent Whisper passes for streaming preview.
    let mut pending_utterance_text = String::new();

    // Decouple audio ingestion (chunk_receiver + VAD/session.feed) from Whisper inference.
    const MAX_PENDING_UTTERANCES: usize = 64;
    let mut pending_utterances: VecDeque<UtteranceWorkItem> = VecDeque::new();
    let mut dropped_utterances: u64 = 0;
    let mut audio_closed = false;

    // Phase 1 (streaming preview) — one utterance transcription in flight.
    let mut utterance_in_flight: Option<tokio::task::JoinHandle<Result<String>>> = None;
    let mut utterance_active: Option<UtteranceWorkItem> = None;

    // Phase 2 (buffered correction) — re-transcription in flight.
    let mut correction_in_flight: Option<tokio::task::JoinHandle<Result<String>>> = None;
    let mut correction_current_suffix: Option<String> = None;

    loop {
        // Start next utterance transcription if possible.
        if utterance_in_flight.is_none()
            && correction_in_flight.is_none()
            && let Some(item) = pending_utterances.pop_front()
        {
            let lang = pipeline.language.clone();
            let handle =
                spawn_utterance_transcription(item.audio.clone(), output_sample_rate, lang);
            utterance_in_flight = Some(handle);
            utterance_active = Some(item);
        }

        // If audio is closed and there is no work left, finish.
        if audio_closed
            && pending_utterances.is_empty()
            && utterance_in_flight.is_none()
            && correction_in_flight.is_none()
        {
            break;
        }

        tokio::select! {
            maybe_data = chunk_receiver.recv(), if !audio_closed => {
                match maybe_data {
                    Some(data) => {
                        for event in session.feed(&data, sample_rate) {
                            let (utterance, is_final) = match event {
                                SpeechEvent::Utterance(u) => (u, false),
                                SpeechEvent::UtteranceFinal(u) => (u, true),
                                _ => continue,
                            };

                            if !vad_start_emitted {
                                if let Some(callback) = &vad_start_callback {
                                    callback();
                                }
                                vad_start_emitted = true;
                            }

                            if pending_utterances.len() >= MAX_PENDING_UTTERANCES {
                                dropped_utterances = dropped_utterances.saturating_add(1);
                                continue;
                            }

                            pending_utterances.push_back(UtteranceWorkItem {
                                audio: utterance,
                                is_final,
                            });
                        }
                    }
                    None => {
                        audio_closed = true;
                        if let Some(event) = session.flush() {
                            let (utterance, is_final) = match event {
                                SpeechEvent::Utterance(u) => (u, false),
                                SpeechEvent::UtteranceFinal(u) => (u, true),
                                _ => (Vec::new(), false),
                            };

                            if !utterance.is_empty() {
                                if pending_utterances.len() < MAX_PENDING_UTTERANCES {
                                    pending_utterances.push_back(UtteranceWorkItem { audio: utterance, is_final });
                                } else {
                                    dropped_utterances = dropped_utterances.saturating_add(1);
                                }
                            }
                        }
                    }
                }
            }
            result = async {
                correction_in_flight.as_mut().unwrap().await
            }, if correction_in_flight.is_some() => {
                let current_suffix = correction_current_suffix.take().unwrap_or_default();
                match result {
                    Ok(Ok(raw)) => {
                        if let Some(cleaned) = pipeline.postprocess(&raw) {
                            let mut guard = emitter.lock().await;
                            guard.push_correction(cleaned);
                            // postprocess() already updated last_suffix to match re-transcription.
                        } else if !current_suffix.is_empty() {
                            // Re-transcription was empty/filtered — restore suffix so next utterance
                            // deduplicates against the Phase-1 draft.
                            pipeline.last_suffix = current_suffix;
                        }
                    }
                    _ => {
                        warn!("Re-transcription failed; keeping Phase 1 draft");
                        if !current_suffix.is_empty() {
                            pipeline.last_suffix = current_suffix;
                        }
                    }
                }

                utterance_count = 0;
                correction_in_flight = None;
            }
            result = async {
                utterance_in_flight.as_mut().unwrap().await
            }, if utterance_in_flight.is_some() => {
                let item = utterance_active.take().unwrap_or_else(|| UtteranceWorkItem { audio: Vec::new(), is_final: false });
                match result {
                    Ok(Ok(raw_text)) => {
                        if utterance_count == 0 && correction_audio_buf.is_empty() {
                            suffix_snapshot = pipeline.last_suffix.clone();
                        }

                        if let Some(cleaned) = pipeline.postprocess(&raw_text) {
                            {
                                let mut guard = emitter.lock().await;
                                guard.push_segment(cleaned.clone());
                            }

                            if !pending_utterance_text.is_empty() {
                                pending_utterance_text.push(' ');
                            }
                            pending_utterance_text.push_str(cleaned.trim());
                        }

                        if item.is_final {
                            if let Some(callback) = &utterance_callback {
                                let payload = pending_utterance_text.trim();
                                if !payload.is_empty() {
                                    callback(payload.to_string());
                                }
                            }
                            pending_utterance_text.clear();

                            // Reset Phase 2 correction state on utterance boundary.
                            correction_audio_buf.clear();
                            utterance_count = 0;
                        } else {
                            // Phase 2 correction accumulation — only for non-final items.
                            correction_audio_buf.extend_from_slice(&item.audio);
                            utterance_count += 1;

                            let audio_duration_s =
                                correction_audio_buf.len() as f32 / output_sample_rate as f32;
                            if utterance_count >= correction_min_utterances || audio_duration_s >= correction_min_sec {
                                let audio = std::mem::take(&mut correction_audio_buf);
                                let lang = pipeline.language.clone();

                                let current_suffix = pipeline.last_suffix.clone();
                                pipeline.last_suffix = suffix_snapshot.clone();
                                correction_current_suffix = Some(current_suffix);

                                // Abort stale correction task to prevent task leak.
                                if let Some(old) = correction_in_flight.take() {
                                    old.abort();
                                }
                                correction_in_flight = Some(spawn_utterance_transcription(
                                    audio,
                                    output_sample_rate,
                                    lang,
                                ));
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        error!("Buffered transcription failed: {}", e);
                    }
                    Err(e) => {
                        error!("Buffered transcription task join error: {}", e);
                    }
                }

                utterance_in_flight = None;
            }
        }
    }

    // On recorder stop, flush any accumulated utterance payload even if the last slice
    // was fully deduplicated (common when we emitted frequent interim segments).
    if let Some(callback) = &utterance_callback {
        let payload = pending_utterance_text.trim();
        if !payload.is_empty() {
            callback(payload.to_string());
        }
    }

    if !vad_start_emitted && let Some(callback) = &vad_start_callback {
        callback();
    }

    {
        let mut guard = emitter.lock().await;
        guard.finish();
        info!(
            "Stream Session Stats: Hallucinations dropped: {}, Overlaps stripped: {}, Corrections applied: {}",
            pipeline.hallucination_drops, pipeline.overlap_strips, guard.corrections_applied
        );
    }

    if let Err(e) = emitter_handle.await {
        error!("Buffered emitter task failed: {}", e);
    }

    if dropped_utterances > 0 {
        warn!(
            "Buffered worker dropped {} utterance(s) due to backpressure (audio was still ingested)",
            dropped_utterances
        );
    }

    info!("Buffered transcription worker finished");
}

#[derive(Clone, Debug)]
struct UtteranceWorkItem {
    audio: Vec<f32>,
    is_final: bool,
}

fn spawn_chunk_transcription(
    samples: Vec<f32>,
    sample_rate: u32,
    language: Option<String>,
) -> tokio::task::JoinHandle<Result<String>> {
    tokio::task::spawn_blocking(move || {
        crate::stt::transcribe_chunk(&samples, sample_rate, language.as_deref())
    })
}

fn spawn_utterance_transcription(
    samples: Vec<f32>,
    sample_rate: u32,
    language: Option<String>,
) -> tokio::task::JoinHandle<Result<String>> {
    tokio::task::spawn_blocking(move || {
        // Use try_lock to avoid blocking-pool saturation when corrections
        // pile up faster than the engine can process them.  If the engine
        // is already busy (main transcription or a previous correction),
        // we bail immediately — the next correction cycle will pick up
        // the accumulated audio.
        crate::stt::try_transcribe_long(&samples, sample_rate, language.as_deref())
    })
}

// ── Public: batch streaming transcription ────────────────────────────────────

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
    // Disabled by default to preserve legacy behavior.
    if env_bool_default("CODESCRIBE_STREAM_LEXICON", false) && !out.trim().is_empty() {
        let mut lex = StreamPostProcessor::new();
        if let Some(cleaned) = lex.process(&out) {
            out = cleaned;
        }
    }

    Ok(out)
}

/// Public helper: run the buffered (overlay) pipeline on in-memory samples.
///
/// This mirrors the live overlay behavior (VAD → utterance → buffered emitter),
/// but runs on a finite sample buffer for test/CLI comparisons.
#[allow(deprecated)]
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
    let transcript_buffer = Arc::new(Mutex::new(String::new()));

    let worker = tokio::spawn(buffered_transcription_worker(
        rx,
        transcript_buffer.clone(),
        BufferedWorkerConfig {
            sample_rate,
            language,
            delta_callback: None,
            utterance_callback: None,
            utterance_silence_sec: None,
            vad_start_callback: None,
            stream_log_path: None,
        },
    ));

    for chunk in samples.chunks(chunk_size) {
        if tx.send(chunk.to_vec()).await.is_err() {
            return Err(anyhow!("Buffered transcription worker dropped channel"));
        }
    }
    drop(tx);

    worker
        .await
        .map_err(|e| anyhow!("Buffered transcription worker join error: {}", e))?;

    Ok(transcript_buffer.lock().await.clone())
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

fn buffered_correction_min_utterances() -> usize {
    env_usize("CODESCRIBE_BUFFERED_CORRECTION_UTTERANCES", 2).clamp(1, 10)
}

fn buffered_correction_min_sec() -> f32 {
    env_f32("CODESCRIBE_BUFFERED_CORRECTION_SEC", 6.0).clamp(1.0, 60.0)
}

fn buffered_correction_prefix_ratio() -> f64 {
    env_f32("CODESCRIBE_BUFFERED_CORRECTION_PREFIX", 0.60).clamp(0.4, 0.9) as f64
}

pub(crate) fn stream_chunk_duration_sec() -> f32 {
    env_f32("CODESCRIBE_STREAM_CHUNK_SEC", DEFAULT_CHUNK_DURATION_SEC).clamp(0.5, 30.0)
}

pub(crate) fn stream_overlap_sec(chunk_duration_sec: f32) -> f32 {
    let ratio = env_f32("CODESCRIBE_STREAM_OVERLAP_RATIO", DEFAULT_OVERLAP_RATIO).clamp(0.05, 0.8);
    (chunk_duration_sec * ratio).min(chunk_duration_sec * 0.8)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_postprocess_components() {
        // Hallucination
        assert!(is_hallucination("Thank you"));
        assert!(is_hallucination("  Dziękuję za uwagę  "));
        assert!(!is_hallucination("Tak")); // Whitelisted
        assert!(!is_hallucination("This is a normal sentence."));

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
    fn test_suffix_preserved_when_postprocess_filters() {
        // Simulates the re-transcription scenario: if postprocess returns None
        // (e.g. hallucination), last_suffix must stay at the pre-snapshot value.
        let mut pipeline = TranscriptionPipeline::new(None);
        pipeline.last_suffix = "original suffix".to_string();

        // "Thank you" is a hallucination — postprocess returns None
        let result = pipeline.postprocess("Thank you");
        assert!(result.is_none());
        // last_suffix unchanged (strip_overlap was never reached)
        assert_eq!(pipeline.last_suffix, "original suffix");
    }

    #[test]
    fn test_suffix_updated_after_successful_postprocess() {
        let mut pipeline = TranscriptionPipeline::new(None);
        pipeline.last_suffix = "old tail".to_string();

        let result = pipeline.postprocess("This is a brand new sentence.");
        assert!(result.is_some());
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
}
