//! Streaming transcription pipeline — orchestration, buffered emission, and policy.
//!
//! Extracted from `audio::streaming_recorder` to decouple pipeline logic
//! (hallucination filtering, overlap dedup, re-transcription, buffered "typing"
//! emission) from the audio capture layer.
//!
//! Created by M&K (c)2026 VetCoders

use crate::audio::chunker::{SpeechEvent, SpeechSession};
use crate::pipeline::dedup::{dedup_chunk_overlap, strip_suffix_overlap};
use crate::pipeline::stream_postprocess::{LexiconPostProcessor, StreamPostProcessor};
use crate::stt::whisper;
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

const DEFAULT_CHUNK_DURATION_SEC: f32 = 15.0;
const DEFAULT_OVERLAP_RATIO: f32 = 0.25; // 25% overlap for context
const DEFAULT_BUFFER_DELAY_MS: u64 = 3000;
const DEFAULT_TYPING_CPS: f32 = 30.0;
const DEFAULT_EMIT_WORDS_MAX: usize = 3;

lazy_static! {
    static ref TOKEN_RE: Regex = Regex::new(r"\s+|\S+\s*").expect("token regex");
}

// ── Public type alias ────────────────────────────────────────────────────────

use crate::pipeline::contracts::{DeltaSink, TranscriptDelta};

/// Legacy alias — now backed by `DeltaSink` trait instead of bare `Fn(&str)`.
/// Consumers should migrate to `Arc<dyn DeltaSink>` directly.
#[deprecated(note = "Use Arc<dyn DeltaSink> directly")]
pub type StreamDeltaCallback = Arc<dyn DeltaSink>;

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
    pub(crate) postprocessor: LexiconPostProcessor,
    pub(crate) last_suffix: String,
    pub(crate) hallucination_drops: u64,
    pub(crate) overlap_strips: u64,
}

impl TranscriptionPipeline {
    pub(crate) fn new(language: Option<String>) -> Self {
        Self {
            language,
            postprocessor: LexiconPostProcessor::new(),
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

        let processed = self.postprocessor.process(&stripped)?;

        let suffix_len = 50;
        let start = processed.len().saturating_sub(suffix_len);
        self.last_suffix = processed[start..].to_string();

        Some(processed)
    }
}

// ── BufferedEmitter ──────────────────────────────────────────────────────────

pub(crate) struct BufferedEmitter {
    queue: VecDeque<String>,
    initial_delay_ms: u64,
    typing_speed_cps: f32,
    emit_words_max: usize,
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
    pub(crate) fn new(
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

    pub(crate) fn push_correction(&mut self, corrected: String) {
        if self.emitted_text.is_empty() {
            return;
        }
        // Guard: reject corrections that would rewrite most of the text.
        // Common-prefix must cover >= 70% of the shorter string.
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
        if min_len > 0 && (prefix_len as f64 / min_len as f64) < 0.70 {
            debug!(
                "Correction rejected: common prefix {}/{} ({:.0}%) < 70%",
                prefix_len,
                min_len,
                prefix_len as f64 / min_len as f64 * 100.0,
            );
            return;
        }
        self.correction_pending = Some(corrected);
    }

    pub(crate) fn push_segment(&mut self, text: String) {
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

    pub(crate) async fn tick(&mut self) -> bool {
        if self.finished && self.queue.is_empty() && self.current_segment.is_none() {
            return true;
        }

        if self.is_buffering() {
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

    pub(crate) fn finish(&mut self) {
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

pub(crate) async fn emitter_tick_loop(emitter: Arc<Mutex<BufferedEmitter>>) {
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

// ── Worker functions ─────────────────────────────────────────────────────────

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

    while let Some(data) = chunk_receiver.recv().await {
        for event in session.feed(&data, sample_rate) {
            if let SpeechEvent::Chunk(samples) = event {
                process_chunk(
                    &samples,
                    &transcript_buffer,
                    session.output_sample_rate(),
                    language.as_deref(),
                    postprocessor.as_mut(),
                    delta_callback.as_ref(),
                    stream_log_path.as_deref(),
                )
                .await;
            }
        }
    }

    if let Some(SpeechEvent::Chunk(samples)) = session.flush() {
        debug!("Processing final chunk ({} samples)", samples.len());
        process_chunk(
            &samples,
            &transcript_buffer,
            session.output_sample_rate(),
            language.as_deref(),
            postprocessor.as_mut(),
            delta_callback.as_ref(),
            stream_log_path.as_deref(),
        )
        .await;
    }

    info!("Transcription worker finished");
}

pub(crate) async fn buffered_transcription_worker(
    mut chunk_receiver: mpsc::Receiver<Vec<f32>>,
    transcript_buffer: Arc<Mutex<String>>,
    sample_rate: u32,
    language: Option<String>,
    delta_callback: Option<Arc<dyn DeltaSink>>,
    stream_log_path: Option<std::path::PathBuf>,
) {
    info!("Buffered transcription worker started");

    let mut session = SpeechSession::new_utterance(sample_rate);
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

    while let Some(data) = chunk_receiver.recv().await {
        for event in session.feed(&data, sample_rate) {
            if let SpeechEvent::Utterance(utterance) = event {
                if utterance_count == 0 && correction_audio_buf.is_empty() {
                    suffix_snapshot = pipeline.last_suffix.clone();
                }

                let audio_copy = utterance.clone();
                let result = handle_utterance(
                    utterance,
                    session.output_sample_rate(),
                    &mut pipeline,
                    &emitter,
                )
                .await;

                if let Err(e) = result {
                    error!("Buffered transcription failed: {}", e);
                    continue;
                }

                correction_audio_buf.extend_from_slice(&audio_copy);
                utterance_count += 1;

                let audio_duration_s = correction_audio_buf.len() as f32 / sample_rate as f32;
                if utterance_count >= 3 || audio_duration_s >= 10.0 {
                    let audio = std::mem::take(&mut correction_audio_buf);
                    let lang = pipeline.language.clone();

                    let current_suffix = pipeline.last_suffix.clone();
                    pipeline.last_suffix = suffix_snapshot.clone();

                    let re_text = tokio::task::spawn_blocking(move || {
                        whisper::transcribe(&audio, sample_rate, lang.as_deref())
                    })
                    .await;

                    match re_text {
                        Ok(Ok(raw)) => {
                            if let Some(cleaned) = pipeline.postprocess(&raw) {
                                let mut guard = emitter.lock().await;
                                guard.push_correction(cleaned);
                                // postprocess() already updated last_suffix to match
                                // the re-transcribed text — no restore needed.
                            } else {
                                // Re-transcription was empty/filtered — restore suffix
                                // so the next utterance deduplicates against the Phase-1 draft.
                                pipeline.last_suffix = current_suffix;
                            }
                        }
                        _ => {
                            warn!("Re-transcription failed; keeping Phase 1 draft");
                            pipeline.last_suffix = current_suffix;
                        }
                    }

                    utterance_count = 0;
                }
            }
        }
    }

    if let Some(SpeechEvent::Utterance(utterance)) = session.flush()
        && let Err(e) = handle_utterance(
            utterance,
            session.output_sample_rate(),
            &mut pipeline,
            &emitter,
        )
        .await
    {
        error!("Final buffered transcription failed: {}", e);
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

    info!("Buffered transcription worker finished");
}

async fn handle_utterance(
    utterance: Vec<f32>,
    sample_rate: u32,
    pipeline: &mut TranscriptionPipeline,
    emitter: &Arc<Mutex<BufferedEmitter>>,
) -> Result<Option<String>> {
    if utterance.is_empty() {
        return Ok(None);
    }

    let language = pipeline.language.clone();
    let raw_text = tokio::task::spawn_blocking(move || {
        whisper::transcribe(&utterance, sample_rate, language.as_deref())
    })
    .await??;

    if let Some(cleaned) = pipeline.postprocess(&raw_text) {
        let mut guard = emitter.lock().await;
        guard.push_segment(cleaned.clone());
        return Ok(Some(cleaned));
    }

    Ok(None)
}

async fn process_chunk(
    samples: &[f32],
    transcript_buffer: &Arc<Mutex<String>>,
    sample_rate: u32,
    language: Option<&str>,
    mut postprocessor: Option<&mut StreamPostProcessor>,
    delta_callback: Option<&Arc<dyn DeltaSink>>,
    stream_log_path: Option<&Path>,
) {
    if samples.is_empty() {
        return;
    }

    let samples_owned = samples.to_vec();
    let lang_owned = language.map(String::from);

    let result = tokio::task::spawn_blocking(move || {
        let engine_mutex = match get_engine() {
            Ok(m) => m,
            Err(e) => return Err(anyhow!("Engine error: {}", e)),
        };

        let mut engine_guard = match engine_mutex.lock() {
            Ok(g) => g,
            Err(e) => return Err(anyhow!("Lock error: {}", e)),
        };

        engine_guard.transcribe_with_language(&samples_owned, sample_rate, lang_owned.as_deref())
    })
    .await;

    match result {
        Ok(Ok(text)) => {
            if !text.trim().is_empty() {
                debug!("Chunk transcribed: '{}'", text.trim());
                let cleaned = if let Some(processor) = postprocessor.as_mut() {
                    processor.process(&text)
                } else {
                    let cleaned = crate::pipeline::stream_postprocess::normalize_whitespace(&text);
                    if cleaned.trim().is_empty() {
                        None
                    } else {
                        Some(cleaned)
                    }
                };

                if let Some(cleaned) = cleaned {
                    let mut buffer = transcript_buffer.lock().await;
                    let before = buffer.clone();
                    dedup_chunk_overlap(&mut buffer, &cleaned);
                    if let Some(delta) = build_redacted_delta(&before, &buffer) {
                        let has_effect =
                            delta.chars().any(|c| c == '\u{0008}' || !c.is_whitespace());
                        if has_effect {
                            if let Some(sink) = delta_callback {
                                sink.apply(&TranscriptDelta::from_raw(&delta));
                            }
                            if let Some(path) = stream_log_path {
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

    Ok(out)
}

// ── Delta helpers ────────────────────────────────────────────────────────────

pub(crate) fn build_redacted_delta(before: &str, after: &str) -> Option<String> {
    if before == after {
        return None;
    }

    let mut prefix_len = 0usize;
    for (a, b) in before.chars().zip(after.chars()) {
        if a == b {
            prefix_len += a.len_utf8();
        } else {
            break;
        }
    }

    let removed = before[prefix_len..].chars().count();
    let mut delta = String::new();
    for _ in 0..removed {
        delta.push('\u{0008}');
    }
    delta.push_str(&after[prefix_len..]);
    Some(delta)
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
}
