//! Typing-animation emitter: buffered character-by-character emission of
//! transcript text via `DeltaSink`, plus redacted-delta correction replay.

use std::collections::VecDeque;
use std::sync::Arc;

use lazy_static::lazy_static;
use regex::Regex;
use tokio::sync::Mutex;
use tokio::time::{Duration, Instant};
use tracing::debug;

use crate::pipeline::contracts::{DeltaSink, TranscriptDelta};

use super::stream_log::append_to_stream_log;
use super::tuning::{buffered_correction_prefix_ratio, env_f32, env_u64, env_usize};

// Golden runtime profile (balanced for low-latency preview + stable quality).
const DEFAULT_BUFFER_DELAY_MS: u64 = 280;
const DEFAULT_TYPING_CPS: f32 = 90.0;
const DEFAULT_EMIT_WORDS_MAX: usize = 2;

lazy_static! {
    static ref TOKEN_RE: Regex = Regex::new(r"\s+|\S+\s*").expect("token regex");
}

// ── BufferedEmitter ──────────────────────────────────────────────────────────

/// Typing-animation emitter for transcript segments.
///
/// Buffers incoming text and emits it character-by-character at a configurable
/// typing speed via `DeltaSink`. Used by `app::presentation::PresentationEmitter`.
pub struct BufferedEmitter {
    pub(crate) queue: VecDeque<String>,
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
    pub(crate) emitted_text: String,
    pub(crate) target_text: String,
    pub(crate) correction_pending: Option<String>,
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
            target_text: String::new(),
            correction_pending: None,
            last_correction_at: None,
            corrections_applied: 0,
        }
    }

    /// Update the desired full rendered transcript for the session.
    ///
    /// The emitter turns prefix growth into incremental queued appends and
    /// everything else into a correction against the currently emitted text.
    /// This keeps append semantics clean across utterance boundaries while
    /// allowing active-tail rewrites from partial/correction passes.
    pub fn set_target_text(&mut self, target: String) -> Option<String> {
        let current_target = if self.target_text.is_empty() && !self.emitted_text.is_empty() {
            self.emitted_text.clone()
        } else {
            self.target_text.clone()
        };

        if target == current_target {
            self.target_text = target;
            return None;
        }

        if target.starts_with(&current_target) {
            let suffix = target[current_target.len()..].to_string();
            if !suffix.is_empty() {
                self.queue.push_back(suffix);
                if self.first_output_at.is_none() {
                    self.first_output_at = Some(Instant::now());
                }
            }
            self.target_text = target.clone();
            return Some(target);
        }

        let prefix_len = self
            .emitted_text
            .chars()
            .zip(target.chars())
            .take_while(|(a, b)| a == b)
            .count();
        let min_len = self
            .emitted_text
            .chars()
            .count()
            .min(target.chars().count());
        if min_len > 0 && (prefix_len as f64 / min_len as f64) < self.correction_prefix_ratio {
            debug!(
                "Applying wide correction: common prefix {}/{} ({:.0}%) < {:.0}%",
                prefix_len,
                min_len,
                prefix_len as f64 / min_len as f64 * 100.0,
                self.correction_prefix_ratio * 100.0,
            );
        }

        self.queue.clear();
        self.current_segment = None;
        self.current_tokens.clear();
        self.current_token_index = 0;
        self.target_text = target.clone();
        self.correction_pending = Some(target.clone());
        if self.first_output_at.is_none() && !target.is_empty() {
            self.first_output_at = Some(Instant::now());
        }
        Some(target)
    }

    pub async fn store_transcript_snapshot(&self, snapshot: String) {
        let mut buffer = self.transcript_buffer.lock().await;
        *buffer = snapshot;
    }

    pub fn push_correction(&mut self, corrected: String) {
        if corrected.trim().is_empty() {
            return;
        }
        let _ = self.set_target_text(corrected);
    }

    pub fn push_segment(&mut self, text: String) {
        if text.trim().is_empty() {
            return;
        }
        let mut target = self.target_text.clone();
        target.push_str(&text);
        let _ = self.set_target_text(target);
    }

    pub async fn tick(&mut self) -> bool {
        if self.finished
            && self.queue.is_empty()
            && self.current_segment.is_none()
            && self.correction_pending.is_none()
        {
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

        self.finished
            && self.queue.is_empty()
            && self.current_segment.is_none()
            && self.correction_pending.is_none()
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
