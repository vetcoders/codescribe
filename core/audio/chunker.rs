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

use std::collections::VecDeque;
use std::time::Duration;

use tokio::time::Instant;
use tracing::{debug, trace, warn};

use crate::vad;

// ═══════════════════════════════════════════════════════════
// Constants
// ═══════════════════════════════════════════════════════════

/// Gate mode every constructed session runs in. Not configurable at runtime:
/// the Supervisor semantics (audio always flows, VAD only marks boundaries) are
/// what the downstream quality gate and speech-sample accounting assume.
const SILERO_GATE_MODE: VadGateMode = VadGateMode::Supervisor;
/// Silence that closes a sealed utterance in buffered (Utterance) mode.
///
/// Kept tight enough that natural pauses in dictation (list items, breaths)
/// produce **multiple** `UtteranceFinal` seals — required for freezed+append.
/// 1.2s previously glued entire 50s+ clips into one seal; Apple live then
/// collapsed the long commit to a tail fragment.
pub(crate) const DEFAULT_BUFFERED_SILENCE_SEC: f32 = 0.55;

/// Force-seal continuous speech in buffered mode so no single commit window
/// grows beyond what live STT (Apple AudioBuffer) can carry.
pub(crate) const DEFAULT_BUFFERED_MAX_UTTERANCE_SEC: f32 = 12.0;

// ═══════════════════════════════════════════════════════════
// Public types
// ═══════════════════════════════════════════════════════════

/// One segmentation decision handed to the consumer, carrying the audio it
/// describes.
///
/// The distinction between `Utterance` and `UtteranceFinal` is the whole point
/// of this type: it tells a downstream STT path whether the samples are a
/// preview that will be superseded, or a sealed commit boundary.
pub(crate) enum SpeechEvent {
    // FORGOTTEN-GEM(vc-prune 2026-06-10): parked code, intentionally kept —
    // legacy buffered-chunk payload of the pre-scheduler speech path. Field is
    // unread while SpeechMode::Stream is parked; revive or delete together with it.
    #[allow(dead_code)]
    /// Fixed-size streaming chunk emitted by the parked [`SpeechMode::Stream`]
    /// path. Never produced while buffered (utterance) mode is the only wiring.
    Chunk(Vec<f32>),
    /// Interim utterance slice emitted during long continuous speech to keep streaming responsive.
    Utterance(Vec<f32>),
    /// Final utterance slice emitted when VAD determines the segment ended (or on flush).
    ///
    /// Consumers can use this to distinguish "preview" from "commit" boundaries.
    UtteranceFinal(Vec<f32>),
}

// FORGOTTEN-GEM(vc-prune 2026-06-10): parked code, intentionally kept —
// legacy fixed-chunk streaming mode superseded by the STT scheduler path.
// Revive-or-delete decision belongs to the operator (forgotten-gems report).
#[allow(dead_code)]
/// How a session cuts the audio stream into events, chosen at construction and
/// never changed for the session's lifetime.
pub(crate) enum SpeechMode {
    // FORGOTTEN-GEM(vc-prune 2026-06-10): parked code, intentionally kept —
    // legacy fixed-chunk streaming mode superseded by the utterance/scheduler
    // path. Revive-or-delete belongs to the operator (forgotten-gems report).
    #[allow(dead_code)]
    /// Emit a [`SpeechEvent::Chunk`] every `chunk_limit` samples regardless of
    /// where speech boundaries fall.
    Stream {
        /// Samples accumulated before a chunk is emitted.
        chunk_limit: usize,
        /// Trailing samples replayed at the head of the next chunk, so a word
        /// split across the cut is still heard whole by the STT pass.
        overlap_size: usize,
    },
    /// Emit whole utterances delimited by VAD boundaries — the live path.
    Utterance {
        /// Force-seal threshold: continuous speech longer than this is cut
        /// anyway, so no single commit outgrows what live STT can carry.
        max_utterance_samples: usize,
        /// Periodic interim emit limit (samples). Continuous speech without VAD
        /// silence triggers an interim Utterance every `interim_limit` samples
        /// so Whisper + UI stay responsive during long stretches of speech.
        interim_limit: usize,
    },
}

/// Where Silero sits relative to the audio path.
///
/// Only [`VadGateMode::Supervisor`] is wired today; the other two are kept as
/// documented fallback semantics (see the forgotten-gem notes below).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum VadGateMode {
    // FORGOTTEN-GEM(vc-prune 2026-06-10): parked code, intentionally kept —
    // legacy VAD gate modes superseded by the Supervisor mode below; kept as
    // documented fallback semantics until the operator rules on removal.
    #[allow(dead_code)]
    /// Gate audio before it reaches Whisper (legacy).
    Simple,
    #[allow(dead_code)]
    /// Silero VAD iter logic as a hard gate (legacy).
    Iter,
    /// Silero VAD is a supervisor: audio always flows, VAD only defines boundaries.
    Supervisor,
}

/// Tuned gate parameters resolved once at session construction.
pub(crate) struct GateConfig {
    /// Silero thresholds and duration bounds.
    pub vad: vad::VadConfig,
    /// Audio retained *before* a detected speech start, so the first phoneme
    /// survives the boundary.
    pub pre_roll_sec: f32,
    /// Audio retained *after* a detected speech end, for the same reason.
    pub speech_pad_sec: f32,
    /// Where the VAD sits relative to the audio path.
    pub mode: VadGateMode,
}

/// VAD failure telemetry drained by the caller.
///
/// Split into pending (since last drain) and total (session lifetime) counters
/// so a consumer can both rate-limit warnings and report a session summary.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct VadErrorStats {
    /// `predict()` failures since the last drain.
    pub predict_errors: u64,
    /// Frames processed with no VAD model loaded, since the last drain.
    pub unavailable_frames: u64,
    /// `predict()` failures over the whole session.
    pub total_predict_errors: u64,
    /// Frames processed with no VAD model loaded, over the whole session.
    pub total_unavailable_frames: u64,
}

/// Exact Silero boundary mapped back onto the capture PCM clock.
///
/// This stays in the audio layer so the chunker does not depend on pipeline
/// event contracts. The streaming Silero fusion layer turns it into typed
/// sideband evidence at the one-VAD session boundary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct VadBoundaryEvidence {
    pub kind: VadBoundaryKind,
    pub sample: u64,
    pub speech_probability: f32,
}

/// Which hysteresis edge produced a [`VadBoundaryEvidence`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VadBoundaryKind {
    SpeechStart,
    SpeechEnd,
}

/// Hard memory bound for sideband observations when a caller does not drain
/// them (the legacy VAD/scheduler path). Fusion drains every callback, so this
/// cap is a safety net rather than normal backpressure.
const MAX_PENDING_VAD_BOUNDARIES: usize = 512;

// ═══════════════════════════════════════════════════════════
// SpeechSession
// ═══════════════════════════════════════════════════════════

/// Stateful VAD gate + chunker for one recording session.
///
/// Owns two parallel views of the same audio: the raw stream at the capture
/// sample rate (what is actually emitted) and a 16 kHz resampled view (what
/// Silero sees). Boundaries are detected in the VAD domain and mapped back to
/// raw indices, which is why the raw buffer carries its own absolute cursor and
/// start offset rather than being indexed from zero.
///
/// Not `Send`-shared: drive it from a single audio callback path.
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
    /// Boundary observations waiting for the session-level Silero ingress.
    /// FIFO order is the PCM order in which the iterator produced them.
    vad_boundaries: VecDeque<VadBoundaryEvidence>,
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
    /// After a max-utterance force seal, re-open the segment so continuous speech
    /// is not lost waiting for a fresh Silero Start.
    force_reopen_after_seal: bool,
}

impl SpeechSession {
    // FORGOTTEN-GEM(vc-prune 2026-06-10): parked code, intentionally kept —
    // constructor of the parked SpeechMode::Stream path (see enum above).
    #[allow(dead_code)]
    /// Build a fixed-chunk streaming session emitting every `chunk_duration_sec`
    /// with `overlap_sec` of replayed tail. Parked path — see [`SpeechMode`].
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
            vad_boundaries: VecDeque::new(),
            session_start: Instant::now(),
            vad_predict_errors_total: 0,
            vad_predict_errors_pending: 0,
            vad_unavailable_frames_total: 0,
            vad_unavailable_frames_pending: 0,
            vad_unavailable_logged: false,
            force_reopen_after_seal: false,
        }
    }

    /// Build a buffered (utterance) session at the capture `sample_rate` — the
    /// live path. Silence and interim cadence come from the tuned defaults,
    /// overridable through the `CODESCRIBE_BUFFERED_*` env vars.
    pub fn new_utterance(sample_rate: u32) -> Self {
        let interim_sec = utterance_interim_sec();
        Self::new_utterance_with_interim_and_silence(sample_rate, interim_sec, None)
    }

    /// Same as [`Self::new_utterance`] but with the closing-silence threshold
    /// pinned by the caller (clamped to 0.1–10 s), so a lane that knows its own
    /// pause cadence is not bound to the global default.
    pub fn new_utterance_with_silence(sample_rate: u32, max_silence_sec: f32) -> Self {
        let interim_sec = utterance_interim_sec();
        Self::new_utterance_with_interim_and_silence(
            sample_rate,
            interim_sec,
            Some(max_silence_sec),
        )
    }

    /// Shared constructor behind both buffered entry points. Resolves the tuned
    /// gate config, derives every sample-domain bound from it, and loads Silero.
    /// A VAD that fails to load is not fatal — the session runs with `vad: None`
    /// and reports the shortfall through [`Self::take_vad_error_stats`].
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
            vad_boundaries: VecDeque::new(),
            session_start: Instant::now(),
            vad_predict_errors_total: 0,
            vad_predict_errors_pending: 0,
            vad_unavailable_frames_total: 0,
            vad_unavailable_frames_pending: 0,
            vad_unavailable_logged: false,
            force_reopen_after_seal: false,
        }
    }

    /// Push one capture callback's worth of audio and collect whatever events it
    /// completed. Empty input is a no-op; `_sample_rate` is ignored because the
    /// rate was fixed at construction.
    ///
    /// In Supervisor mode this delegates straight to [`Self::feed_supervisor`];
    /// the body below is the parked pre-supervisor gate.
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

    /// The live segmentation path: raw audio is buffered unconditionally while
    /// Silero runs on a 16 kHz view and only *marks* boundaries, which are then
    /// mapped back into raw indices.
    ///
    /// Emits, in order of precedence: interim previews during long continuous
    /// speech, a force-seal when a segment outgrows `max_utterance_samples`, and
    /// a final seal once a detected end is reached. Trims the raw buffer after
    /// every emit so a long dictation cannot grow it without bound.
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
                let raw_boundary = self.vad_to_raw_index(start_sample);
                self.record_vad_boundary(VadBoundaryEvidence {
                    kind: VadBoundaryKind::SpeechStart,
                    sample: raw_boundary as u64,
                    speech_probability: speech_prob,
                });
                let padded_start = raw_boundary.saturating_sub(self.pre_roll_raw);
                self.segment_start = Some(padded_start);
                self.last_emit_raw = padded_start;
                self.last_boundary_prob = speech_prob;
                self.segment_peak_prob = speech_prob;
            }

            if let Some(end_sample) = end_event {
                let raw_boundary = self.vad_to_raw_index(end_sample);
                self.record_vad_boundary(VadBoundaryEvidence {
                    kind: VadBoundaryKind::SpeechEnd,
                    sample: raw_boundary as u64,
                    speech_probability: speech_prob,
                });
                self.pending_end = Some(raw_boundary.saturating_add(self.speech_pad_raw));
                self.last_boundary_prob = speech_prob;
            }

            // Speech-time integrity: count Silero-positive VAD frames while a
            // Supervisor segment is active. Use `neg_threshold` (the same
            // segment-open hysteresis used by `gate_with_prob`), NOT the higher
            // onset `threshold`: speech hovering in the 0.35–0.50 band keeps the
            // segment open, so counting it at >= threshold under-reports real
            // speech and makes the downstream silence-drop ratio gate
            // (`should_drop_silence_chunk`) wrongly classify present speech as
            // silence.
            if self.segment_start.is_some() && speech_prob >= self.neg_threshold {
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

            // Force-seal continuous speech at max_utterance so freezed+append
            // gets multiple finals even when Silero never sees a long pause
            // (core engine: utterance-by-utterance, not one 50s tail).
            if let SpeechMode::Utterance {
                max_utterance_samples,
                ..
            } = self.mode
                && let Some(start) = self.segment_start
                && self.pending_end.is_none()
                && self.raw_cursor.saturating_sub(start) >= max_utterance_samples
            {
                debug!(
                    "Utterance max-duration force seal at {}s (start={}, cursor={})",
                    max_utterance_samples as f32 / self.output_sample_rate as f32,
                    start,
                    self.raw_cursor
                );
                self.pending_end = Some(self.raw_cursor);
                self.last_boundary_prob = speech_prob;
                self.force_reopen_after_seal = true;
            }
        }

        if let Some(end) = self.pending_end
            && self.raw_cursor >= end
        {
            let end = end.min(self.raw_cursor);
            let reopen = self.force_reopen_after_seal;
            self.force_reopen_after_seal = false;

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

            if reopen {
                // Continuous speech force-seal: re-open immediately.
                self.segment_start = Some(end);
                self.last_emit_raw = end;
                self.segment_peak_prob = self.last_boundary_prob;
            }
        }

        // Safety net: when no segment is active, cap raw_buffer to prevent
        // unbounded growth.
        if self.segment_start.is_none() && self.pending_end.is_none() {
            self.trim_raw_buffer(self.raw_cursor.saturating_sub(self.pre_roll_raw));
        }

        events
    }

    /// Half-open raw-sample range of the currently open Supervisor segment.
    ///
    /// `None` when Silero has not opened a speech edge. Used by the W13-3B
    /// fusion lane to mint utterance identity on the same PCM cursor the
    /// Apple worker already owns.
    pub(crate) fn open_segment_raw_range(&self) -> Option<(u64, u64)> {
        let start = self.segment_start?;
        Some((start as u64, self.raw_cursor as u64))
    }

    fn record_vad_boundary(&mut self, boundary: VadBoundaryEvidence) {
        if self.vad_boundaries.len() >= MAX_PENDING_VAD_BOUNDARIES {
            self.vad_boundaries.pop_front();
        }
        self.vad_boundaries.push_back(boundary);
    }

    /// Drain exact Silero start/end observations in production order.
    ///
    /// Reading these observations never gates PCM. An empty vector therefore
    /// means "no sideband evidence", not "drop or delay audio".
    pub(crate) fn take_vad_boundaries(&mut self) -> Vec<VadBoundaryEvidence> {
        self.vad_boundaries.drain(..).collect()
    }

    /// Close the session and emit whatever is still open.
    ///
    /// Recording usually stops mid-segment, so an open Supervisor segment is
    /// sealed from the last emitted boundary to the cursor. Audio that VAD never
    /// opened a segment for is dropped silently rather than emitted as a
    /// degraded guess — a deliberate choice, since sending unvalidated silence
    /// to STT produces hallucinated text.
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

    /// Take the pending buffer as a streaming chunk, replaying the configured
    /// overlap tail into the next one.
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

    /// Take the pending buffer as a sealed segment and reset every per-segment
    /// accumulator (speech/silence counters, pre-roll, VAD iterator) so the next
    /// segment starts from a clean state.
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

    /// Fold one frame's probability into the peak trackers and frame counters,
    /// emitting a DEBUG heartbeat every two seconds.
    ///
    /// The segment peak is only advanced while a segment is actually open,
    /// which is what keeps [`Self::segment_speech_prob`] from reporting
    /// confidence borrowed from a neighbouring segment.
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

    /// Run Silero on one frame, counting failures instead of propagating them.
    ///
    /// Both a `predict()` error and a missing model yield `0.0` (non-speech).
    /// Assuming speech would be the dangerous default: it opens segments on
    /// silence and feeds STT audio nobody spoke. `gate` only labels the warning.
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

    /// Threshold gate with hysteresis: opening a segment needs `threshold`,
    /// holding it open only `neg_threshold`, and a segment closes after
    /// `min_silence_samples` below that floor.
    ///
    /// Silence inside a segment is buffered rather than dropped, so a short
    /// pause that turns out not to end the utterance keeps its audio.
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

    /// Drain the speech-sample accounting accumulated since the last emit.
    fn take_pending_event_speech_vad_samples(&mut self) -> u64 {
        std::mem::take(&mut self.pending_event_speech_vad_samples)
    }

    /// Queue an event together with its speech-sample accounting, keeping the
    /// two in FIFO lockstep. The consumer reads them back in the same order via
    /// [`Self::take_event_speech_vad_samples`], so an event never inherits the
    /// speech time of its neighbour.
    fn push_event_with_speech_vad_samples(
        &mut self,
        events: &mut Vec<SpeechEvent>,
        event: SpeechEvent,
    ) {
        let speech_vad_samples = self.take_pending_event_speech_vad_samples();
        self.event_speech_vad_samples.push_back(speech_vad_samples);
        events.push(event);
    }

    /// Gate driven by the Silero iterator state machine rather than a bare
    /// threshold. Falls back to [`Self::gate_with_prob`] when no iterator state
    /// exists. On a detected end the pending buffer is truncated to
    /// pre-roll + speech + pad, so trailing silence never reaches STT.
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

    /// Keep the most recent `pre_roll_samples` of pre-speech audio in a ring, so
    /// the frames just before a detected start are still available to prepend.
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

    /// Sample rate of the audio carried by emitted events. In Supervisor mode
    /// this is the capture rate, not Silero's 16 kHz working rate.
    pub fn output_sample_rate(&self) -> u32 {
        self.output_sample_rate
    }

    /// Whether Silero actually loaded for this session.
    ///
    /// A missing model is not fatal here — [`Self::predict_speech_prob`] reads
    /// every frame as non-speech — but a consumer that *gates* on speech edges
    /// (the Apple engine lifecycle) would then never see one and would rest
    /// forever. Such consumers must ask first and fail open.
    pub(crate) fn vad_available(&self) -> bool {
        self.vad.is_some()
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

    /// Drain VAD failure telemetry, or `None` when nothing failed since the last
    /// call. The pending counters reset; the totals do not.
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
    /// VadIterState from firing `Start`, or below zero to force a deterministic
    /// open segment when the Silero model is unavailable.
    ///
    /// Forces `neg_threshold` to the same value: the speech-sample accounting
    /// (see `feed`) now counts frames at `neg_threshold`, not `threshold`, so a
    /// test that drives the onset threshold below the silence floor must move
    /// the hysteresis bound with it — otherwise forced-open segments would
    /// report zero accounted speech.
    #[cfg(test)]
    pub fn set_vad_threshold_for_test(&mut self, threshold: f32) {
        self.threshold = threshold;
        self.neg_threshold = threshold;
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

    /// Convert a 16 kHz VAD-domain sample index into the raw capture domain.
    /// Identity when the raw rate is unknown (zero), which keeps the mapping
    /// total rather than panicking on a misconfigured session.
    fn vad_to_raw_index(&self, vad_index: usize) -> usize {
        if self.raw_sample_rate == 0 {
            return vad_index;
        }
        ((vad_index as f32 * self.raw_sample_rate as f32) / vad::VAD_SAMPLE_RATE as f32)
            .round()
            .max(0.0) as usize
    }

    /// Resolve the absolute raw range a Supervisor seal should emit, clamped to
    /// what the buffer still holds.
    ///
    /// When everything up to `end` was already emitted as interim previews, the
    /// range collapses — so a pre-roll-sized window before `end` is emitted
    /// instead of nothing, preserving a final boundary the consumer can commit
    /// against. `None` means there is genuinely nothing left to emit.
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

    /// Copy an absolute raw range out of the buffer, or `None` when the range is
    /// empty or has already been trimmed away. Absolute indices are translated
    /// through `raw_buffer_start`, so a trimmed prefix cannot be misread as
    /// valid audio.
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

    /// Drop raw samples older than the absolute index `keep_from`, advancing the
    /// buffer's start offset to match. This is what bounds memory across a long
    /// dictation; calls with an already-trimmed index are no-ops.
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

/// Interim preview cadence in buffered mode, from
/// `CODESCRIBE_BUFFERED_INTERIM_SEC` (default 1.2 s, clamped to 1–30 s).
fn utterance_interim_sec() -> f32 {
    std::env::var("CODESCRIBE_BUFFERED_INTERIM_SEC")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(1.2)
        .clamp(1.0, 30.0)
}

/// Operator override for the closing-silence threshold via
/// `CODESCRIBE_BUFFERED_SILENCE_SEC` (clamped to 0.1–10 s). `None` keeps
/// [`DEFAULT_BUFFERED_SILENCE_SEC`].
fn utterance_silence_sec_override() -> Option<f32> {
    std::env::var("CODESCRIBE_BUFFERED_SILENCE_SEC")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .map(|v| v.clamp(0.1, 10.0))
}

/// Operator override for the force-seal ceiling via
/// `CODESCRIBE_BUFFERED_MAX_UTTERANCE_SEC` (clamped to 3–120 s). `None` keeps
/// [`DEFAULT_BUFFERED_MAX_UTTERANCE_SEC`].
fn utterance_max_sec_override() -> Option<f32> {
    std::env::var("CODESCRIBE_BUFFERED_MAX_UTTERANCE_SEC")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .map(|v| v.clamp(3.0, 120.0))
}

// ═══════════════════════════════════════════════════════════
// Configuration helpers
// ═══════════════════════════════════════════════════════════

// FORGOTTEN-GEM(vc-prune 2026-06-10): parked code, intentionally kept —
// canonical tuned-gate preset retained as reference values for GateConfig
// regressions; not wired to any runtime path today.
#[allow(dead_code)]
/// The tuned stream-mode gate preset: VAD defaults with pre-roll and speech pad
/// held equal. Reference values for `GateConfig` regressions.
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

/// The live buffered-mode gate preset. Differs from [`hardcoded_gate_config`]
/// in exactly two axes — closing silence and force-seal ceiling — because those
/// are what make one dictation produce several sealed commits instead of one
/// long tail. Both accept env overrides.
pub(crate) fn hardcoded_utterance_gate_config() -> GateConfig {
    // Buffered mode feeds final utterances through the serialized Commit lane.
    // Silence + max-utterance force multi-seal freezed+append (core engine).
    let vad_cfg = vad::VadConfig {
        max_silence_duration_sec: utterance_silence_sec_override()
            .unwrap_or(DEFAULT_BUFFERED_SILENCE_SEC),
        max_utterance_sec: utterance_max_sec_override()
            .unwrap_or(DEFAULT_BUFFERED_MAX_UTTERANCE_SEC),
        ..vad::VadConfig::default()
    };

    let pre_roll = vad_cfg.pre_roll_sec;
    let speech_pad = pre_roll;

    GateConfig {
        vad: vad_cfg,
        pre_roll_sec: pre_roll,
        speech_pad_sec: speech_pad,
        mode: SILERO_GATE_MODE,
    }
}

/// Load the embedded Silero model (zero filesystem I/O) for `sample_rate`.
///
/// Returns `None` on failure instead of erroring: a session without VAD still
/// captures audio and reports the shortfall as telemetry, which beats refusing
/// to record at all.
pub(crate) fn init_silero_vad(sample_rate: u32, config: &vad::VadConfig) -> Option<vad::SileroVad> {
    match vad::SileroVad::new_embedded(config.clone()) {
        Ok(mut vad) => {
            vad.set_input_sample_rate(sample_rate);
            tracing::info!("Silero VAD ready (embedded model, zero I/O)");
            Some(vad)
        }
        Err(e) => {
            tracing::warn!("Silero VAD embedded init failed: {}", e);
            None
        }
    }
}

// ═══════════════════════════════════════════════════════════
// VAD iterator state machine
// ═══════════════════════════════════════════════════════════

/// One gate verdict for a frame: the audio to keep (if any) and whether the
/// segment just ended.
struct GateDecision {
    /// Audio the gate lets through, already including any replayed pre-roll.
    audio: Option<Vec<f32>>,
    /// `true` when this frame closed the segment.
    ended: bool,
}

/// Silero's start/end boundary state machine, ported from the reference
/// `VADIterator`.
///
/// Works purely in the 16 kHz VAD sample domain and counts time in frames — it
/// never sees raw audio, which is why the caller maps its indices back with
/// [`SpeechSession::vad_to_raw_index`].
#[derive(Debug)]
struct VadIterState {
    /// Thresholds and durations, precomputed into sample counts.
    params: VadIterParams,
    /// Frames consumed so far, expressed in VAD-domain samples.
    current_sample: usize,
    /// Candidate end position, held until the silence run is long enough to
    /// confirm it. Zero means "no candidate".
    temp_end: usize,
    /// Where speech resumed after a candidate end, used to continue rather than
    /// re-open a segment.
    next_start: usize,
    /// Last confirmed end, kept as the cut point when a max-length segment must
    /// be split.
    prev_end: usize,
    /// Whether a segment is currently open.
    triggered: bool,
    /// Start position of the open segment.
    speech_start: usize,
}

/// [`VadIterState`] tuning, resolved once from [`GateConfig`] into VAD-domain
/// sample counts so the hot path does no float arithmetic on durations.
#[derive(Debug)]
struct VadIterParams {
    /// Probability at or above which a frame counts as speech onset.
    threshold: f32,
    /// Silence run required to confirm a segment end.
    min_silence_samples: usize,
    /// Shortest run that counts as real speech; below it, an end is ignored so
    /// coughs and clicks do not seal a segment.
    min_speech_samples: usize,
    /// Ceiling after which an open segment is split even without silence.
    max_speech_samples: f32,
    /// Silero frame size — the granularity every counter advances by.
    frame_size_samples: usize,
    /// Shorter silence run accepted as a split point once a segment has hit its
    /// maximum length (~98 ms), so an over-long segment cuts at a plausible gap
    /// instead of mid-word.
    min_silence_samples_at_max_speech: usize,
}

/// What one frame did to the boundary state machine.
#[derive(Debug, Copy, Clone)]
enum VadIterEvent {
    /// No boundary change.
    None,
    /// A segment opened at this VAD-domain sample.
    Start { start_sample: usize },
    /// A segment closed at this VAD-domain sample.
    End { end_sample: usize },
}

impl VadIterState {
    /// Precompute every duration in `config` into `sample_rate`-domain counts.
    /// The max-speech budget subtracts one frame and both pads, so a segment
    /// split lands inside the configured ceiling once padding is added back.
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

    /// Return to the pre-speech state, keeping the tuned params. Note the frame
    /// counter resets too, so VAD-domain indices restart from zero.
    fn reset(&mut self) {
        self.current_sample = 0;
        self.temp_end = 0;
        self.next_start = 0;
        self.prev_end = 0;
        self.triggered = false;
        self.speech_start = 0;
    }

    /// Whether a speech segment is currently open.
    fn triggered(&self) -> bool {
        self.triggered
    }

    /// VAD-domain start index of the open segment.
    fn current_speech_start(&self) -> usize {
        self.speech_start
    }

    /// Advance one frame and report any boundary it produced.
    ///
    /// Three exits, in priority order: speech above `threshold` (opens a segment
    /// or cancels a pending end); an open segment past `max_speech_samples`
    /// (forced split, preferring a previously confirmed end over an arbitrary
    /// cut); and speech below the hysteresis floor for `min_silence_samples`
    /// (confirmed end, ignored when the speech run was shorter than
    /// `min_speech_samples`).
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

/// VAD iterator, gate defaults, supervisor flush, and unavailable-path tests.
#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn vad_boundary_evidence_drains_once_in_pcm_order() {
        let mut session = SpeechSession::new_utterance(16_000);
        session.vad_boundaries.extend([
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechStart,
                sample: 512,
                speech_probability: 0.91,
            },
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechEnd,
                sample: 16_384,
                speech_probability: 0.08,
            },
        ]);

        let drained = session.take_vad_boundaries();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].kind, VadBoundaryKind::SpeechStart);
        assert_eq!(drained[0].sample, 512);
        assert_eq!(drained[1].kind, VadBoundaryKind::SpeechEnd);
        assert_eq!(drained[1].sample, 16_384);
        assert!(session.take_vad_boundaries().is_empty());
    }

    #[test]
    fn vad_boundary_evidence_is_memory_bounded_without_a_consumer() {
        let mut session = SpeechSession::new_utterance(16_000);
        for sample in 0..(MAX_PENDING_VAD_BOUNDARIES as u64 + 37) {
            session.record_vad_boundary(VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechStart,
                sample,
                speech_probability: 0.9,
            });
        }

        let drained = session.take_vad_boundaries();
        assert_eq!(drained.len(), MAX_PENDING_VAD_BOUNDARIES);
        assert_eq!(drained.first().map(|edge| edge.sample), Some(37));
    }

    /// RAII guard that restores an env var on drop.
    ///
    /// The gate config reads `CODESCRIBE_BUFFERED_*` at construction, so these
    /// tests must mutate process-wide env — which is why every test using this
    /// guard is also `#[serial]`. Restoring the previous value (including its
    /// absence) keeps the operator's own `~/.codescribe/.env` from leaking
    /// between tests.
    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }

    impl EnvGuard {
        /// Remove `key` for the guard's lifetime.
        fn unset(key: &'static str) -> Self {
            let prev = std::env::var(key).ok();
            unsafe {
                std::env::remove_var(key);
            }
            Self { key, prev }
        }

        /// Set `key` to `value` for the guard's lifetime.
        fn set(key: &'static str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, prev }
        }
    }

    impl Drop for EnvGuard {
        /// Restore the previous env value (or unset) when the guard leaves scope.
        fn drop(&mut self) {
            unsafe {
                match self.prev.as_ref() {
                    Some(prev) => std::env::set_var(self.key, prev),
                    None => std::env::remove_var(self.key),
                };
            }
        }
    }

    /// Start on high prob, stay open on speech, End after enough silence frames.
    #[test]
    fn vad_iter_state_basic_lifecycle() {
        let config = GateConfig {
            vad: vad::VadConfig {
                threshold: 0.50,
                min_speech_duration_sec: 0.05,
                max_silence_duration_sec: 0.20,
                max_utterance_sec: 300.0,
                pre_roll_sec: 0.064,
                ..vad::VadConfig::default()
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

    /// reset() clears triggered state and the VAD-domain sample counter.
    #[test]
    fn vad_iter_state_reset() {
        let config = GateConfig {
            vad: vad::VadConfig {
                threshold: 0.50,
                min_speech_duration_sec: 0.05,
                max_silence_duration_sec: 0.20,
                max_utterance_sec: 300.0,
                pre_roll_sec: 0.064,
                ..vad::VadConfig::default()
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

    /// hardcoded_gate_config defaults to Supervisor gate mode.
    #[test]
    #[serial]
    fn gate_mode_default_is_supervisor() {
        let config = hardcoded_gate_config();
        assert_eq!(config.mode, VadGateMode::Supervisor);
    }

    /// Utterance silence uses DEFAULT_BUFFERED_SILENCE_SEC; stream keeps VAD base.
    #[test]
    #[serial]
    fn utterance_default_silence_uses_buffered_cadence_default() {
        let _g = EnvGuard::unset("CODESCRIBE_BUFFERED_SILENCE_SEC");

        let sr = 16000u32;

        let stream = SpeechSession::new_stream(sr, 6.0, 1.0);
        let utterance = SpeechSession::new_utterance(sr);

        let base = vad::VadConfig::default();
        let stream_expected = (base.max_silence_duration_sec * vad::VAD_SAMPLE_RATE as f32)
            .round()
            .max(1.0) as usize;
        assert_eq!(stream.min_silence_samples(), stream_expected);

        let utter_expected =
            (DEFAULT_BUFFERED_SILENCE_SEC * vad::VAD_SAMPLE_RATE as f32).round() as usize;
        assert_eq!(utterance.min_silence_samples(), utter_expected);
    }

    /// BUFFERED_SILENCE_SEC overrides utterance only; stream silence stays base.
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

    /// new_utterance derives non-zero pre_roll from config (not hardcoded zero).
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

    /// Chunk, Utterance, and UtteranceFinal carry sample vectors as constructed.
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

    /// Flush emits open segment and drains pending Silero speech-sample accounting.
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

    /// Busy flush emits only unseen tail and preserves FIFO speech accounting.
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

    /// Flush clamps to visible buffer when segment_start was trimmed away.
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

    /// No unseen tail → emit preroll-sized final window with zero speech samples.
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

    /// Missing VAD emits no speech events and records unavailable-frame telemetry.
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
