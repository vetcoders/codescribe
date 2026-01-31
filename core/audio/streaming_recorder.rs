use crate::audio::recorder::{Recorder, RecorderConfig};
use crate::stream_postprocess::StreamPostProcessor;
use crate::stt::whisper;
use crate::stt::whisper::append_with_overlap_dedup;
use crate::stt::whisper::singleton::engine as get_engine;
use crate::vad;
use anyhow::{Context, Result, anyhow};
use chrono::SecondsFormat;
use lazy_static::lazy_static;
use regex::Regex;
use std::collections::VecDeque;
use std::sync::Arc;
use std::{fs::OpenOptions, io::Write, path::Path};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tokio::time::{Duration, Instant};
use tracing::{debug, error, info, trace, warn};

const DEFAULT_CHUNK_DURATION_SEC: f32 = 15.0;
const DEFAULT_OVERLAP_RATIO: f32 = 0.25; // 25% overlap for context
// VAD config now centralized in core/vad/config.rs (CODESCRIBE_VAD_* env vars)
const DEFAULT_BUFFER_DELAY_MS: u64 = 3000;
const DEFAULT_TYPING_CPS: f32 = 30.0;
const DEFAULT_EMIT_WORDS_MAX: usize = 3;

lazy_static! {
    static ref TOKEN_RE: Regex = Regex::new(r"\s+|\S+\s*").expect("token regex");
}

pub type StreamDeltaCallback = Arc<dyn Fn(&str) + Send + Sync>;

pub struct StreamingRecorder {
    pub recorder: Recorder,
    transcript_buffer: Arc<Mutex<String>>,
    transcription_handle: Option<JoinHandle<()>>,
    sample_rate: u32,
    delta_callback: Option<StreamDeltaCallback>,
}

impl StreamingRecorder {
    pub fn new() -> Result<Self> {
        let recorder = Recorder::new()?;
        let sample_rate = recorder.config.sample_rate;

        Ok(Self {
            recorder,
            transcript_buffer: Arc::new(Mutex::new(String::new())),
            transcription_handle: None,
            sample_rate,
            delta_callback: None,
        })
    }

    pub fn with_config(config: RecorderConfig) -> Result<Self> {
        let sample_rate = config.sample_rate;
        let recorder = Recorder::with_config(config)?;

        Ok(Self {
            recorder,
            transcript_buffer: Arc::new(Mutex::new(String::new())),
            transcription_handle: None,
            sample_rate,
            delta_callback: None,
        })
    }

    pub fn set_delta_callback(&mut self, callback: Option<StreamDeltaCallback>) {
        self.delta_callback = callback;
    }

    pub async fn start(&mut self, language: Option<String>) -> Result<()> {
        // docs/env.md says default is 0: prefer real-time chunking (live preview),
        // and keep buffered mode as an explicit opt-in.
        let use_buffered_stream = env_bool_default("CODESCRIBE_BUFFERED_STREAM", false);
        self.start_with_buffered(language, use_buffered_stream)
            .await
    }

    pub async fn start_with_buffered(
        &mut self,
        language: Option<String>,
        use_buffered_stream: bool,
    ) -> Result<()> {
        // Clear previous transcript
        *self.transcript_buffer.lock().await = String::new();

        // Create channel for audio chunks
        // Buffer size: enough to hold a few seconds if worker is slow
        let (tx, rx) = mpsc::channel::<Vec<f32>>(500);

        // Setup callback to send audio data
        // Note: try_send to avoid blocking audio thread
        self.recorder.set_callback(Box::new(move |data| {
            if let Err(_e) = tx.try_send(data.to_vec()) {
                // If channel is full, we drop audio (better than blocking)
                // But we should log occasionally?
                // For now just ignore or print to stderr if needed, but avoid spamming logs
            }
        }));

        // Start the actual audio stream first, so we know the *real* sample rate (often 48kHz).
        self.recorder.start().await?;

        // Update sample rate to the one used by the input stream.
        // This is critical: we must pass the correct `sample_rate` to Whisper so it can resample.
        let actual_sample_rate = self.recorder.actual_sample_rate();
        if actual_sample_rate != self.sample_rate {
            info!(
                "StreamingRecorder sample_rate updated: config={}Hz -> actual={}Hz",
                self.sample_rate, actual_sample_rate
            );
            self.sample_rate = actual_sample_rate;
        } else {
            debug!("StreamingRecorder sample_rate: {}Hz", actual_sample_rate);
        }

        // Start transcription worker (after we know the real sample rate)
        let transcript_buffer = self.transcript_buffer.clone();
        let stream_log_path = stream_log_path();
        let delta_callback = self.delta_callback.clone();
        self.transcription_handle = Some(tokio::spawn(async move {
            if use_buffered_stream {
                buffered_transcription_worker(
                    rx,
                    transcript_buffer,
                    actual_sample_rate,
                    language,
                    delta_callback,
                    stream_log_path,
                )
                .await;
            } else {
                let postprocessor = if env_bool("CODESCRIBE_BUFFERED_STREAM") {
                    Some(StreamPostProcessor::new())
                } else {
                    None
                };
                transcription_worker(
                    rx,
                    transcript_buffer,
                    actual_sample_rate,
                    language,
                    postprocessor,
                    delta_callback,
                    stream_log_path,
                )
                .await;
            }
        }));

        Ok(())
    }

    pub async fn stop(&mut self) -> Result<(String, Option<std::path::PathBuf>)> {
        info!("Stopping streaming recorder...");

        // 1. Stop recording (drops callback and sender)
        let audio_path = self.recorder.stop().await?;

        // 2. Wait for worker to finish processing remaining chunks
        if let Some(handle) = self.transcription_handle.take() {
            debug!("Waiting for transcription worker to finish...");
            handle.await.context("Transcription worker failed")?;
        }

        // 3. Return collected transcript
        let transcript = self.transcript_buffer.lock().await.clone();
        Ok((transcript, audio_path))
    }

    pub async fn stop_without_saving(&mut self) -> Result<String> {
        info!("Stopping streaming recorder (no WAV)...");

        // Recorder::stop() always writes a temp WAV today; delete it immediately.
        if let Some(path) = self.recorder.stop().await? {
            let _ = tokio::fs::remove_file(path).await;
        }

        // 2. Wait for worker to finish processing remaining chunks
        if let Some(handle) = self.transcription_handle.take() {
            debug!("Waiting for transcription worker to finish...");
            handle.await.context("Transcription worker failed")?;
        }

        // 3. Return collected transcript
        let transcript = self.transcript_buffer.lock().await.clone();
        Ok(transcript)
    }
}

async fn transcription_worker(
    mut chunk_receiver: mpsc::Receiver<Vec<f32>>,
    transcript_buffer: Arc<Mutex<String>>,
    sample_rate: u32,
    language: Option<String>,
    mut postprocessor: Option<StreamPostProcessor>,
    delta_callback: Option<StreamDeltaCallback>,
    stream_log_path: Option<std::path::PathBuf>,
) {
    info!("Transcription worker started");

    let chunk_duration_sec = stream_chunk_duration_sec();
    let overlap_sec = stream_overlap_sec(chunk_duration_sec);
    let mut session = SpeechSession::new_stream(sample_rate, chunk_duration_sec, overlap_sec);

    // We keep track of how many samples we've processed to know when to overlap
    // Actually, we just keep the last samples in pending_samples?
    // No, pending_samples grows. When it hits limit, we transcribe.
    // Then we keep the tail as the new pending_samples.

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

enum SpeechEvent {
    Chunk(Vec<f32>),
    Utterance(Vec<f32>),
}

enum SpeechMode {
    Stream {
        chunk_limit: usize,
        overlap_size: usize,
    },
    Utterance {
        max_utterance_samples: usize,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum VadGateMode {
    /// Gate audio before it reaches Whisper (legacy).
    Simple,
    /// Silero VAD iter logic as a hard gate (legacy).
    Iter,
    /// Silero VAD is a supervisor: audio always flows, VAD only defines boundaries.
    Supervisor,
}

struct GateConfig {
    vad: vad::VadConfig,
    pre_roll_sec: f32,
    speech_pad_sec: f32,
    mode: VadGateMode,
}

struct SpeechSession {
    mode: SpeechMode,
    threshold: f32,
    neg_threshold: f32,
    min_speech_samples: usize,
    min_silence_samples: usize,
    in_speech: bool,
    speech_samples: usize,
    silence_samples: usize,
    pending_speech: Vec<f32>,
    pending_silence: Vec<f32>,
    pending_samples: Vec<f32>,
    pre_roll: VecDeque<f32>,
    pre_roll_samples: usize,
    speech_pad_samples: usize,
    last_append_at: Instant,
    vad: Option<vad::SileroVad>,
    resampler: Option<vad::Resampler>,
    vad_resample_buf: Vec<f32>,
    output_sample_rate: u32,
    raw_sample_rate: u32,
    gate_mode: VadGateMode,
    iter_state: Option<VadIterState>,
    iter_speech_start: Option<usize>,
    raw_buffer: VecDeque<f32>,
    raw_buffer_start: usize,
    raw_cursor: usize,
    segment_start: Option<usize>,
    pending_end: Option<usize>,
    pre_roll_raw: usize,
    speech_pad_raw: usize,
    last_emit_raw: usize,
}

impl SpeechSession {
    fn new_stream(sample_rate: u32, chunk_duration_sec: f32, overlap_sec: f32) -> Self {
        let config = hardcoded_gate_config();
        let vad_sample_rate = vad::VAD_SAMPLE_RATE;
        let output_sample_rate = match config.mode {
            VadGateMode::Supervisor => sample_rate,
            _ => vad_sample_rate,
        };
        let min_speech_samples = (config.vad.min_speech_duration_sec * vad_sample_rate as f32)
            .round()
            .max(1.0) as usize;
        let min_silence_samples = (config.vad.max_silence_duration_sec * vad_sample_rate as f32)
            .round()
            .max(1.0) as usize;
        let neg_threshold = (config.vad.threshold - 0.15).max(0.05);

        let vad = init_silero_vad(vad_sample_rate, &config.vad);
        let resampler = if sample_rate != vad_sample_rate {
            Some(vad::Resampler::new(sample_rate))
        } else {
            None
        };
        let pre_roll_samples = (config.pre_roll_sec * output_sample_rate as f32)
            .round()
            .max(0.0) as usize;
        let speech_pad_samples = (config.speech_pad_sec * output_sample_rate as f32)
            .round()
            .max(0.0) as usize;
        let iter_state = match config.mode {
            VadGateMode::Iter | VadGateMode::Supervisor => {
                Some(VadIterState::new(&config, vad::VAD_SAMPLE_RATE))
            }
            VadGateMode::Simple => None,
        };
        let chunk_limit_raw = (sample_rate as f32 * chunk_duration_sec).round().max(1.0) as usize;
        let overlap_raw = (sample_rate as f32 * overlap_sec).round().max(0.0) as usize;
        let pre_roll_raw = (sample_rate as f32 * config.pre_roll_sec).round().max(0.0) as usize;
        let speech_pad_raw = (sample_rate as f32 * config.speech_pad_sec)
            .round()
            .max(0.0) as usize;

        Self {
            mode: SpeechMode::Stream {
                chunk_limit: chunk_limit_raw,
                overlap_size: overlap_raw,
            },
            threshold: config.vad.threshold,
            neg_threshold,
            min_speech_samples,
            min_silence_samples,
            in_speech: false,
            speech_samples: 0,
            silence_samples: 0,
            pending_speech: Vec::new(),
            pending_silence: Vec::new(),
            pending_samples: Vec::new(),
            pre_roll: VecDeque::new(),
            pre_roll_samples,
            speech_pad_samples,
            last_append_at: Instant::now(),
            vad,
            resampler,
            vad_resample_buf: Vec::new(),
            output_sample_rate,
            raw_sample_rate: sample_rate,
            gate_mode: config.mode,
            iter_state,
            iter_speech_start: None,
            raw_buffer: VecDeque::new(),
            raw_buffer_start: 0,
            raw_cursor: 0,
            segment_start: None,
            pending_end: None,
            pre_roll_raw,
            speech_pad_raw,
            last_emit_raw: 0,
        }
    }

    fn new_utterance(sample_rate: u32) -> Self {
        let config = hardcoded_gate_config();
        let vad_sample_rate = vad::VAD_SAMPLE_RATE;
        let output_sample_rate = match config.mode {
            VadGateMode::Supervisor => sample_rate,
            _ => vad_sample_rate,
        };
        let min_speech_samples = (config.vad.min_speech_duration_sec * vad_sample_rate as f32)
            .round()
            .max(1.0) as usize;
        let min_silence_samples = (config.vad.max_silence_duration_sec * vad_sample_rate as f32)
            .round()
            .max(1.0) as usize;
        let neg_threshold = (config.vad.threshold - 0.15).max(0.05);

        let vad = init_silero_vad(vad_sample_rate, &config.vad);
        let resampler = if sample_rate != vad_sample_rate {
            Some(vad::Resampler::new(sample_rate))
        } else {
            None
        };

        let max_utterance_samples =
            (config.vad.max_utterance_sec * output_sample_rate as f32) as usize;
        let pre_roll_samples = (config.pre_roll_sec * output_sample_rate as f32)
            .round()
            .max(0.0) as usize;
        let speech_pad_samples = (config.speech_pad_sec * output_sample_rate as f32)
            .round()
            .max(0.0) as usize;
        let iter_state = match config.mode {
            VadGateMode::Iter | VadGateMode::Supervisor => {
                Some(VadIterState::new(&config, vad::VAD_SAMPLE_RATE))
            }
            VadGateMode::Simple => None,
        };

        Self {
            mode: SpeechMode::Utterance {
                max_utterance_samples,
            },
            threshold: config.vad.threshold,
            neg_threshold,
            min_speech_samples,
            min_silence_samples,
            in_speech: false,
            speech_samples: 0,
            silence_samples: 0,
            pending_speech: Vec::new(),
            pending_silence: Vec::new(),
            pending_samples: Vec::new(),
            pre_roll: VecDeque::new(),
            pre_roll_samples,
            speech_pad_samples,
            last_append_at: Instant::now(),
            vad,
            resampler,
            vad_resample_buf: Vec::new(),
            output_sample_rate,
            raw_sample_rate: sample_rate,
            gate_mode: config.mode,
            iter_state,
            iter_speech_start: None,
            raw_buffer: VecDeque::new(),
            raw_buffer_start: 0,
            raw_cursor: 0,
            segment_start: None,
            pending_end: None,
            pre_roll_raw: 0,
            speech_pad_raw: 0,
            last_emit_raw: 0,
        }
    }

    fn feed(&mut self, audio: &[f32], _sample_rate: u32) -> Vec<SpeechEvent> {
        let mut events = Vec::new();
        if audio.is_empty() {
            return events;
        }

        if self.gate_mode == VadGateMode::Supervisor {
            return self.feed_supervisor(audio);
        }

        let resampled = if let Some(resampler) = self.resampler.as_mut() {
            resampler.resample(audio)
        } else {
            audio.to_vec()
        };
        self.vad_resample_buf.extend_from_slice(&resampled);

        while self.vad_resample_buf.len() >= vad::CHUNK_SIZE {
            let frame: Vec<f32> = self.vad_resample_buf.drain(..vad::CHUNK_SIZE).collect();
            let speech_prob = match self.vad.as_mut() {
                Some(vad) => vad.predict(&frame).unwrap_or(0.0),
                None => 1.0,
            };
            let decision = match self.gate_mode {
                VadGateMode::Simple => self.gate_with_prob(&frame, speech_prob),
                VadGateMode::Iter => self.gate_with_iter(&frame, speech_prob),
                VadGateMode::Supervisor => self.gate_with_iter(&frame, speech_prob),
            };
            if let Some(mut gated) = decision.audio {
                self.pending_samples.append(&mut gated);
                self.last_append_at = Instant::now();
            }

            if decision.ended {
                if !self.pending_samples.is_empty() {
                    events.push(self.emit_final());
                }
                return events;
            }

            match self.mode {
                SpeechMode::Stream {
                    chunk_limit,
                    overlap_size: _,
                } => {
                    if self.pending_samples.len() >= chunk_limit {
                        events.push(self.emit_chunk());
                    }
                }
                SpeechMode::Utterance {
                    max_utterance_samples,
                } => {
                    if self.pending_samples.len() >= max_utterance_samples {
                        events.push(self.emit_final());
                    }
                }
            }
        }
        events
    }

    fn feed_supervisor(&mut self, audio: &[f32]) -> Vec<SpeechEvent> {
        let mut events = Vec::new();
        if audio.is_empty() {
            return events;
        }

        // Always keep raw audio flowing.
        self.raw_buffer.extend(audio);
        self.raw_cursor = self.raw_cursor.saturating_add(audio.len());

        // Run Silero on 16kHz view of the audio, then map boundaries to raw.
        let resampled = if let Some(resampler) = self.resampler.as_mut() {
            resampler.resample(audio)
        } else {
            audio.to_vec()
        };
        self.vad_resample_buf.extend_from_slice(&resampled);

        while self.vad_resample_buf.len() >= vad::CHUNK_SIZE {
            let frame: Vec<f32> = self.vad_resample_buf.drain(..vad::CHUNK_SIZE).collect();
            let speech_prob = match self.vad.as_mut() {
                Some(vad) => vad.predict(&frame).unwrap_or(0.0),
                None => 1.0,
            };

            trace!(
                "VAD frame: prob={:.3} threshold={:.3} triggered={} segment_start={:?}",
                speech_prob,
                self.threshold,
                self.iter_state
                    .as_ref()
                    .map_or(self.in_speech, |s| s.triggered()),
                self.segment_start,
            );

            let mut start_event: Option<usize> = None;
            let mut end_event: Option<usize> = None;

            if let Some(iter_state) = self.iter_state.as_mut() {
                let event = iter_state.update(speech_prob);
                match event {
                    VadIterEvent::Start { start_sample } => {
                        start_event = Some(start_sample);
                    }
                    VadIterEvent::End { end_sample } => {
                        end_event = Some(end_sample);
                    }
                    VadIterEvent::None => {}
                }
            } else {
                // Fallback to simple threshold logic.
                if !self.in_speech && speech_prob >= self.threshold {
                    self.in_speech = true;
                    self.speech_samples = 0;
                    start_event = Some(self.speech_samples);
                } else if self.in_speech && speech_prob < self.neg_threshold {
                    self.silence_samples = self.silence_samples.saturating_add(frame.len());
                    if self.silence_samples >= self.min_silence_samples {
                        self.in_speech = false;
                        self.silence_samples = 0;
                        end_event = Some(self.speech_samples);
                    }
                }
            }

            if let Some(start_sample) = start_event {
                let raw_start = self
                    .vad_to_raw_index(start_sample)
                    .saturating_sub(self.pre_roll_raw);
                self.segment_start = Some(raw_start);
                self.last_emit_raw = raw_start;
            }

            if let Some(end_sample) = end_event {
                let raw_end = self
                    .vad_to_raw_index(end_sample)
                    .saturating_add(self.speech_pad_raw);
                self.pending_end = Some(raw_end);
            }

            if let Some(iter_state) = self.iter_state.as_ref() {
                trace!(
                    "VAD index sync: vad_current={} raw_cursor={} mapped={}",
                    iter_state.current_sample,
                    self.raw_cursor,
                    self.vad_to_raw_index(iter_state.current_sample)
                );
            }

            if let SpeechMode::Stream {
                chunk_limit,
                overlap_size,
            } = self.mode
                && self.segment_start.is_some()
                && self.pending_end.is_none()
                && self.raw_cursor.saturating_sub(self.last_emit_raw) >= chunk_limit
            {
                let end = self.raw_cursor;
                if let Some(chunk) = self.raw_slice(self.last_emit_raw, end) {
                    events.push(SpeechEvent::Chunk(chunk));
                }
                if overlap_size > 0 {
                    self.last_emit_raw = end.saturating_sub(overlap_size);
                } else {
                    self.last_emit_raw = end;
                }
            }
        }

        if let Some(end) = self.pending_end
            && self.raw_cursor >= end
        {
            if let Some(start) = self.segment_start.take()
                && let Some(chunk) = self.raw_slice(start, end)
            {
                match self.mode {
                    SpeechMode::Stream { .. } => events.push(SpeechEvent::Chunk(chunk)),
                    SpeechMode::Utterance { .. } => events.push(SpeechEvent::Utterance(chunk)),
                }
            }
            self.pending_end = None;
            self.last_emit_raw = end;
            self.trim_raw_buffer(end.saturating_sub(self.pre_roll_raw));
        }

        events
    }

    fn flush(&mut self) -> Option<SpeechEvent> {
        if self.gate_mode == VadGateMode::Supervisor {
            if let Some(start) = self.segment_start.take() {
                // VAD fired Start but recording ended before End — emit what we have.
                let end = self.pending_end.take().unwrap_or(self.raw_cursor);
                let end = end.min(self.raw_cursor);
                self.last_emit_raw = end;
                if let Some(chunk) = self.raw_slice(start, end) {
                    debug!(
                        "Supervisor flush: open segment {}..{} ({} samples)",
                        start,
                        end,
                        chunk.len()
                    );
                    return Some(match self.mode {
                        SpeechMode::Stream { .. } => SpeechEvent::Chunk(chunk),
                        SpeechMode::Utterance { .. } => SpeechEvent::Utterance(chunk),
                    });
                }
            }
            // VAD never triggered Start — no speech detected, intentionally drop.
            return None;
        }
        if self.pending_samples.is_empty() {
            return None;
        }
        Some(self.emit_final())
    }

    fn emit_chunk(&mut self) -> SpeechEvent {
        let chunk = std::mem::take(&mut self.pending_samples);
        if let SpeechMode::Stream { overlap_size, .. } = self.mode
            && overlap_size > 0
            && chunk.len() > overlap_size
        {
            let start = chunk.len() - overlap_size;
            self.pending_samples.extend_from_slice(&chunk[start..]);
        }
        self.last_append_at = Instant::now();
        SpeechEvent::Chunk(chunk)
    }

    fn emit_final(&mut self) -> SpeechEvent {
        let chunk = std::mem::take(&mut self.pending_samples);
        self.pending_speech.clear();
        self.pending_silence.clear();
        self.speech_samples = 0;
        self.silence_samples = 0;
        self.in_speech = false;
        self.pre_roll.clear();
        self.iter_speech_start = None;
        if let Some(iter_state) = self.iter_state.as_mut() {
            iter_state.reset();
        }
        self.last_append_at = Instant::now();
        match self.mode {
            SpeechMode::Stream { .. } => SpeechEvent::Chunk(chunk),
            SpeechMode::Utterance { .. } => SpeechEvent::Utterance(chunk),
        }
    }

    fn gate_with_prob(&mut self, audio: &[f32], speech_prob: f32) -> GateDecision {
        let is_speech = speech_prob >= self.threshold;

        if self.in_speech {
            if speech_prob >= self.neg_threshold {
                self.silence_samples = 0;
                if !self.pending_silence.is_empty() {
                    let mut out = Vec::with_capacity(self.pending_silence.len() + audio.len());
                    out.append(&mut self.pending_silence);
                    out.extend_from_slice(audio);
                    return GateDecision {
                        audio: Some(out),
                        ended: false,
                    };
                }
                return GateDecision {
                    audio: Some(audio.to_vec()),
                    ended: false,
                };
            }

            self.pending_silence.extend_from_slice(audio);
            self.silence_samples = self.silence_samples.saturating_add(audio.len());
            if self.silence_samples >= self.min_silence_samples {
                self.in_speech = false;
                self.silence_samples = 0;
                if self.speech_pad_samples > 0 && !self.pending_silence.is_empty() {
                    let pad = self
                        .pending_silence
                        .drain(..self.speech_pad_samples.min(self.pending_silence.len()))
                        .collect::<Vec<_>>();
                    if !pad.is_empty() {
                        self.pending_samples.extend_from_slice(&pad);
                    }
                }
                self.pending_silence.clear();
                self.speech_samples = 0;
                self.pending_speech.clear();
                return GateDecision {
                    audio: None,
                    ended: true,
                };
            }
            return GateDecision {
                audio: None,
                ended: false,
            };
        }

        if is_speech {
            self.pending_speech.extend_from_slice(audio);
            self.speech_samples = self.speech_samples.saturating_add(audio.len());
            if self.speech_samples >= self.min_speech_samples {
                self.in_speech = true;
                self.speech_samples = 0;
                let mut out = Vec::new();
                if self.pre_roll_samples > 0 && !self.pre_roll.is_empty() {
                    out.extend(self.pre_roll.drain(..));
                }
                out.extend(std::mem::take(&mut self.pending_speech));
                return GateDecision {
                    audio: Some(out),
                    ended: false,
                };
            }
            return GateDecision {
                audio: None,
                ended: false,
            };
        }

        self.pending_speech.clear();
        self.speech_samples = 0;
        self.push_pre_roll(audio);
        GateDecision {
            audio: None,
            ended: false,
        }
    }

    fn gate_with_iter(&mut self, audio: &[f32], speech_prob: f32) -> GateDecision {
        let Some(iter_state) = self.iter_state.as_mut() else {
            return self.gate_with_prob(audio, speech_prob);
        };

        let was_triggered = iter_state.triggered();
        let event = iter_state.update(speech_prob);
        let is_triggered = iter_state.triggered();

        if !is_triggered {
            self.push_pre_roll(audio);
        } else {
            if !was_triggered {
                if let VadIterEvent::Start { start_sample } = event {
                    self.iter_speech_start = Some(start_sample);
                } else {
                    self.iter_speech_start = Some(iter_state.current_speech_start());
                }
                if self.pre_roll_samples > 0 && !self.pre_roll.is_empty() {
                    self.pending_samples.extend(self.pre_roll.drain(..));
                }
            }
            self.pending_samples.extend_from_slice(audio);
        }

        if let VadIterEvent::End { end_sample } = event {
            if let Some(start_sample) = self.iter_speech_start.take() {
                let speech_len = end_sample.saturating_sub(start_sample);
                let mut target_len = self
                    .pre_roll_samples
                    .saturating_add(speech_len)
                    .saturating_add(self.speech_pad_samples);
                if target_len == 0 {
                    target_len = self.pending_samples.len();
                }
                if self.pending_samples.len() > target_len {
                    self.pending_samples.truncate(target_len);
                }
            }
            self.in_speech = false;
            self.speech_samples = 0;
            self.silence_samples = 0;
            self.pending_speech.clear();
            self.pending_silence.clear();
            return GateDecision {
                audio: None,
                ended: true,
            };
        }

        GateDecision {
            audio: None,
            ended: false,
        }
    }

    fn push_pre_roll(&mut self, audio: &[f32]) {
        if self.pre_roll_samples == 0 {
            return;
        }
        for &sample in audio {
            if self.pre_roll.len() >= self.pre_roll_samples {
                self.pre_roll.pop_front();
            }
            self.pre_roll.push_back(sample);
        }
    }

    fn output_sample_rate(&self) -> u32 {
        self.output_sample_rate
    }

    fn vad_to_raw_index(&self, vad_index: usize) -> usize {
        if self.raw_sample_rate == 0 {
            return vad_index;
        }
        ((vad_index as f32 * self.raw_sample_rate as f32) / vad::VAD_SAMPLE_RATE as f32)
            .round()
            .max(0.0) as usize
    }

    fn raw_slice(&self, start: usize, end: usize) -> Option<Vec<f32>> {
        if end <= start {
            return None;
        }
        if start < self.raw_buffer_start || end > self.raw_cursor {
            return None;
        }
        let start_idx = start - self.raw_buffer_start;
        let end_idx = end - self.raw_buffer_start;
        if end_idx <= start_idx {
            return None;
        }
        Some(
            self.raw_buffer
                .iter()
                .skip(start_idx)
                .take(end_idx.saturating_sub(start_idx))
                .cloned()
                .collect(),
        )
    }

    fn trim_raw_buffer(&mut self, keep_from: usize) {
        if keep_from <= self.raw_buffer_start {
            return;
        }
        let drop = keep_from - self.raw_buffer_start;
        for _ in 0..drop.min(self.raw_buffer.len()) {
            self.raw_buffer.pop_front();
        }
        self.raw_buffer_start = keep_from;
    }
}

fn hardcoded_gate_config() -> GateConfig {
    // IMPORTANT:
    // Use the shared VAD config (env-driven) so tray presets and ~/.codescribe/.env
    // actually affect segmentation. Hardcoding short silence windows (e.g. 0.2s)
    // causes ultra-fragmented utterances and garbage/duplicated transcripts.
    let vad_cfg = vad::VadConfig::default();
    let speech_pad_sec = std::env::var("CODESCRIBE_VAD_SPEECH_PAD_SEC")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(0.064)
        .clamp(0.0, 2.0);

    GateConfig {
        // Single source of truth for thresholds/silence/utterance limits.
        // Reads env vars like CODESCRIBE_VAD_* (including our tray presets).
        vad: vad_cfg.clone(),
        // Pre-roll defaults to VadConfig (env-driven).
        pre_roll_sec: vad_cfg.pre_roll_sec,
        // Extra padding after speech end (tunable separately).
        speech_pad_sec,
        mode: gate_mode_from_env(),
    }
}

fn init_silero_vad(sample_rate: u32, config: &vad::VadConfig) -> Option<vad::SileroVad> {
    let model_path = vad::default_model_path();
    match vad::SileroVad::new(&model_path, config.clone()) {
        Ok(mut vad) => {
            vad.set_input_sample_rate(sample_rate);
            info!("Silero VAD ready (model: {})", model_path.display());
            Some(vad)
        }
        Err(e) => {
            warn!("Silero VAD init failed ({}): {}", model_path.display(), e);
            None
        }
    }
}

struct GateDecision {
    audio: Option<Vec<f32>>,
    ended: bool,
}

#[derive(Debug)]
struct VadIterState {
    params: VadIterParams,
    current_sample: usize,
    temp_end: usize,
    next_start: usize,
    prev_end: usize,
    triggered: bool,
    speech_start: usize,
}

#[derive(Debug)]
struct VadIterParams {
    threshold: f32,
    min_silence_samples: usize,
    min_speech_samples: usize,
    max_speech_samples: f32,
    frame_size_samples: usize,
    min_silence_samples_at_max_speech: usize,
}

#[derive(Debug, Copy, Clone)]
enum VadIterEvent {
    None,
    Start { start_sample: usize },
    End { end_sample: usize },
}

impl VadIterState {
    fn new(config: &GateConfig, sample_rate: u32) -> Self {
        let sr_per_ms = sample_rate as f32 / 1000.0;
        let frame_size_samples = vad::CHUNK_SIZE;
        let min_silence_samples = (config.vad.max_silence_duration_sec * sample_rate as f32)
            .round()
            .max(1.0) as usize;
        let min_speech_samples = (config.vad.min_speech_duration_sec * sample_rate as f32)
            .round()
            .max(1.0) as usize;
        let speech_pad_samples = (config.speech_pad_sec * sample_rate as f32)
            .round()
            .max(0.0) as usize;
        let max_speech_samples = config.vad.max_utterance_sec * sample_rate as f32
            - frame_size_samples as f32
            - 2.0 * speech_pad_samples as f32;
        let min_silence_samples_at_max_speech = (sr_per_ms * 98.0).round() as usize;

        Self {
            params: VadIterParams {
                threshold: config.vad.threshold,
                min_silence_samples,
                min_speech_samples,
                max_speech_samples,
                frame_size_samples,
                min_silence_samples_at_max_speech,
            },
            current_sample: 0,
            temp_end: 0,
            next_start: 0,
            prev_end: 0,
            triggered: false,
            speech_start: 0,
        }
    }

    fn reset(&mut self) {
        self.current_sample = 0;
        self.temp_end = 0;
        self.next_start = 0;
        self.prev_end = 0;
        self.triggered = false;
        self.speech_start = 0;
    }

    fn triggered(&self) -> bool {
        self.triggered
    }

    fn current_speech_start(&self) -> usize {
        self.speech_start
    }

    fn update(&mut self, speech_prob: f32) -> VadIterEvent {
        self.current_sample = self
            .current_sample
            .saturating_add(self.params.frame_size_samples);
        let frame_start = self
            .current_sample
            .saturating_sub(self.params.frame_size_samples);

        if speech_prob > self.params.threshold {
            if self.temp_end != 0 {
                self.temp_end = 0;
                if self.next_start < self.prev_end {
                    self.next_start = frame_start;
                }
            }
            if !self.triggered {
                self.triggered = true;
                self.speech_start = frame_start;
                return VadIterEvent::Start {
                    start_sample: frame_start,
                };
            }
            return VadIterEvent::None;
        }

        if self.triggered
            && (self.current_sample.saturating_sub(self.speech_start) as f32)
                > self.params.max_speech_samples
        {
            if self.prev_end > 0 {
                let end = self.prev_end;
                if self.next_start < self.prev_end {
                    self.triggered = false;
                } else {
                    self.speech_start = self.next_start;
                }
                self.prev_end = 0;
                self.next_start = 0;
                self.temp_end = 0;
                return VadIterEvent::End { end_sample: end };
            }

            let end = self.current_sample;
            self.triggered = false;
            self.prev_end = 0;
            self.next_start = 0;
            self.temp_end = 0;
            return VadIterEvent::End { end_sample: end };
        }

        let neg_threshold = (self.params.threshold - 0.15).max(0.05);
        if self.triggered && speech_prob < neg_threshold {
            if self.temp_end == 0 {
                self.temp_end = self.current_sample;
            }
            if self.current_sample.saturating_sub(self.temp_end)
                > self.params.min_silence_samples_at_max_speech
            {
                self.prev_end = self.temp_end;
            }
            if self.current_sample.saturating_sub(self.temp_end) >= self.params.min_silence_samples
            {
                let end = self.temp_end;
                if end.saturating_sub(self.speech_start) > self.params.min_speech_samples {
                    self.triggered = false;
                    self.prev_end = 0;
                    self.next_start = 0;
                    self.temp_end = 0;
                    return VadIterEvent::End { end_sample: end };
                }
            }
        }

        VadIterEvent::None
    }
}

struct TranscriptionPipeline {
    language: Option<String>,
    postprocessor: StreamPostProcessor,
}

impl TranscriptionPipeline {
    fn new(language: Option<String>) -> Self {
        Self {
            language,
            postprocessor: StreamPostProcessor::new(),
        }
    }

    fn postprocess(&mut self, text: &str) -> Option<String> {
        self.postprocessor.process_utterance(text)
    }
}

struct BufferedEmitter {
    queue: VecDeque<String>,
    initial_delay_ms: u64,
    typing_speed_cps: f32,
    emit_words_max: usize,
    first_output_at: Option<Instant>,
    current_segment: Option<String>,
    current_tokens: Vec<String>,
    current_token_index: usize,
    delta_callback: Option<StreamDeltaCallback>,
    transcript_buffer: Arc<Mutex<String>>,
    stream_log_path: Option<std::path::PathBuf>,
    finished: bool,
    has_output: bool,
}

impl BufferedEmitter {
    fn new(
        transcript_buffer: Arc<Mutex<String>>,
        delta_callback: Option<StreamDeltaCallback>,
        stream_log_path: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            queue: VecDeque::new(),
            initial_delay_ms: env_u64("CODESCRIBE_BUFFER_DELAY_MS", DEFAULT_BUFFER_DELAY_MS),
            // In buffered mode we emit "tokens" (word-level) instead of chars.
            // CODESCRIBE_TYPING_CPS is interpreted as tokens-per-second.
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
        }
    }

    fn push_segment(&mut self, text: String) {
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

    async fn tick(&mut self) -> bool {
        if self.finished && self.queue.is_empty() && self.current_segment.is_none() {
            return true;
        }

        if self.is_buffering() {
            return false;
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
            {
                let mut buffer = self.transcript_buffer.lock().await;
                apply_delta_to_string(&mut buffer, &delta);
            }

            if let Some(callback) = &self.delta_callback {
                callback(&delta);
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

    fn finish(&mut self) {
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

async fn emitter_tick_loop(emitter: Arc<Mutex<BufferedEmitter>>) {
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

async fn buffered_transcription_worker(
    mut chunk_receiver: mpsc::Receiver<Vec<f32>>,
    transcript_buffer: Arc<Mutex<String>>,
    sample_rate: u32,
    language: Option<String>,
    delta_callback: Option<StreamDeltaCallback>,
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

    while let Some(data) = chunk_receiver.recv().await {
        for event in session.feed(&data, sample_rate) {
            if let SpeechEvent::Utterance(utterance) = event
                && let Err(e) = handle_utterance(
                    utterance,
                    session.output_sample_rate(),
                    &mut pipeline,
                    &emitter,
                )
                .await
            {
                error!("Buffered transcription failed: {}", e);
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
) -> Result<()> {
    if utterance.is_empty() {
        return Ok(());
    }

    let language = pipeline.language.clone();
    let raw_text = tokio::task::spawn_blocking(move || {
        whisper::transcribe(&utterance, sample_rate, language.as_deref())
    })
    .await??;

    if let Some(cleaned) = pipeline.postprocess(&raw_text) {
        let mut guard = emitter.lock().await;
        guard.push_segment(cleaned);
    }

    Ok(())
}

async fn process_chunk(
    samples: &[f32],
    transcript_buffer: &Arc<Mutex<String>>,
    sample_rate: u32,
    language: Option<&str>,
    mut postprocessor: Option<&mut StreamPostProcessor>,
    delta_callback: Option<&StreamDeltaCallback>,
    stream_log_path: Option<&Path>,
) {
    if samples.is_empty() {
        return;
    }

    let samples_owned = samples.to_vec();
    let lang_owned = language.map(String::from);

    // Run in blocking task
    let result = tokio::task::spawn_blocking(move || {
        let engine_mutex = match get_engine() {
            Ok(m) => m,
            Err(e) => return Err(anyhow!("Engine error: {}", e)),
        };

        let mut engine_guard = match engine_mutex.lock() {
            Ok(g) => g,
            Err(e) => return Err(anyhow!("Lock error: {}", e)),
        };

        // If sample_rate is not 16k, engine handles resampling?
        // transcribe_samples_16k expects 16k.
        // But our Recorder is configured for 16k (SAMPLE_RATE constant).
        // However, Recorder might use native rate.
        // Recorder::start() sets actual_sample_rate.
        // If actual_sample_rate != 16k, we need to resample.
        // Current implementation passes raw samples.
        // transcribe_samples_16k assumes 16k.
        // transcribe_with_language handles resampling.

        // Wait, engine.transcribe_samples_16k is specific.
        // engine.transcribe_with_language(audio, sample_rate, language) handles everything.
        // Let's use that one to be safe, or check if we need 16k.

        // The plan says "transcribe_samples_16k() - transcribes raw f32, zero I/O".
        // If we use transcribe_with_language, it calls transcribe_long_with_language -> detect_language -> ...
        // transcribe_samples_16k is lower level.

        // If sample_rate is 16000, we can use transcribe_samples_16k directly?
        // Yes, but we should be robust.
        // Let's use transcribe_with_language which handles resampling if needed.
        // It's safer.

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
                    normalize_whitespace_basic(&text)
                };

                if let Some(cleaned) = cleaned {
                    let mut buffer = transcript_buffer.lock().await;
                    let before = buffer.clone();
                    append_with_overlap_dedup(&mut buffer, &cleaned);
                    if let Some(delta) = build_redacted_delta(&before, &buffer) {
                        let has_effect =
                            delta.chars().any(|c| c == '\u{0008}' || !c.is_whitespace());
                        if has_effect {
                            if let Some(callback) = delta_callback {
                                callback(&delta);
                            }

                            // Log to file if enabled
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

fn normalize_whitespace_basic(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    let cleaned = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

fn stream_log_path() -> Option<std::path::PathBuf> {
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

fn build_redacted_delta(before: &str, after: &str) -> Option<String> {
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

fn apply_delta_to_string(target: &mut String, delta: &str) {
    for ch in delta.chars() {
        if ch == '\u{0008}' {
            target.pop();
        } else {
            target.push(ch);
        }
    }
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

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(default)
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

fn env_bool(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn env_bool_default(key: &str, default: bool) -> bool {
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

fn gate_mode_from_env() -> VadGateMode {
    if env_bool("CODESCRIBE_VAD_ITER") {
        return VadGateMode::Iter;
    }
    match std::env::var("CODESCRIBE_VAD_GATE_MODE")
        .ok()
        .map(|v| v.to_lowercase())
        .as_deref()
    {
        Some("supervisor") | Some("quality") | Some("managed") => VadGateMode::Supervisor,
        Some("iter") | Some("vad_iter") | Some("silero_iter") => VadGateMode::Iter,
        Some("simple") | Some("gate") | Some("basic") => VadGateMode::Simple,
        _ => VadGateMode::Supervisor,
    }
}

fn stream_chunk_duration_sec() -> f32 {
    env_f32("CODESCRIBE_STREAM_CHUNK_SEC", DEFAULT_CHUNK_DURATION_SEC).clamp(0.5, 30.0)
}

fn stream_overlap_sec(chunk_duration_sec: f32) -> f32 {
    let ratio = env_f32("CODESCRIBE_STREAM_OVERLAP_RATIO", DEFAULT_OVERLAP_RATIO).clamp(0.05, 0.8);
    (chunk_duration_sec * ratio).min(chunk_duration_sec * 0.8)
}

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
                append_with_overlap_dedup(&mut out, &cleaned);
            }
        } else {
            append_with_overlap_dedup(&mut out, &text);
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

// Note: calculate_rms_db removed - now using vad::speech_probability for voice detection

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::load_audio_file;
    use crate::stt::whisper;
    use serial_test::serial;
    use std::fs;
    use std::path::{Path, PathBuf};

    #[test]
    #[ignore] // Manual: requires microphone + Silero model (set CODESCRIBE_E2E_MIC=1)
    fn test_vad_gate_live_chunk_sizes() {
        if !env_bool("CODESCRIBE_E2E_MIC") {
            eprintln!("Skipping mic gate test (set CODESCRIBE_E2E_MIC=1 to enable)");
            return;
        }

        let model_path = vad::default_model_path();
        if !model_path.exists() {
            eprintln!(
                "Skipping: Silero VAD model not found at {}",
                model_path.display()
            );
            return;
        }

        let record_sec = env_f32("CODESCRIBE_E2E_MIC_SEC", 6.0).max(2.0);
        println!("Speak now for ~{:.1}s...", record_sec);

        let mut recorder = Recorder::new().expect("Failed to create recorder");
        let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
        let wav_path = rt
            .block_on(async {
                recorder.start().await.expect("Failed to start recorder");
                tokio::time::sleep(Duration::from_secs_f32(record_sec)).await;
                recorder.stop().await.expect("Failed to stop recorder")
            })
            .expect("No WAV produced");

        let (samples, sample_rate) =
            load_audio_file(&wav_path).expect("Failed to load recorded audio");

        let mut resampler = vad::Resampler::new(sample_rate);
        let samples_16k = resampler.resample(&samples);
        let chunk_sec = 4.0f32;
        let chunk_limit = (vad::VAD_SAMPLE_RATE as f32 * chunk_sec) as usize;

        let cases = [
            ("lt", chunk_limit / 2),
            ("eq", chunk_limit),
            ("gt", chunk_limit * 2),
        ];

        for (label, block_len) in cases {
            let mut session = SpeechSession::new_stream(vad::VAD_SAMPLE_RATE, chunk_sec, 0.0);
            let mut chunk_events = 0usize;
            let mut idx = 0usize;
            while idx < samples_16k.len() {
                let end = (idx + block_len).min(samples_16k.len());
                let slice = &samples_16k[idx..end];
                for event in session.feed(slice, vad::VAD_SAMPLE_RATE) {
                    if matches!(event, SpeechEvent::Chunk(_)) {
                        chunk_events += 1;
                    }
                }
                idx = end;
            }
            if let Some(SpeechEvent::Chunk(_)) = session.flush() {
                chunk_events += 1;
            }

            assert!(
                chunk_events > 0,
                "Expected at least one chunk for case {} (block_len={})",
                label,
                block_len
            );
        }

        let _ = fs::remove_file(&wav_path);
    }

    #[test]
    #[serial]
    fn test_stream_postprocess_corpus_pairs() {
        if !env_bool("CODESCRIBE_E2E_CORPUS") {
            eprintln!("Skipping corpus E2E (set CODESCRIBE_E2E_CORPUS=1 to enable)");
            return;
        }

        let corpus_dir = corpus_root();
        let date_filter = std::env::var("CODESCRIBE_E2E_CORPUS_DATE").ok();
        let limit = env_usize("CODESCRIBE_E2E_CORPUS_LIMIT", 3);
        let max_regression = env_f32("CODESCRIBE_E2E_CORPUS_MAX_REGRESSION", 0.05);

        let pairs = collect_pairs(&corpus_dir, date_filter.as_deref(), limit);
        if pairs.is_empty() {
            eprintln!("No WAV+TXT pairs found in {}", corpus_dir.to_string_lossy());
            return;
        }

        whisper::init().expect("Failed to init Whisper");
        let language = std::env::var("CODESCRIBE_E2E_CORPUS_LANGUAGE").ok();
        let mut failures = Vec::new();
        let mut total_raw_wer = 0.0;
        let mut total_post_wer = 0.0;
        let mut total_raw_cer = 0.0;
        let mut total_post_cer = 0.0;
        let mut processed = 0usize;

        for (wav_path, txt_path) in pairs {
            let reference = fs::read_to_string(&txt_path)
                .unwrap_or_else(|_| String::new())
                .trim()
                .to_string();
            if reference.is_empty() {
                eprintln!("Skipping empty reference: {}", txt_path.display());
                continue;
            }

            let (samples, sample_rate) = load_audio_file(&wav_path).expect("Failed to load audio");

            let raw =
                transcribe_streaming_samples(&samples, sample_rate, language.as_deref(), None)
                    .expect("Raw streaming transcription failed");
            let mut postprocessor = StreamPostProcessor::new();
            let post = transcribe_streaming_samples(
                &samples,
                sample_rate,
                language.as_deref(),
                Some(&mut postprocessor),
            )
            .expect("Post streaming transcription failed");

            let (ref_tokens, ref_norm) = normalize_for_eval(&reference);
            let (raw_tokens, raw_norm) = normalize_for_eval(&raw);
            let (post_tokens, post_norm) = normalize_for_eval(&post);

            let wer_raw = word_error_rate(&ref_tokens, &raw_tokens);
            let wer_post = word_error_rate(&ref_tokens, &post_tokens);
            let cer_raw = char_error_rate(&ref_norm, &raw_norm);
            let cer_post = char_error_rate(&ref_norm, &post_norm);

            processed += 1;
            total_raw_wer += wer_raw;
            total_post_wer += wer_post;
            total_raw_cer += cer_raw;
            total_post_cer += cer_post;

            println!(
                "Corpus: {}\n  WER raw={:.3} post={:.3} (Δ={:.3})\n  CER raw={:.3} post={:.3} (Δ={:.3})",
                wav_path.file_name().unwrap_or_default().to_string_lossy(),
                wer_raw,
                wer_post,
                wer_post - wer_raw,
                cer_raw,
                cer_post,
                cer_post - cer_raw,
            );

            if wer_post > wer_raw + max_regression {
                failures.push(format!(
                    "{}: WER regression {:.3} > {:.3}",
                    wav_path.display(),
                    wer_post - wer_raw,
                    max_regression
                ));
            }
        }

        if processed > 0 {
            let denom = processed as f32;
            let avg_raw_wer = total_raw_wer / denom;
            let avg_post_wer = total_post_wer / denom;
            let avg_raw_cer = total_raw_cer / denom;
            let avg_post_cer = total_post_cer / denom;

            println!(
                "Average WER raw={:.3} post={:.3} | CER raw={:.3} post={:.3}",
                avg_raw_wer, avg_post_wer, avg_raw_cer, avg_post_cer
            );
        }

        if !failures.is_empty() {
            panic!(
                "Corpus postprocess regressions detected:\n{}",
                failures.join("\n")
            );
        }
    }

    fn env_bool(key: &str) -> bool {
        std::env::var(key)
            .ok()
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    }

    fn env_f32(key: &str, default: f32) -> f32 {
        std::env::var(key)
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(default)
    }

    fn env_usize(key: &str, default: usize) -> usize {
        std::env::var(key)
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(default)
    }

    fn corpus_root() -> PathBuf {
        if let Ok(dir) = std::env::var("CODESCRIBE_E2E_CORPUS_DIR") {
            return PathBuf::from(shellexpand::tilde(&dir).into_owned());
        }

        crate::config::Config::config_dir().join("transcriptions")
    }

    fn collect_pairs(
        root: &Path,
        date_filter: Option<&str>,
        limit: usize,
    ) -> Vec<(PathBuf, PathBuf)> {
        let mut pairs = Vec::new();
        if !root.exists() {
            return pairs;
        }

        let mut subdirs = Vec::new();
        if let Some(date) = date_filter {
            let dir = root.join(date);
            if dir.exists() {
                subdirs.push(dir);
            }
        } else if let Ok(entries) = fs::read_dir(root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    subdirs.push(path);
                }
            }
        }

        subdirs.sort();

        for dir in subdirs {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            let mut wavs = Vec::new();
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("wav") {
                    wavs.push(path);
                }
            }

            wavs.sort();
            for wav in wavs {
                let stem = match wav.file_stem().and_then(|s| s.to_str()) {
                    Some(stem) => stem,
                    None => continue,
                };
                let txt = wav.with_file_name(format!("{stem}.txt"));
                if txt.exists() {
                    pairs.push((wav, txt));
                }
            }
        }

        if limit > 0 && pairs.len() > limit {
            let start = pairs.len() - limit;
            pairs = pairs[start..].to_vec();
        }

        pairs
    }

    fn normalize_for_eval(text: &str) -> (Vec<String>, String) {
        let mut normalized = String::with_capacity(text.len());
        for ch in text.to_lowercase().chars() {
            if ch.is_alphanumeric() || ch.is_whitespace() {
                normalized.push(ch);
            } else {
                normalized.push(' ');
            }
        }
        let tokens: Vec<String> = normalized
            .split_whitespace()
            .map(|t| t.to_string())
            .collect();
        let normalized = tokens.join(" ");
        (tokens, normalized)
    }

    fn word_error_rate(reference: &[String], hypothesis: &[String]) -> f32 {
        let dist = levenshtein(reference, hypothesis);
        let denom = reference.len().max(1) as f32;
        dist as f32 / denom
    }

    fn char_error_rate(reference: &str, hypothesis: &str) -> f32 {
        let ref_chars: Vec<char> = reference.chars().collect();
        let hyp_chars: Vec<char> = hypothesis.chars().collect();
        let dist = levenshtein(&ref_chars, &hyp_chars);
        let denom = ref_chars.len().max(1) as f32;
        dist as f32 / denom
    }

    #[test]
    fn test_vad_index_sync_no_drift() {
        // Simulate 100 cpal callbacks of ~1024 samples @ 48kHz.
        // After resampling to 16kHz each callback yields ~341 samples,
        // which is NOT a multiple of CHUNK_SIZE (512).
        // The accumulation buffer must prevent index drift.
        let input_sr = 48000u32;
        let callback_size = 1024usize;
        let num_callbacks = 100usize;

        let mut session = SpeechSession::new_stream(input_sr, 15.0, 0.0);

        // Only run if gate mode is Supervisor (default).
        if session.gate_mode != VadGateMode::Supervisor {
            eprintln!("Skipping: gate mode is not Supervisor");
            return;
        }

        // Feed synthetic audio (sine wave so VAD might trigger).
        let freq = 440.0f32;
        let mut phase = 0.0f32;
        let phase_inc = 2.0 * std::f32::consts::PI * freq / input_sr as f32;

        for _ in 0..num_callbacks {
            let mut buf = Vec::with_capacity(callback_size);
            for _ in 0..callback_size {
                buf.push(phase.sin() * 0.5);
                phase += phase_inc;
            }
            let _ = session.feed(&buf, input_sr);
        }

        // After all callbacks, check index alignment.
        let total_raw = num_callbacks * callback_size;
        assert_eq!(
            session.raw_cursor, total_raw,
            "raw_cursor should equal total input samples"
        );

        // If iter_state exists, verify mapped index is close to raw_cursor.
        if let Some(ref iter_state) = session.iter_state {
            let mapped = session.vad_to_raw_index(iter_state.current_sample);
            let drift = if mapped > session.raw_cursor {
                mapped - session.raw_cursor
            } else {
                session.raw_cursor - mapped
            };
            // Tolerance: one CHUNK_SIZE worth of raw samples (~1536 @ 48kHz for 512 @ 16kHz).
            let tolerance =
                ((vad::CHUNK_SIZE as f32 * input_sr as f32) / vad::VAD_SAMPLE_RATE as f32) as usize;
            assert!(
                drift <= tolerance,
                "VAD index drift too large: mapped={} raw_cursor={} drift={} tolerance={}",
                mapped,
                session.raw_cursor,
                drift,
                tolerance
            );
        }

        // Verify residual buffer is smaller than one full frame.
        assert!(
            session.vad_resample_buf.len() < vad::CHUNK_SIZE,
            "Residual buffer should be < CHUNK_SIZE, got {}",
            session.vad_resample_buf.len()
        );
    }

    #[test]
    fn test_vad_supervisor_segments_real_audio() {
        // Load real WAV files and run VAD directly to check speech_prob values.
        let corpus_dir =
            std::path::PathBuf::from(shellexpand::tilde("~/.codescribe/transcriptions").as_ref());
        if !corpus_dir.exists() {
            eprintln!("Skipping: no transcriptions dir");
            return;
        }

        // Find one WAV
        let mut wav_path: Option<std::path::PathBuf> = None;
        let mut dirs: Vec<_> = fs::read_dir(&corpus_dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_dir())
            .collect();
        dirs.sort_by_key(|e| e.file_name());
        dirs.reverse();
        'outer: for dir in &dirs {
            if let Ok(entries) = fs::read_dir(dir.path()) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.extension().and_then(|s| s.to_str()) == Some("wav") {
                        wav_path = Some(p);
                        break 'outer;
                    }
                }
            }
        }
        let wav_path = match wav_path {
            Some(p) => p,
            None => {
                eprintln!("Skipping: no WAV files");
                return;
            }
        };

        let (samples, sample_rate) = load_audio_file(&wav_path).expect("load WAV");
        let audio_sec = samples.len() as f32 / sample_rate as f32;
        println!(
            "Testing: {} ({:.1}s @ {}Hz)",
            wav_path.file_name().unwrap_or_default().to_string_lossy(),
            audio_sec,
            sample_rate,
        );

        // Step 1: Raw VAD probe — resample to 16kHz, run Silero directly.
        let vad_config = vad::VadConfig {
            threshold: 0.50,
            min_speech_duration_sec: 0.05,
            max_silence_duration_sec: 0.20,
            max_utterance_sec: 300.0,
            pre_roll_sec: 0.064,
        };
        let model_path = vad::default_model_path();
        if !model_path.exists() {
            eprintln!("Skipping: no Silero model at {}", model_path.display());
            return;
        }

        let mut silero = vad::SileroVad::new(&model_path, vad_config).expect("load Silero");
        let mut resampler = vad::Resampler::new(sample_rate);

        // Resample entire file, then run frame-by-frame
        let samples_16k = resampler.resample(&samples);
        let mut probs = Vec::new();
        let mut max_prob = 0.0f32;
        let mut above_threshold = 0usize;

        for chunk in samples_16k.chunks(vad::CHUNK_SIZE) {
            if chunk.len() < vad::CHUNK_SIZE {
                break;
            }
            let prob = silero.predict(chunk).unwrap_or(0.0);
            if prob > max_prob {
                max_prob = prob;
            }
            if prob >= 0.5 {
                above_threshold += 1;
            }
            probs.push(prob);
        }

        let total_frames = probs.len();
        println!(
            "  VAD direct: {} frames, max_prob={:.3}, above_0.5={}/{} ({:.0}%)",
            total_frames,
            max_prob,
            above_threshold,
            total_frames,
            if total_frames > 0 {
                above_threshold as f32 / total_frames as f32 * 100.0
            } else {
                0.0
            },
        );

        // Show first 20 prob values
        let show = probs
            .iter()
            .take(20)
            .map(|p| format!("{:.2}", p))
            .collect::<Vec<_>>()
            .join(" ");
        println!("  First 20 probs: {}", show);

        // Show some probs around the middle (where speech likely is)
        if probs.len() > 40 {
            let mid = probs.len() / 2;
            let show_mid = probs[mid..mid + 20.min(probs.len() - mid)]
                .iter()
                .map(|p| format!("{:.2}", p))
                .collect::<Vec<_>>()
                .join(" ");
            println!("  Mid probs [{}-{}]: {}", mid, mid + 20, show_mid);
        }

        // Step 2: Run through SpeechSession to check segment emission
        let callback_size = 1024usize;
        let mut session = SpeechSession::new_utterance(sample_rate);
        let mut events = Vec::new();
        let mut offset = 0usize;
        while offset < samples.len() {
            let end = (offset + callback_size).min(samples.len());
            for event in session.feed(&samples[offset..end], sample_rate) {
                events.push(event);
            }
            offset = end;
        }
        if let Some(event) = session.flush() {
            events.push(event);
        }

        let n_segments = events.len();
        let total_speech: usize = events
            .iter()
            .map(|e| match e {
                SpeechEvent::Utterance(s) | SpeechEvent::Chunk(s) => s.len(),
            })
            .sum();
        let speech_sec = total_speech as f32 / session.output_sample_rate() as f32;

        println!(
            "  Session: {} segments, {:.1}s speech, gate_mode={:?}",
            n_segments, speech_sec, session.gate_mode,
        );

        // Assertion: VAD should detect SOME speech in real audio
        assert!(
            max_prob >= 0.3,
            "Silero returned very low probs on real speech audio (max={:.3}). Model or context broken.",
            max_prob,
        );
    }

    fn levenshtein<T: Eq>(a: &[T], b: &[T]) -> usize {
        let mut prev: Vec<usize> = (0..=b.len()).collect();
        let mut cur = vec![0usize; b.len() + 1];

        for (i, item_a) in a.iter().enumerate() {
            cur[0] = i + 1;
            for (j, item_b) in b.iter().enumerate() {
                let cost = if item_a == item_b { 0 } else { 1 };
                cur[j + 1] =
                    std::cmp::min(std::cmp::min(prev[j + 1] + 1, cur[j] + 1), prev[j] + cost);
            }
            prev.clone_from(&cur);
        }

        prev[b.len()]
    }
}
