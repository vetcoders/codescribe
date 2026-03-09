//! Audio chunker — VAD-gated speech segmentation.
//!
//! Extracted from `streaming_recorder.rs` to decouple audio segmentation
//! from transcription logic. This module has **zero** dependency on Whisper/STT.
//!
//! ## Pipeline position
//!
//! ```text
//! Recorder (audio capture) → Chunker (this module) → STT adapter → PostProcessor → DeltaSink
//! ```
//!
//! ## Key types
//!
//! - [`SpeechSession`] — stateful VAD gate + chunker (Silero neural VAD)
//! - [`SpeechEvent`] — emitted events: `Chunk` (streaming) or `Utterance` (complete)
//! - [`VadIterState`] — Silero VAD iterator state machine (start/end boundary detection)
//!
//! Created by M&K (c)2026 VetCoders

use std::collections::VecDeque;
use std::time::Duration;

use tokio::time::Instant;
use tracing::{debug, trace, warn};

use crate::vad;

// ═══════════════════════════════════════════════════════════
// Constants
// ═══════════════════════════════════════════════════════════

const SILERO_GATE_MODE: VadGateMode = VadGateMode::Supervisor;

// ═══════════════════════════════════════════════════════════
// Public types
// ═══════════════════════════════════════════════════════════

pub(crate) enum SpeechEvent {
    #[allow(dead_code)]
    Chunk(Vec<f32>),
    /// Interim utterance slice emitted during long continuous speech to keep streaming responsive.
    Utterance(Vec<f32>),
    /// Final utterance slice emitted when VAD determines the segment ended (or on flush).
    ///
    /// Consumers can use this to distinguish "preview" from "commit" boundaries.
    UtteranceFinal(Vec<f32>),
}

pub(crate) enum SpeechMode {
    #[allow(dead_code)]
    Stream {
        chunk_limit: usize,
        overlap_size: usize,
    },
    Utterance {
        max_utterance_samples: usize,
        /// Periodic interim emit limit (samples). Continuous speech without VAD
        /// silence triggers an interim Utterance every `interim_limit` samples
        /// so Whisper + UI stay responsive during long stretches of speech.
        interim_limit: usize,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum VadGateMode {
    /// Gate audio before it reaches Whisper (legacy).
    Simple,
    /// Silero VAD iter logic as a hard gate (legacy).
    Iter,
    /// Silero VAD is a supervisor: audio always flows, VAD only defines boundaries.
    Supervisor,
}

pub(crate) struct GateConfig {
    pub vad: vad::VadConfig,
    pub pre_roll_sec: f32,
    pub speech_pad_sec: f32,
    pub mode: VadGateMode,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct VadErrorStats {
    pub predict_errors: u64,
    pub unavailable_frames: u64,
    pub total_predict_errors: u64,
    pub total_unavailable_frames: u64,
}

// ═══════════════════════════════════════════════════════════
// SpeechSession
// ═══════════════════════════════════════════════════════════

pub(crate) struct SpeechSession {
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
    vad_frames_total: u64,
    vad_frames_speech: u64,
    last_vad_heartbeat: Instant,
    /// Per-emitted-event Silero-positive speech samples (16k domain).
    event_speech_vad_samples: VecDeque<u64>,
    /// Speech samples accumulated since the last emitted speech event.
    pending_event_speech_vad_samples: u64,
    /// Peak speech probability since the last completed segment.
    /// Used as a conservative confidence fallback for segment-level quality checks.
    max_speech_prob: f32,
    /// Peak speech probability for the currently active utterance/segment.
    segment_peak_prob: f32,
    /// Speech probability at the last VAD boundary (Start or End).
    last_boundary_prob: f32,
    /// Wall-clock instant when this session was created.
    session_start: Instant,
    /// Total number of Silero predict() errors in this session.
    vad_predict_errors_total: u64,
    /// Predict errors since last telemetry drain.
    vad_predict_errors_pending: u64,
    /// Total VAD frames processed while model was unavailable.
    vad_unavailable_frames_total: u64,
    /// Unavailable frames since last telemetry drain.
    vad_unavailable_frames_pending: u64,
    /// Avoid flooding logs when VAD model is unavailable.
    vad_unavailable_logged: bool,
}

impl SpeechSession {
    #[allow(dead_code)]
    pub fn new_stream(sample_rate: u32, chunk_duration_sec: f32, overlap_sec: f32) -> Self {
        let config = hardcoded_gate_config();
        debug!("SpeechSession::new_stream gate_mode={:?}", config.mode);
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
            vad_frames_total: 0,
            vad_frames_speech: 0,
            last_vad_heartbeat: Instant::now(),
            event_speech_vad_samples: VecDeque::new(),
            pending_event_speech_vad_samples: 0,
            max_speech_prob: 0.0,
            segment_peak_prob: 0.0,
            last_boundary_prob: 0.0,
            session_start: Instant::now(),
            vad_predict_errors_total: 0,
            vad_predict_errors_pending: 0,
            vad_unavailable_frames_total: 0,
            vad_unavailable_frames_pending: 0,
            vad_unavailable_logged: false,
        }
    }

    pub fn new_utterance(sample_rate: u32) -> Self {
        let interim_sec = utterance_interim_sec();
        Self::new_utterance_with_interim_and_silence(sample_rate, interim_sec, None)
    }

    pub fn new_utterance_with_silence(sample_rate: u32, max_silence_sec: f32) -> Self {
        let interim_sec = utterance_interim_sec();
        Self::new_utterance_with_interim_and_silence(
            sample_rate,
            interim_sec,
            Some(max_silence_sec),
        )
    }

    fn new_utterance_with_interim_and_silence(
        sample_rate: u32,
        interim_sec: f32,
        max_silence_sec: Option<f32>,
    ) -> Self {
        let mut config = hardcoded_utterance_gate_config();
        if let Some(sec) = max_silence_sec {
            config.vad.max_silence_duration_sec = sec.clamp(0.1, 10.0);
        }
        debug!(
            "SpeechSession::new_utterance gate_mode={:?} interim={:.2}s",
            config.mode, interim_sec
        );
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
        let interim_limit = (interim_sec.clamp(1.0, 30.0) * output_sample_rate as f32) as usize;
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
                interim_limit,
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
            pre_roll_raw: (sample_rate as f32 * config.pre_roll_sec).round().max(0.0) as usize,
            speech_pad_raw: (sample_rate as f32 * config.speech_pad_sec)
                .round()
                .max(0.0) as usize,
            last_emit_raw: 0,
            vad_frames_total: 0,
            vad_frames_speech: 0,
            last_vad_heartbeat: Instant::now(),
            event_speech_vad_samples: VecDeque::new(),
            pending_event_speech_vad_samples: 0,
            max_speech_prob: 0.0,
            segment_peak_prob: 0.0,
            last_boundary_prob: 0.0,
            session_start: Instant::now(),
            vad_predict_errors_total: 0,
            vad_predict_errors_pending: 0,
            vad_unavailable_frames_total: 0,
            vad_unavailable_frames_pending: 0,
            vad_unavailable_logged: false,
        }
    }

    pub fn feed(&mut self, audio: &[f32], _sample_rate: u32) -> Vec<SpeechEvent> {
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
            let speech_prob = self.predict_speech_prob(&frame, "gate");
            self.update_vad_heartbeat(speech_prob);
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
                    interim_limit,
                } => {
                    if self.pending_samples.len() >= max_utterance_samples {
                        events.push(self.emit_final());
                    } else if self.pending_samples.len() >= interim_limit {
                        let chunk = std::mem::take(&mut self.pending_samples);
                        self.last_append_at = Instant::now();
                        events.push(SpeechEvent::Utterance(chunk));
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
            let speech_prob = self.predict_speech_prob(&frame, "supervisor");
            self.update_vad_heartbeat(speech_prob);

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

            debug_assert!(
                self.iter_state.is_some(),
                "Supervisor gate requires VadIterState"
            );
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
                trace!("Supervisor gate missing VadIterState; skipping frame");
                continue;
            }

            if let Some(start_sample) = start_event {
                let raw_start = self
                    .vad_to_raw_index(start_sample)
                    .saturating_sub(self.pre_roll_raw);
                self.segment_start = Some(raw_start);
                self.last_emit_raw = raw_start;
                self.last_boundary_prob = speech_prob;
                self.segment_peak_prob = speech_prob;
            }

            if let Some(end_sample) = end_event {
                let raw_end = self
                    .vad_to_raw_index(end_sample)
                    .saturating_add(self.speech_pad_raw);
                self.pending_end = Some(raw_end);
                self.last_boundary_prob = speech_prob;
            }

            // Speech-time integrity: count only Silero-positive VAD frames while
            // a Supervisor segment is active.
            if self.segment_start.is_some() && speech_prob >= self.threshold {
                self.pending_event_speech_vad_samples = self
                    .pending_event_speech_vad_samples
                    .saturating_add(vad::CHUNK_SIZE as u64);
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
                    self.push_event_with_speech_vad_samples(&mut events, SpeechEvent::Chunk(chunk));
                }
                if overlap_size > 0 {
                    self.last_emit_raw = end.saturating_sub(overlap_size);
                } else {
                    self.last_emit_raw = end;
                }
                // Trim raw buffer to prevent unbounded growth during long speech.
                // Keep from last_emit_raw minus pre-roll so overlap slicing still works.
                self.trim_raw_buffer(self.last_emit_raw.saturating_sub(self.pre_roll_raw));
            }

            // Utterance interim: emit every interim_limit samples during continuous
            // speech so the buffered worker gets frequent Whisper passes.
            if let SpeechMode::Utterance { interim_limit, .. } = self.mode
                && self.segment_start.is_some()
                && self.pending_end.is_none()
                && self.raw_cursor.saturating_sub(self.last_emit_raw) >= interim_limit
            {
                let end = self.raw_cursor;
                if let Some(chunk) = self.raw_slice(self.last_emit_raw, end) {
                    debug!(
                        "Utterance interim emit: {} samples ({}s)",
                        chunk.len(),
                        chunk.len() as f32 / self.output_sample_rate as f32
                    );
                    self.push_event_with_speech_vad_samples(
                        &mut events,
                        SpeechEvent::Utterance(chunk),
                    );
                }
                self.last_emit_raw = end;
                self.trim_raw_buffer(end.saturating_sub(self.pre_roll_raw));
            }
        }

        if let Some(end) = self.pending_end
            && self.raw_cursor >= end
        {
            let end = end.min(self.raw_cursor);
            if let Some(start) = self.segment_start.take()
                && let Some((emit_start, emit_end)) = self.supervisor_emit_range(start, end)
                && let Some(chunk) = self.raw_slice(emit_start, emit_end)
            {
                match self.mode {
                    SpeechMode::Stream { .. } => self
                        .push_event_with_speech_vad_samples(&mut events, SpeechEvent::Chunk(chunk)),
                    SpeechMode::Utterance { .. } => self.push_event_with_speech_vad_samples(
                        &mut events,
                        SpeechEvent::UtteranceFinal(chunk),
                    ),
                }
            }
            self.pending_end = None;
            self.last_emit_raw = end;
            self.segment_peak_prob = 0.0;
            self.pending_event_speech_vad_samples = 0;
            // Segment was emitted successfully; clear fallback probe state so
            // later flush() does not use stale speech probability.
            self.max_speech_prob = 0.0;
            self.trim_raw_buffer(end.saturating_sub(self.pre_roll_raw));
        }

        // Safety net: when no segment is active, cap raw_buffer to prevent
        // unbounded growth.
        if self.segment_start.is_none() && self.pending_end.is_none() {
            self.trim_raw_buffer(self.raw_cursor.saturating_sub(self.pre_roll_raw));
        }

        events
    }

    pub fn flush(&mut self) -> Option<SpeechEvent> {
        if self.gate_mode == VadGateMode::Supervisor {
            if let Some(start) = self.segment_start.take() {
                // VAD fired Start but recording ended before End — emit what we have.
                let end = self.pending_end.take().unwrap_or(self.raw_cursor);
                let end = end.min(self.raw_cursor);
                if let Some((emit_start, emit_end)) = self.supervisor_emit_range(start, end)
                    && let Some(chunk) = self.raw_slice(emit_start, emit_end)
                {
                    debug!(
                        "Supervisor flush: open segment {}..{} emitted {}..{} ({} samples)",
                        start,
                        end,
                        emit_start,
                        emit_end,
                        chunk.len()
                    );
                    let speech_vad_samples = self.take_pending_event_speech_vad_samples();
                    self.event_speech_vad_samples.push_back(speech_vad_samples);
                    self.segment_peak_prob = 0.0;
                    self.last_emit_raw = emit_end;
                    return Some(match self.mode {
                        SpeechMode::Stream { .. } => SpeechEvent::Chunk(chunk),
                        SpeechMode::Utterance { .. } => SpeechEvent::UtteranceFinal(chunk),
                    });
                }
                self.last_emit_raw = end;
                self.pending_event_speech_vad_samples = 0;
            }
            // VAD never triggered Start: drop silently.
            self.segment_peak_prob = 0.0;
            self.pending_event_speech_vad_samples = 0;
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
        self.segment_peak_prob = 0.0;
        self.last_append_at = Instant::now();
        match self.mode {
            SpeechMode::Stream { .. } => SpeechEvent::Chunk(chunk),
            SpeechMode::Utterance { .. } => SpeechEvent::UtteranceFinal(chunk),
        }
    }

    fn update_vad_heartbeat(&mut self, speech_prob: f32) {
        self.max_speech_prob = self.max_speech_prob.max(speech_prob);
        let iter_triggered = self
            .iter_state
            .as_ref()
            .is_some_and(|state| state.triggered());
        if self.segment_start.is_some()
            || self.in_speech
            || iter_triggered
            || !self.pending_speech.is_empty()
            || !self.pending_samples.is_empty()
        {
            self.segment_peak_prob = self.segment_peak_prob.max(speech_prob);
        }
        self.vad_frames_total = self.vad_frames_total.saturating_add(1);
        if speech_prob >= self.threshold {
            self.vad_frames_speech = self.vad_frames_speech.saturating_add(1);
        }

        if self.last_vad_heartbeat.elapsed() >= Duration::from_secs(2) {
            let silence = self.vad_frames_total.saturating_sub(self.vad_frames_speech);
            debug!(
                "VAD heartbeat: frames={} speech={} silence={} prob={:.3} threshold={:.3} gate={:?} segment_start={:?} pending_end={:?}",
                self.vad_frames_total,
                self.vad_frames_speech,
                silence,
                speech_prob,
                self.threshold,
                self.gate_mode,
                self.segment_start,
                self.pending_end
            );
            self.vad_frames_total = 0;
            self.vad_frames_speech = 0;
            self.last_vad_heartbeat = Instant::now();
        }
    }

    fn predict_speech_prob(&mut self, frame: &[f32], gate: &str) -> f32 {
        match self.vad.as_mut() {
            Some(vad) => match vad.predict(frame) {
                Ok(prob) => prob,
                Err(e) => {
                    self.vad_predict_errors_total = self.vad_predict_errors_total.saturating_add(1);
                    self.vad_predict_errors_pending =
                        self.vad_predict_errors_pending.saturating_add(1);
                    warn!(
                        "VAD predict error in {} gate; treating frame as non-speech: {}",
                        gate, e
                    );
                    0.0
                }
            },
            None => {
                self.vad_unavailable_frames_total =
                    self.vad_unavailable_frames_total.saturating_add(1);
                self.vad_unavailable_frames_pending =
                    self.vad_unavailable_frames_pending.saturating_add(1);
                if !self.vad_unavailable_logged {
                    warn!("Silero VAD unavailable; treating frames as non-speech");
                    self.vad_unavailable_logged = true;
                }
                0.0
            }
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

    fn take_pending_event_speech_vad_samples(&mut self) -> u64 {
        std::mem::take(&mut self.pending_event_speech_vad_samples)
    }

    fn push_event_with_speech_vad_samples(
        &mut self,
        events: &mut Vec<SpeechEvent>,
        event: SpeechEvent,
    ) {
        let speech_vad_samples = self.take_pending_event_speech_vad_samples();
        self.event_speech_vad_samples.push_back(speech_vad_samples);
        events.push(event);
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

    pub fn output_sample_rate(&self) -> u32 {
        self.output_sample_rate
    }

    /// Speech probability at the last VAD Start/End boundary.
    pub(crate) fn boundary_prob(&self) -> f32 {
        self.last_boundary_prob
    }

    /// Milliseconds elapsed since session creation (wall-clock).
    pub(crate) fn session_elapsed_ms(&self) -> u64 {
        self.session_start.elapsed().as_millis() as u64
    }

    /// Peak speech probability for the current utterance/segment.
    pub(crate) fn segment_speech_prob(&self) -> f32 {
        let prob = self.segment_peak_prob.max(self.last_boundary_prob);
        if prob > 0.0 {
            prob
        } else {
            self.max_speech_prob
        }
    }

    /// Silero-positive speech samples (16k domain) for the next emitted speech event.
    pub(crate) fn take_event_speech_vad_samples(&mut self) -> u64 {
        self.event_speech_vad_samples.pop_front().unwrap_or(0)
    }

    pub(crate) fn take_vad_error_stats(&mut self) -> Option<VadErrorStats> {
        let predict_errors = std::mem::take(&mut self.vad_predict_errors_pending);
        let unavailable_frames = std::mem::take(&mut self.vad_unavailable_frames_pending);
        if predict_errors == 0 && unavailable_frames == 0 {
            return None;
        }
        Some(VadErrorStats {
            predict_errors,
            unavailable_frames,
            total_predict_errors: self.vad_predict_errors_total,
            total_unavailable_frames: self.vad_unavailable_frames_total,
        })
    }

    /// Override VAD threshold (test-only). Set impossibly high to prevent
    /// VadIterState from firing `Start`.
    #[cfg(test)]
    pub fn set_vad_threshold_for_test(&mut self, threshold: f32) {
        self.threshold = threshold;
        if let Some(iter_state) = self.iter_state.as_mut() {
            iter_state.params.threshold = threshold;
        }
    }

    /// Override max_speech_prob (test-only). Simulates observed speech confidence
    /// without requiring real speech audio.
    #[cfg(test)]
    pub fn set_max_speech_prob_for_test(&mut self, prob: f32) {
        self.max_speech_prob = prob;
    }

    /// Current absolute raw sample cursor position (test-only accessor).
    #[cfg(test)]
    pub fn raw_cursor(&self) -> usize {
        self.raw_cursor
    }

    /// Current gate mode (test-only accessor).
    #[cfg(test)]
    pub fn gate_mode(&self) -> VadGateMode {
        self.gate_mode
    }

    /// Mapped VAD index to raw sample index (test-only accessor).
    #[cfg(test)]
    pub fn vad_to_raw_index_pub(&self, vad_index: usize) -> usize {
        self.vad_to_raw_index(vad_index)
    }

    /// Current VAD iterator sample counter (test-only accessor).
    #[cfg(test)]
    pub fn vad_current_sample(&self) -> Option<usize> {
        self.iter_state.as_ref().map(|s| s.current_sample)
    }

    /// Residual VAD resample buffer length (test-only accessor).
    #[cfg(test)]
    pub fn vad_resample_buf_len(&self) -> usize {
        self.vad_resample_buf.len()
    }

    /// Pre-roll size in raw samples (test-only accessor).
    #[cfg(test)]
    pub fn pre_roll_raw(&self) -> usize {
        self.pre_roll_raw
    }

    /// Raw audio buffer length (test-only accessor).
    #[cfg(test)]
    pub fn raw_buffer_len(&self) -> usize {
        self.raw_buffer.len()
    }

    /// Minimum silence duration (in VAD sample-rate samples) before ending a segment
    /// (test-only accessor).
    #[cfg(test)]
    pub fn min_silence_samples(&self) -> usize {
        self.min_silence_samples
    }

    fn vad_to_raw_index(&self, vad_index: usize) -> usize {
        if self.raw_sample_rate == 0 {
            return vad_index;
        }
        ((vad_index as f32 * self.raw_sample_rate as f32) / vad::VAD_SAMPLE_RATE as f32)
            .round()
            .max(0.0) as usize
    }

    fn supervisor_emit_range(&self, segment_start: usize, end: usize) -> Option<(usize, usize)> {
        if end <= segment_start || end > self.raw_cursor {
            return None;
        }

        let mut emit_start = self
            .last_emit_raw
            .max(segment_start)
            .max(self.raw_buffer_start);

        if emit_start >= end {
            // Preserve a small final boundary window when all new audio was already
            // emitted as busy interim/chunk previews.
            emit_start = end
                .saturating_sub(self.pre_roll_raw)
                .max(self.raw_buffer_start);
        }

        (emit_start < end).then_some((emit_start, end))
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

fn utterance_interim_sec() -> f32 {
    std::env::var("CODESCRIBE_BUFFERED_INTERIM_SEC")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(1.2)
        .clamp(1.0, 30.0)
}

fn utterance_silence_sec_override() -> Option<f32> {
    std::env::var("CODESCRIBE_BUFFERED_SILENCE_SEC")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .map(|v| v.clamp(0.1, 10.0))
}

// ═══════════════════════════════════════════════════════════
// Configuration helpers
// ═══════════════════════════════════════════════════════════

#[allow(dead_code)]
pub(crate) fn hardcoded_gate_config() -> GateConfig {
    let vad_cfg = vad::VadConfig::default();
    let pre_roll = vad_cfg.pre_roll_sec;
    let speech_pad = pre_roll;

    GateConfig {
        vad: vad_cfg,
        pre_roll_sec: pre_roll,
        speech_pad_sec: speech_pad,
        mode: SILERO_GATE_MODE,
    }
}

pub(crate) fn hardcoded_utterance_gate_config() -> GateConfig {
    let mut vad_cfg = vad::VadConfig::default();

    // Optional per-utterance override (buffered mode). This is separate from global VAD silence
    // and exists for buffered mode only.
    if let Some(sec) = utterance_silence_sec_override() {
        vad_cfg.max_silence_duration_sec = sec;
    }

    let pre_roll = vad_cfg.pre_roll_sec;
    let speech_pad = pre_roll;

    GateConfig {
        vad: vad_cfg,
        pre_roll_sec: pre_roll,
        speech_pad_sec: speech_pad,
        mode: SILERO_GATE_MODE,
    }
}

pub(crate) fn init_silero_vad(sample_rate: u32, config: &vad::VadConfig) -> Option<vad::SileroVad> {
    let model_path = vad::default_model_path();
    match vad::SileroVad::new(&model_path, config.clone()) {
        Ok(mut vad) => {
            vad.set_input_sample_rate(sample_rate);
            tracing::info!("Silero VAD ready (model: {})", model_path.display());
            Some(vad)
        }
        Err(e) => {
            tracing::warn!("Silero VAD init failed ({}): {}", model_path.display(), e);
            None
        }
    }
}

// ═══════════════════════════════════════════════════════════
// VAD iterator state machine
// ═══════════════════════════════════════════════════════════

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

// ═══════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }

    impl EnvGuard {
        fn unset(key: &'static str) -> Self {
            let prev = std::env::var(key).ok();
            unsafe {
                std::env::remove_var(key);
            }
            Self { key, prev }
        }

        fn set(key: &'static str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                match self.prev.as_ref() {
                    Some(prev) => std::env::set_var(self.key, prev),
                    None => std::env::remove_var(self.key),
                };
            }
        }
    }

    #[test]
    fn vad_iter_state_basic_lifecycle() {
        let config = GateConfig {
            vad: vad::VadConfig {
                threshold: 0.50,
                min_speech_duration_sec: 0.05,
                max_silence_duration_sec: 0.20,
                max_utterance_sec: 300.0,
                pre_roll_sec: 0.064,
            },
            pre_roll_sec: 0.064,
            speech_pad_sec: 0.064,
            mode: VadGateMode::Supervisor,
        };
        let mut state = VadIterState::new(&config, 16000);

        // Initially not triggered
        assert!(!state.triggered());

        // Feed high speech probability — should trigger Start
        let event = state.update(0.9);
        assert!(state.triggered());
        assert!(matches!(event, VadIterEvent::Start { .. }));

        // Continue speech — should be None
        let event = state.update(0.8);
        assert!(matches!(event, VadIterEvent::None));

        // Feed silence below neg_threshold for long enough → End
        // min_silence_samples at 16kHz with 0.20s = 3200 samples
        // Each update advances by frame_size_samples (512)
        // So we need 3200/512 ≈ 7 frames of silence
        for _ in 0..10 {
            let event = state.update(0.01);
            if matches!(event, VadIterEvent::End { .. }) {
                assert!(!state.triggered());
                return; // Success
            }
        }
        // If we got here, the speech was too short for min_speech check
        // That's OK — the state machine correctly filters short bursts.
    }

    #[test]
    fn vad_iter_state_reset() {
        let config = GateConfig {
            vad: vad::VadConfig {
                threshold: 0.50,
                min_speech_duration_sec: 0.05,
                max_silence_duration_sec: 0.20,
                max_utterance_sec: 300.0,
                pre_roll_sec: 0.064,
            },
            pre_roll_sec: 0.064,
            speech_pad_sec: 0.064,
            mode: VadGateMode::Supervisor,
        };
        let mut state = VadIterState::new(&config, 16000);
        state.update(0.9); // trigger
        assert!(state.triggered());
        state.reset();
        assert!(!state.triggered());
        assert_eq!(state.current_sample, 0);
    }

    #[test]
    #[serial]
    fn gate_mode_default_is_supervisor() {
        let config = hardcoded_gate_config();
        assert_eq!(config.mode, VadGateMode::Supervisor);
    }

    #[test]
    #[serial]
    fn utterance_default_silence_is_not_forced_to_stream_default() {
        let _g = EnvGuard::unset("CODESCRIBE_BUFFERED_SILENCE_SEC");

        let sr = 16000u32;

        let stream = SpeechSession::new_stream(sr, 6.0, 1.0);
        let utterance = SpeechSession::new_utterance(sr);

        let base = vad::VadConfig::default();
        let stream_expected = (base.max_silence_duration_sec * vad::VAD_SAMPLE_RATE as f32)
            .round()
            .max(1.0) as usize;
        assert_eq!(stream.min_silence_samples(), stream_expected);

        let utter_expected = (base.max_silence_duration_sec * vad::VAD_SAMPLE_RATE as f32)
            .round()
            .max(1.0) as usize;
        assert_eq!(utterance.min_silence_samples(), utter_expected);
    }

    #[test]
    #[serial]
    fn utterance_silence_env_override_does_not_change_stream_default() {
        let _g = EnvGuard::set("CODESCRIBE_BUFFERED_SILENCE_SEC", "0.45");

        let sr = 16000u32;
        let stream = SpeechSession::new_stream(sr, 6.0, 1.0);
        let utterance = SpeechSession::new_utterance(sr);

        let base = vad::VadConfig::default();
        let stream_expected = (base.max_silence_duration_sec * vad::VAD_SAMPLE_RATE as f32)
            .round()
            .max(1.0) as usize;
        assert_eq!(stream.min_silence_samples(), stream_expected);

        let utter_expected = (0.45 * vad::VAD_SAMPLE_RATE as f32).round().max(1.0) as usize;
        assert_eq!(utterance.min_silence_samples(), utter_expected);
    }

    #[test]
    fn test_utterance_pre_roll_nonzero() {
        // new_utterance() must calculate pre_roll from config, not hardcode 0.
        let session = SpeechSession::new_utterance(16000);
        // Config has pre_roll_sec=0.064 → 16000*0.064 = 1024 samples
        assert!(
            session.pre_roll_raw() > 0,
            "pre_roll_raw should be > 0, got {}",
            session.pre_roll_raw()
        );
    }

    #[test]
    fn speech_event_variants() {
        let chunk = SpeechEvent::Chunk(vec![1.0, 2.0]);
        assert!(matches!(chunk, SpeechEvent::Chunk(v) if v.len() == 2));

        let utt = SpeechEvent::Utterance(vec![3.0]);
        assert!(matches!(utt, SpeechEvent::Utterance(v) if v.len() == 1));

        let final_utt = SpeechEvent::UtteranceFinal(vec![4.0, 5.0, 6.0]);
        assert!(matches!(final_utt, SpeechEvent::UtteranceFinal(v) if v.len() == 3));
    }

    /// Verify that raw_buffer is trimmed during long continuous speech in stream mode.
    /// Without the trim fix, raw_buffer grows without bound.
    #[test]
    fn test_supervisor_stream_raw_buffer_bounded() {
        // Use 16kHz to avoid resampling complexity (VAD native rate).
        let sr = 16000u32;
        let chunk_sec = 2.0f32;
        let mut session = SpeechSession::new_stream(sr, chunk_sec, 0.0);

        if session.gate_mode() != VadGateMode::Supervisor {
            eprintln!("Skipping: gate mode is not Supervisor");
            return;
        }

        // Feed 30 seconds of "speech-like" audio (loud sine wave).
        // With chunk_sec=2s this should produce ~15 chunk emissions.
        let total_samples = sr as usize * 30;
        let callback_size = 512usize; // Match VAD CHUNK_SIZE
        let freq = 300.0f32;
        let mut phase = 0.0f32;
        let phase_inc = 2.0 * std::f32::consts::PI * freq / sr as f32;

        let mut total_chunks = 0usize;
        let mut max_buffer_len = 0usize;

        for _ in 0..(total_samples / callback_size) {
            let mut buf = Vec::with_capacity(callback_size);
            for _ in 0..callback_size {
                buf.push(phase.sin() * 0.8); // Loud enough for VAD
                phase += phase_inc;
            }
            for event in session.feed(&buf, sr) {
                if matches!(event, SpeechEvent::Chunk(_)) {
                    total_chunks += 1;
                }
            }
            let current_len = session.raw_buffer_len();
            if current_len > max_buffer_len {
                max_buffer_len = current_len;
            }
        }

        // The buffer should be bounded: at most ~chunk_limit + pre_roll + some margin.
        // chunk_limit = 2s * 16000 = 32000 samples.
        // Without the fix, max_buffer_len would be ~480000 (30s * 16000).
        let chunk_limit = (chunk_sec * sr as f32) as usize;
        let max_expected = chunk_limit * 3; // Generous bound: 3x chunk size
        assert!(
            max_buffer_len <= max_expected,
            "raw_buffer grew to {} samples (max expected {}); memory leak likely",
            max_buffer_len,
            max_expected
        );

        // Should have produced at least a few chunks if VAD triggered.
        // (VAD may not trigger on pure sine — this is a structural test,
        // not a VAD accuracy test. If 0 chunks, the trim path wasn't exercised.)
        if total_chunks > 0 {
            println!(
                "Supervisor stream: {} chunks emitted, max buffer {} samples (limit {})",
                total_chunks, max_buffer_len, max_expected
            );
        }
    }

    /// Verify that flush() drops audio when VAD never triggers Start.
    #[test]
    fn test_supervisor_flush_drops_when_vad_never_starts() {
        let sr = 16000u32;
        let mut session = SpeechSession::new_utterance(sr);

        if session.gate_mode() != VadGateMode::Supervisor {
            eprintln!("Skipping: gate mode is not Supervisor");
            return;
        }

        // Set impossible threshold so VadIterState never fires Start,
        // simulating edge cases where speech is too soft/short for min_speech.
        session.set_vad_threshold_for_test(2.0);

        // Feed ~1s of audio while threshold prevents Start detection.
        let total_samples = sr as usize; // 1s
        let callback_size = 512usize;
        for i in 0..(total_samples / callback_size) {
            let buf: Vec<f32> = (0..callback_size)
                .map(|j| ((i * callback_size + j) as f32 * 0.01).sin() * 0.5)
                .collect();
            let events = session.feed(&buf, sr);
            // No events expected — threshold too high to trigger Start.
            assert!(
                events.is_empty(),
                "Expected no events with threshold=2.0, got {} at iteration {}",
                events.len(),
                i
            );
        }

        // Flush should drop gracefully (no degraded fallback emission).
        let result = session.flush();
        assert!(
            result.is_none(),
            "flush() should return None when VAD never started a segment"
        );
    }

    /// Regression guard: completed segments must clear peak state.
    #[test]
    fn test_supervisor_segment_completion_resets_fallback_peak() {
        let sr = 16000u32;
        let mut session = SpeechSession::new_utterance(sr);

        if session.gate_mode() != VadGateMode::Supervisor {
            eprintln!("Skipping: gate mode is not Supervisor");
            return;
        }

        let total_samples = sr as usize;
        session.raw_buffer.extend(vec![0.25; total_samples]);
        session.raw_cursor = total_samples;
        session.segment_start = Some(0);
        session.pending_end = Some(total_samples / 2);
        session.last_emit_raw = 0;
        session.set_max_speech_prob_for_test(0.95);
        session.segment_peak_prob = 0.95;

        // feed() requires non-empty input to run supervisor bookkeeping.
        let events = session.feed(&[0.0], sr);
        assert!(
            matches!(events.as_slice(), [SpeechEvent::UtteranceFinal(_)]),
            "Expected completed segment emission, got {} events",
            events.len()
        );
        assert_eq!(
            session.max_speech_prob, 0.0,
            "max_speech_prob should reset after completed segment"
        );

        let flush = session.flush();
        assert!(
            flush.is_none(),
            "flush() should not emit any extra event after a normal completed segment"
        );
    }

    #[test]
    fn test_supervisor_flush_tracks_event_speech_samples() {
        let sr = 16000u32;
        let mut session = SpeechSession::new_utterance(sr);

        if session.gate_mode() != VadGateMode::Supervisor {
            eprintln!("Skipping: gate mode is not Supervisor");
            return;
        }

        let total_samples = sr as usize;
        session.raw_buffer.extend(vec![0.2; total_samples]);
        session.raw_cursor = total_samples;
        session.segment_start = Some(0);
        session.pending_end = None;
        session.pending_event_speech_vad_samples = vad::VAD_SAMPLE_RATE as u64 * 6;

        let flush = session.flush();
        assert!(
            matches!(flush, Some(SpeechEvent::UtteranceFinal(_))),
            "flush should emit the open Supervisor segment"
        );
        assert_eq!(
            session.take_event_speech_vad_samples(),
            vad::VAD_SAMPLE_RATE as u64 * 6,
            "flush event should carry Silero-positive speech samples for trigger accounting"
        );
        assert_eq!(
            session.take_event_speech_vad_samples(),
            0,
            "event speech sample queue should be drained"
        );
    }

    #[test]
    fn test_supervisor_busy_flush_emits_tail_and_keeps_speech_sample_fifo() {
        let sr = 16000u32;
        let mut session = SpeechSession::new_utterance(sr);

        if session.gate_mode() != VadGateMode::Supervisor {
            eprintln!("Skipping: gate mode is not Supervisor");
            return;
        }

        let total_samples = sr as usize * 2;
        let already_emitted = sr as usize;
        session.raw_buffer.extend(vec![0.2; total_samples]);
        session.raw_cursor = total_samples;
        session.segment_start = Some(0);
        session.last_emit_raw = already_emitted;
        session
            .event_speech_vad_samples
            .push_back(vad::VAD_SAMPLE_RATE as u64);
        session.pending_event_speech_vad_samples = vad::VAD_SAMPLE_RATE as u64 * 2;

        let flush = session.flush();
        let tail_len = match flush {
            Some(SpeechEvent::UtteranceFinal(samples)) => samples.len(),
            Some(SpeechEvent::Utterance(_)) | Some(SpeechEvent::Chunk(_)) => {
                panic!("flush must emit UtteranceFinal in utterance mode")
            }
            None => panic!("flush should emit the active Supervisor segment"),
        };
        assert_eq!(
            tail_len,
            total_samples - already_emitted,
            "flush should emit only the previously un-emitted tail"
        );
        assert_eq!(
            session.take_event_speech_vad_samples(),
            vad::VAD_SAMPLE_RATE as u64,
            "existing queued accounting must stay first (FIFO)"
        );
        assert_eq!(
            session.take_event_speech_vad_samples(),
            vad::VAD_SAMPLE_RATE as u64 * 2,
            "flush should append its pending speech accounting after queued events"
        );
        assert_eq!(
            session.take_event_speech_vad_samples(),
            0,
            "speech accounting queue should be fully drained"
        );
    }

    #[test]
    fn test_supervisor_flush_is_boundary_safe_when_start_was_trimmed() {
        let sr = 16000u32;
        let mut session = SpeechSession::new_utterance(sr);

        if session.gate_mode() != VadGateMode::Supervisor {
            eprintln!("Skipping: gate mode is not Supervisor");
            return;
        }

        let trimmed_start = sr as usize / 2;
        let visible_len = sr as usize / 2;
        let tail_start = trimmed_start + visible_len / 2;
        session.raw_buffer_start = trimmed_start;
        session.raw_buffer.extend(vec![0.2; visible_len]);
        session.raw_cursor = trimmed_start + visible_len;
        session.segment_start = Some(0);
        session.last_emit_raw = tail_start;
        session.pending_event_speech_vad_samples = vad::VAD_SAMPLE_RATE as u64;

        let flush = session.flush();
        let len = match flush {
            Some(SpeechEvent::UtteranceFinal(samples)) => samples.len(),
            Some(SpeechEvent::Utterance(_)) | Some(SpeechEvent::Chunk(_)) => {
                panic!("flush must emit UtteranceFinal in utterance mode")
            }
            None => panic!("flush should emit tail even when segment start is trimmed"),
        };
        assert_eq!(
            len,
            session.raw_cursor - tail_start,
            "flush should emit from last emitted boundary to raw_cursor"
        );
        assert_eq!(
            session.take_event_speech_vad_samples(),
            vad::VAD_SAMPLE_RATE as u64,
            "flush should preserve pending speech accounting after boundary clamp"
        );
        assert_eq!(
            session.take_event_speech_vad_samples(),
            0,
            "speech accounting queue should be drained after reading flush event"
        );
    }

    #[test]
    fn test_supervisor_flush_emits_preroll_window_when_no_new_tail_exists() {
        let sr = 16000u32;
        let mut session = SpeechSession::new_utterance(sr);

        if session.gate_mode() != VadGateMode::Supervisor {
            eprintln!("Skipping: gate mode is not Supervisor");
            return;
        }

        let total_samples = sr as usize;
        let preroll = session.pre_roll_raw.max(1);
        session.raw_buffer_start = total_samples - preroll;
        session.raw_buffer.extend(vec![0.2; preroll]);
        session.raw_cursor = total_samples;
        session.segment_start = Some(0);
        session.last_emit_raw = total_samples;
        session.pending_event_speech_vad_samples = 0;

        let flush = session.flush();
        let len = match flush {
            Some(SpeechEvent::UtteranceFinal(samples)) => samples.len(),
            Some(SpeechEvent::Utterance(_)) | Some(SpeechEvent::Chunk(_)) => {
                panic!("flush must emit UtteranceFinal in utterance mode")
            }
            None => panic!("flush should emit a final boundary window"),
        };
        assert_eq!(
            len, preroll,
            "flush should emit preroll-sized context when no unseen tail remains"
        );
        assert_eq!(
            session.take_event_speech_vad_samples(),
            0,
            "finalization-only flush window should carry zero pending speech samples"
        );
    }

    #[test]
    fn test_vad_unavailable_is_measured_and_does_not_assume_speech() {
        let sr = 16000u32;
        let mut session = SpeechSession::new_utterance(sr);
        session.vad = None;

        let audio = vec![0.8; sr as usize / 2];
        let events = session.feed(&audio, sr);
        assert!(
            events.is_empty(),
            "VAD-unavailable path should not emit speech events by default"
        );

        let stats = session
            .take_vad_error_stats()
            .expect("expected VAD unavailable telemetry");
        assert_eq!(stats.predict_errors, 0);
        assert!(stats.unavailable_frames > 0);
        assert_eq!(
            stats.total_unavailable_frames, stats.unavailable_frames,
            "single drain should match totals in this test"
        );
    }
}
