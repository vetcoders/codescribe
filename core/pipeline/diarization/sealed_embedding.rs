//! Speaker attribution after ledger finality, on the capture sample clock.
//!
//! `group_word_slots_into_sealed_spans` is a grouping projection, not evidence
//! of a seal: `WordSlot` has no finality flag. `attribute_sealed` authenticates
//! each owner against the ledger before invoking the embedding callback on
//! exactly the selected PCM range. It must run downstream of text publication;
//! an attribution error never grants permission to delay or rewrite text.
//!
//! One tracker belongs to one session, epoch and embedding model. A model or
//! preprocessing change requires a new tracker and compatible enrollment.
//! Embeddings/profiles stay in memory; this experiment has no persistence,
//! logging, network, enrollment consent UI or production bus integration.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use ndarray::Array3;
use ort::session::Session;
use ort::value::Value;
use realfft::{RealFftPlanner, RealToComplex};
use serde::{Deserialize, Serialize};

use crate::pipeline::acoustic_ledger::{AcousticLedger, WordSlot};

/// A half-open PCM range. IDs are caller-owned and unique within an epoch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedPcmSpan {
    pub span_id: u64,
    pub session_epoch: u64,
    pub sample_start: u64,
    pub sample_end: u64,
    /// Indices in the same immutable slot snapshot supplied to attribution.
    pub word_indices: Vec<usize>,
}

/// One model's speaker vector. Public fields are validated at every boundary.
/// No Debug implementation: accidentally logging a voiceprint is undesirable.
#[derive(Clone, PartialEq)]
pub struct SpeakerEmbedding {
    pub dim: usize,
    pub l2_normalized_vec: Vec<f32>,
}

impl SpeakerEmbedding {
    /// Normalize a finite, nonzero vector without overflowing f32 norm sums.
    pub fn from_vector(vector: Vec<f32>) -> Result<Self, AttributionError> {
        let vector = normalize(vector)?;
        Ok(Self {
            dim: vector.len(),
            l2_normalized_vec: vector,
        })
    }

    fn validate(&self, expected_dim: usize) -> Result<(), AttributionError> {
        if self.dim != expected_dim || self.l2_normalized_vec.len() != expected_dim {
            return Err(AttributionError::EmbeddingDimension);
        }
        let norm = squared_norm(&self.l2_normalized_vec)?;
        if (norm - 1.0).abs() > 1e-4 {
            return Err(AttributionError::InvalidEmbedding);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AttributionDecision {
    EnrolledSpeaker {
        speaker_id: String,
        cosine_sim: f32,
    },
    DynamicCluster {
        cluster_id: usize,
        cosine_sim: f32,
    },
    /// `None` is an unresolved short span, never a guessed speaker.
    ShortSpanCarryOver {
        inherited_speaker: Option<String>,
        duration_samples: u64,
    },
    RejectedStaleEpoch {
        span_epoch: u64,
        active_epoch: u64,
    },
}

/// Metadata for the supplied immutable slot snapshot, not another document.
#[derive(Debug, Clone, PartialEq)]
pub struct SpanAttribution {
    pub span: SealedPcmSpan,
    pub decision: AttributionDecision,
}

impl SpanAttribution {
    /// Propagate the same annotation to every constituent slot without
    /// modifying `WordSlot`, its text or its equality/occurrence identity.
    /// Stale results deliberately produce no word annotations.
    pub fn word_attributions(&self) -> impl Iterator<Item = (usize, &AttributionDecision)> {
        let accepted = !matches!(
            &self.decision,
            AttributionDecision::RejectedStaleEpoch { .. }
        );
        self.span
            .word_indices
            .iter()
            .copied()
            .filter(move |_| accepted)
            .map(|index| (index, &self.decision))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AttributionError {
    InvalidConfiguration,
    InvalidSpan,
    InvalidWordIndex,
    WrongCapture,
    UnsealedOwner,
    SpanIdReused,
    OutOfOrderSpan,
    PcmUnavailable,
    NonFinitePcm,
    EmbeddingDimension,
    InvalidEmbedding,
    InferenceFailed,
    InvalidSpeakerId,
    AmbiguousEnrollment,
    InsufficientCentroidEvidence,
    ClusterCapacity,
}

impl std::fmt::Display for AttributionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "speaker attribution: {self:?}")
    }
}

impl std::error::Error for AttributionError {}

/// Sample counts use the supplied PCM rate; defaults describe 16 kHz mono.
/// Thresholds are experimental policy values, not measured accuracy claims.
#[derive(Debug, Clone)]
pub struct CentroidTrackerConfig {
    pub embedding_dim: usize,
    pub sample_rate_hz: u32,
    pub min_embedding_samples: u64,
    pub min_centroid_update_samples: u64,
    pub max_clusters: usize,
    /// A dynamic match below this cosine threshold starts a new cluster.
    pub cosine_new_cluster_threshold: f32,
    pub enrolled_match_threshold: f32,
    pub enrolled_match_margin: f32,
    /// Weight of the new, sufficiently long observation in the EMA.
    pub ema_weight: f32,
    /// Carry only a nearby direct attribution, never across arbitrary silence.
    pub max_carry_gap_samples: u64,
}

impl CentroidTrackerConfig {
    pub fn for_16khz(embedding_dim: usize) -> Self {
        Self {
            embedding_dim,
            sample_rate_hz: 16_000,
            min_embedding_samples: 12_800,
            min_centroid_update_samples: 24_000,
            max_clusters: 8,
            cosine_new_cluster_threshold: 0.70,
            enrolled_match_threshold: 0.80,
            enrolled_match_margin: 0.05,
            ema_weight: 0.15,
            max_carry_gap_samples: 8_000,
        }
    }

    pub fn for_rate(embedding_dim: usize, sample_rate_hz: u32) -> Self {
        let rate = u64::from(sample_rate_hz.max(1));
        Self {
            embedding_dim,
            sample_rate_hz: sample_rate_hz.max(1),
            min_embedding_samples: (rate * 800) / 1_000,
            min_centroid_update_samples: (rate * 1_500) / 1_000,
            max_clusters: 8,
            cosine_new_cluster_threshold: 0.70,
            enrolled_match_threshold: 0.80,
            enrolled_match_margin: 0.05,
            ema_weight: 0.15,
            max_carry_gap_samples: (rate * 500) / 1_000,
        }
    }

    fn validate(&self) -> Result<(), AttributionError> {
        let bounded = |value: f32| value.is_finite() && (0.0..=1.0).contains(&value);
        if self.embedding_dim == 0
            || self.sample_rate_hz == 0
            || self.min_embedding_samples == 0
            || self.min_centroid_update_samples < self.min_embedding_samples
            || self.max_clusters == 0
            || !bounded(self.cosine_new_cluster_threshold)
            || !bounded(self.enrolled_match_threshold)
            || !bounded(self.enrolled_match_margin)
            || !bounded(self.ema_weight)
            || self.ema_weight == 0.0
        {
            return Err(AttributionError::InvalidConfiguration);
        }
        Ok(())
    }
}

/// Borrowed evidence; neither PCM nor slots are retained by the tracker.
/// `slots` must be the complete immutable ledger projection for this capture,
/// and PCM must come from that same capture. The frozen span has no session
/// string or buffer provenance; the caller is responsible for this binding.
pub struct SealedEmbeddingInput<'a> {
    pub ledger: &'a AcousticLedger,
    pub slots: &'a [WordSlot],
    pub span: &'a SealedPcmSpan,
    /// First capture sample represented by `pcm[0]` (not necessarily zero).
    pub pcm_sample_start: u64,
    pub pcm: &'a [f32],
    pub sample_rate_hz: u32,
}

/// Session-local cluster state. Enrolled profiles are immutable anchors;
/// only dynamic centroids learn from sufficiently long spans.
pub struct OnlineCentroidTracker {
    session: String,
    active_epoch: u64,
    config: CentroidTrackerConfig,
    enrolled_centroids: BTreeMap<String, SpeakerEmbedding>,
    dynamic_centroids: Vec<SpeakerEmbedding>,
    completed: BTreeMap<u64, SpanAttribution>,
    last_end: Option<u64>,
    /// Only direct named results can satisfy the frozen string carry field.
    /// Anonymous cluster IDs are never disguised as enrollment identifiers.
    last_named: Option<(String, u64)>,
}

impl OnlineCentroidTracker {
    pub fn new(
        session: String,
        active_epoch: u64,
        config: CentroidTrackerConfig,
    ) -> Result<Self, AttributionError> {
        config.validate()?;
        if session.is_empty() {
            return Err(AttributionError::InvalidConfiguration);
        }
        Ok(Self {
            session,
            active_epoch,
            config,
            enrolled_centroids: BTreeMap::new(),
            dynamic_centroids: Vec::new(),
            completed: BTreeMap::new(),
            last_end: None,
            last_named: None,
        })
    }

    /// Supply consented, model-compatible anchors before processing an epoch.
    /// Reconfiguration after attribution begins requires a new tracker.
    pub fn enroll(
        &mut self,
        speaker_id: String,
        embedding: SpeakerEmbedding,
    ) -> Result<(), AttributionError> {
        if speaker_id.trim().is_empty() || self.enrolled_centroids.contains_key(&speaker_id) {
            return Err(AttributionError::InvalidSpeakerId);
        }
        if !self.completed.is_empty() {
            return Err(AttributionError::InvalidConfiguration);
        }
        embedding.validate(self.config.embedding_dim)?;
        self.enrolled_centroids.insert(speaker_id, embedding);
        Ok(())
    }

    /// Keep model-compatible anchors, discard all session learning and carry.
    /// Calling this is an explicit capture-owner action, never auto-rollover
    /// in response to an asynchronous result.
    pub fn reset_capture(
        &mut self,
        session: String,
        active_epoch: u64,
    ) -> Result<(), AttributionError> {
        if session.is_empty() {
            return Err(AttributionError::InvalidConfiguration);
        }
        self.session = session;
        self.active_epoch = active_epoch;
        self.dynamic_centroids.clear();
        self.completed.clear();
        self.last_end = None;
        self.last_named = None;
        Ok(())
    }

    pub fn dynamic_cluster_count(&self) -> usize {
        self.dynamic_centroids.len()
    }

    /// Synchronous post-seal processing; schedule downstream of publication.
    /// The callback must use one fixed model/preprocessing contract. It sees
    /// only the sealed PCM slice, and is never called for short/stale/replayed
    /// spans. Errors never change centroids or completed results. A valid long
    /// span that fails inference/classification clears carry: an unknown turn
    /// cannot lend the previous speaker's name to a following short reply.
    pub fn attribute_sealed(
        &mut self,
        input: SealedEmbeddingInput<'_>,
        embed: impl FnOnce(&[f32]) -> Result<SpeakerEmbedding, AttributionError>,
    ) -> Result<SpanAttribution, AttributionError> {
        let span = input.span;
        if span.session_epoch != self.active_epoch {
            return Ok(SpanAttribution {
                span: span.clone(),
                decision: AttributionDecision::RejectedStaleEpoch {
                    span_epoch: span.session_epoch,
                    active_epoch: self.active_epoch,
                },
            });
        }
        if input.sample_rate_hz != self.config.sample_rate_hz {
            return Err(AttributionError::InvalidConfiguration);
        }
        self.validate_sealed_words(&input)?;
        if let Some(previous) = self.completed.get(&span.span_id) {
            return if previous.span == *span {
                Ok(previous.clone())
            } else {
                Err(AttributionError::SpanIdReused)
            };
        }
        if self.last_end.is_some_and(|end| span.sample_start < end) {
            return Err(AttributionError::OutOfOrderSpan);
        }
        let duration_samples = span.sample_end - span.sample_start;
        let decision = if duration_samples < self.config.min_embedding_samples {
            let inherited_speaker = self.last_named.as_ref().and_then(|(speaker, end)| {
                (span.sample_start.saturating_sub(*end) <= self.config.max_carry_gap_samples)
                    .then(|| speaker.clone())
            });
            AttributionDecision::ShortSpanCarryOver {
                inherited_speaker,
                duration_samples,
            }
        } else {
            self.last_named = None;
            let pcm = self.sealed_pcm(&input)?;
            let embedding = embed(pcm)?;
            embedding.validate(self.config.embedding_dim)?;
            self.classify(&embedding, duration_samples)?
        };

        // Only infallible bookkeeping follows classify's possible centroid update.
        match &decision {
            AttributionDecision::EnrolledSpeaker { speaker_id, .. } => {
                self.last_named = Some((speaker_id.clone(), span.sample_end));
            }
            AttributionDecision::ShortSpanCarryOver { .. } => {
                // Do not extend the direct evidence's age through a chain of "yes".
            }
            _ => self.last_named = None,
        }
        let result = SpanAttribution {
            span: span.clone(),
            decision,
        };
        self.completed.insert(span.span_id, result.clone());
        self.last_end = Some(span.sample_end);
        Ok(result)
    }

    fn validate_sealed_words(
        &self,
        input: &SealedEmbeddingInput<'_>,
    ) -> Result<(), AttributionError> {
        let span = input.span;
        if span.sample_end <= span.sample_start || span.word_indices.is_empty() {
            return Err(AttributionError::InvalidSpan);
        }
        let mut seen = BTreeSet::new();
        let mut sealed_owner = None;
        let mut start = u64::MAX;
        let mut end = 0;
        for &index in &span.word_indices {
            if !seen.insert(index) {
                return Err(AttributionError::InvalidWordIndex);
            }
            let slot = input
                .slots
                .get(index)
                .ok_or(AttributionError::InvalidWordIndex)?;
            let owner = &slot.observation.occurrence;
            if owner.session != self.session || owner.capture_epoch != self.active_epoch {
                return Err(AttributionError::WrongCapture);
            }
            if !input.ledger.is_sealed(owner) {
                return Err(AttributionError::UnsealedOwner);
            }
            // Two sealed endpoints do not seal the intervening audio. The
            // entire PCM slice must fit one exact ledger-sealed occurrence.
            if sealed_owner.is_some_and(|prior| prior != owner) {
                return Err(AttributionError::InvalidSpan);
            }
            sealed_owner = Some(owner);
            if slot.sample_end <= slot.sample_start
                || slot.sample_start < owner.sample_start
                || slot.sample_end > owner.sample_end
                || slot.sample_start < span.sample_start
                || slot.sample_end > span.sample_end
            {
                return Err(AttributionError::InvalidSpan);
            }
            start = start.min(slot.sample_start);
            end = end.max(slot.sample_end);
        }
        if start != span.sample_start || end != span.sample_end {
            return Err(AttributionError::InvalidSpan);
        }
        for (index, slot) in input.slots.iter().enumerate() {
            let owner = &slot.observation.occurrence;
            if owner.session == self.session
                && owner.capture_epoch == self.active_epoch
                && slot.sample_start < span.sample_end
                && slot.sample_end > span.sample_start
                && !seen.contains(&index)
            {
                return Err(AttributionError::InvalidWordIndex);
            }
        }
        Ok(())
    }

    fn sealed_pcm<'a>(
        &self,
        input: &SealedEmbeddingInput<'a>,
    ) -> Result<&'a [f32], AttributionError> {
        let offset = |sample: u64| {
            sample
                .checked_sub(input.pcm_sample_start)
                .and_then(|offset| usize::try_from(offset).ok())
                .ok_or(AttributionError::PcmUnavailable)
        };
        let pcm = input
            .pcm
            .get(offset(input.span.sample_start)?..offset(input.span.sample_end)?)
            .ok_or(AttributionError::PcmUnavailable)?;
        if pcm.iter().any(|sample| !sample.is_finite()) {
            return Err(AttributionError::NonFinitePcm);
        }
        Ok(pcm)
    }

    fn classify(
        &mut self,
        embedding: &SpeakerEmbedding,
        duration_samples: u64,
    ) -> Result<AttributionDecision, AttributionError> {
        let mut enrolled: Vec<_> = self
            .enrolled_centroids
            .iter()
            .map(|(id, centroid)| (id, cosine(embedding, centroid)))
            .collect();
        enrolled.sort_by(|left, right| right.1.total_cmp(&left.1).then(left.0.cmp(right.0)));
        if let Some((id, score)) = enrolled.first()
            && *score >= self.config.enrolled_match_threshold
        {
            if enrolled
                .get(1)
                .is_some_and(|(_, second)| score - second < self.config.enrolled_match_margin)
            {
                return Err(AttributionError::AmbiguousEnrollment);
            }
            return Ok(AttributionDecision::EnrolledSpeaker {
                speaker_id: (*id).clone(),
                cosine_sim: *score,
            });
        }
        let best = self
            .dynamic_centroids
            .iter()
            .enumerate()
            .map(|(index, centroid)| (index, cosine(embedding, centroid)))
            .max_by(|left, right| left.1.total_cmp(&right.1).then(right.0.cmp(&left.0)));
        if let Some((cluster_id, score)) = best
            && score >= self.config.cosine_new_cluster_threshold
        {
            if duration_samples >= self.config.min_centroid_update_samples {
                let centroid = &self.dynamic_centroids[cluster_id];
                let weight = f64::from(self.config.ema_weight);
                let blended = centroid
                    .l2_normalized_vec
                    .iter()
                    .zip(&embedding.l2_normalized_vec)
                    .map(|(old, new)| {
                        ((1.0 - weight) * f64::from(*old) + weight * f64::from(*new)) as f32
                    })
                    .collect();
                let updated = SpeakerEmbedding::from_vector(blended)?;
                self.dynamic_centroids[cluster_id] = updated;
            }
            return Ok(AttributionDecision::DynamicCluster {
                cluster_id,
                cosine_sim: score,
            });
        }
        if duration_samples < self.config.min_centroid_update_samples {
            return Err(AttributionError::InsufficientCentroidEvidence);
        }
        if self.dynamic_centroids.len() >= self.config.max_clusters {
            return Err(AttributionError::ClusterCapacity);
        }
        let cluster_id = self.dynamic_centroids.len();
        self.dynamic_centroids.push(embedding.clone());
        Ok(AttributionDecision::DynamicCluster {
            cluster_id,
            cosine_sim: 1.0,
        })
    }
}

/// Group adjacent slots on one capture clock, retaining original indices.
///
/// PRECONDITION: the caller supplies only ledger-sealed slots from one immutable
/// snapshot. This signature cannot prove a seal; `attribute_sealed` checks it.
/// Invalid/foreign-epoch slots break a group and are omitted. Reversed input
/// order or an occurrence change breaks a group. Adjacent sealed owners never
/// imply that the PCM between them is sealed, even when the gap is small.
/// Overlap extends the current range; silence longer than the gap splits it.
/// Gap proximity is not speaker-turn detection and cannot resolve overlap.
///
/// IDs are snapshot-local ordinals. A caller processing multiple snapshots must
/// assign stable epoch-unique IDs and freeze each submitted island; reusing an
/// ID for an extended/re-grouped span is rejected by the tracker.
pub fn group_word_slots_into_sealed_spans(
    slots: &[crate::pipeline::acoustic_ledger::WordSlot],
    max_intra_turn_gap_samples: u64,
    session_epoch: u64,
) -> Vec<SealedPcmSpan> {
    let mut spans: Vec<SealedPcmSpan> = Vec::new();
    let mut previous: Option<&WordSlot> = None;
    for (index, slot) in slots.iter().enumerate() {
        let owner = &slot.observation.occurrence;
        if owner.capture_epoch != session_epoch
            || slot.sample_end <= slot.sample_start
            || slot.sample_start < owner.sample_start
            || slot.sample_end > owner.sample_end
        {
            previous = None;
            continue;
        }
        let adjacent = previous.is_some_and(|prior| {
            prior.observation.occurrence == *owner && slot.sample_start >= prior.sample_start
        }) && spans.last().is_some_and(|span| {
            slot.sample_start.saturating_sub(span.sample_end) <= max_intra_turn_gap_samples
        });
        if adjacent {
            if let Some(span) = spans.last_mut() {
                span.sample_end = span.sample_end.max(slot.sample_end);
                span.word_indices.push(index);
            }
        } else {
            spans.push(SealedPcmSpan {
                span_id: spans.len() as u64,
                session_epoch,
                sample_start: slot.sample_start,
                sample_end: slot.sample_end,
                word_indices: vec![index],
            });
        }
        previous = Some(slot);
    }
    spans
}

fn squared_norm(vector: &[f32]) -> Result<f64, AttributionError> {
    if vector.is_empty() || vector.iter().any(|value| !value.is_finite()) {
        return Err(AttributionError::InvalidEmbedding);
    }
    let sum: f64 = vector.iter().map(|value| f64::from(*value).powi(2)).sum();
    if !sum.is_finite() || sum <= 0.0 {
        return Err(AttributionError::InvalidEmbedding);
    }
    Ok(sum)
}

fn normalize(vector: Vec<f32>) -> Result<Vec<f32>, AttributionError> {
    let norm = squared_norm(&vector)?.sqrt();
    Ok(vector
        .into_iter()
        .map(|value| (f64::from(value) / norm) as f32)
        .collect())
}

fn cosine(left: &SpeakerEmbedding, right: &SpeakerEmbedding) -> f32 {
    left.l2_normalized_vec
        .iter()
        .zip(&right.l2_normalized_vec)
        .map(|(a, b)| f64::from(*a) * f64::from(*b))
        .sum::<f64>()
        .clamp(-1.0, 1.0) as f32
}

// ═══════════════════════════════════════════════════════════════════════════
// Sealed-Span Acoustic Feature & ONNX Embedding Backend (`ort` + `realfft`)
// ═══════════════════════════════════════════════════════════════════════════

/// Trait abstracting speaker embedding extraction over a sealed 16 kHz PCM span.
pub trait SealedSpanEmbedder: Send + Sync {
    /// Dimensionality of the output [`SpeakerEmbedding`].
    fn embedding_dim(&self) -> usize;

    /// Compute an L2-normalized [`SpeakerEmbedding`] for a sealed 16 kHz PCM slice.
    fn embed_sealed_pcm(&self, pcm_16khz: &[f32]) -> Result<SpeakerEmbedding>;
}

/// Configuration for Kaldi-compatible 80-dimensional log-mel filterbank (`Fbank`)
/// extraction expected by WeSpeaker ResNet / CAM++ speaker embedding ONNX models.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FbankConfig {
    /// Input audio sample rate in Hz (16,000 Hz).
    pub sample_rate_hz: u32,
    /// Frame length in samples (400 samples = 25 ms @ 16 kHz).
    pub frame_length_samples: usize,
    /// Frame shift / hop in samples (160 samples = 10 ms @ 16 kHz).
    pub frame_shift_samples: usize,
    /// FFT size (512 samples).
    pub fft_size: usize,
    /// Number of triangular mel bins (80).
    pub num_mel_bins: usize,
    /// Pre-emphasis coefficient (`0.97`).
    pub preemphasis: f32,
    /// Low cutoff frequency in Hz (`20.0`).
    pub low_freq_hz: f32,
    /// High cutoff frequency in Hz (`7600.0` for 16 kHz Nyquist).
    pub high_freq_hz: f32,
    /// Apply per-utterance cepstral mean normalization (CMN) across frames.
    pub apply_cmn: bool,
}

impl Default for FbankConfig {
    fn default() -> Self {
        Self {
            sample_rate_hz: 16_000,
            frame_length_samples: 400,
            frame_shift_samples: 160,
            fft_size: 512,
            num_mel_bins: 80,
            preemphasis: 0.97,
            low_freq_hz: 20.0,
            high_freq_hz: 7_600.0,
            apply_cmn: true,
        }
    }
}

/// Deterministic 80-bin log-mel filterbank extractor using `realfft` for sealed
/// utterance audio spans.
pub struct FbankExtractor {
    config: FbankConfig,
    povey_window: Vec<f32>,
    mel_filters: Vec<f32>,
    fft: Arc<dyn RealToComplex<f32>>,
}

impl FbankExtractor {
    /// Build a new filterbank extractor with precomputed Povey window, mel filter
    /// weights, and planned real FFT.
    pub fn new(config: FbankConfig) -> Self {
        let frame_len = config.frame_length_samples.max(1);
        let fft_size = config.fft_size.max(frame_len).next_power_of_two();
        let two_pi = 2.0 * std::f32::consts::PI;
        // Kaldi Povey window: (0.5 - 0.5 * cos(2*pi*n / (N-1)))^0.85
        let denom = (frame_len.saturating_sub(1)).max(1) as f32;
        let povey_window: Vec<f32> = (0..frame_len)
            .map(|n| (0.5 - 0.5 * (two_pi * n as f32 / denom).cos()).powf(0.85))
            .collect();

        let num_fft_bins = 1 + fft_size / 2;
        let mel_filters = Self::build_mel_filterbank(
            config.num_mel_bins,
            fft_size,
            num_fft_bins,
            config.sample_rate_hz as f32,
            config.low_freq_hz,
            config.high_freq_hz,
        );
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(fft_size);

        Self {
            config: FbankConfig { fft_size, ..config },
            povey_window,
            mel_filters,
            fft,
        }
    }

    /// Compute `[num_frames, num_mel_bins]` log-mel filterbank features from 16 kHz PCM.
    pub fn extract(&self, samples: &[f32]) -> Option<(usize, usize, Vec<f32>)> {
        let frame_len = self.config.frame_length_samples;
        let shift = self.config.frame_shift_samples.max(1);
        let n_mel = self.config.num_mel_bins;
        if samples.len() < frame_len || n_mel == 0 {
            return None;
        }
        let num_frames = 1 + (samples.len() - frame_len) / shift;
        let fft_size = self.config.fft_size;
        let num_fft_bins = 1 + fft_size / 2;

        let mut input = self.fft.make_input_vec();
        let mut spectrum = self.fft.make_output_vec();
        let mut scratch = self.fft.make_scratch_vec();
        let mut power = vec![0.0_f32; num_fft_bins];
        let mut features = vec![0.0_f32; num_frames * n_mel];

        for frame_idx in 0..num_frames {
            let offset = frame_idx * shift;
            let frame = &samples[offset..offset + frame_len];
            let mean: f32 = frame.iter().copied().sum::<f32>() / frame_len as f32;

            let mut prev = frame[0] - mean;
            input[0] = (prev * (1.0 - self.config.preemphasis)) * self.povey_window[0];
            for i in 1..frame_len {
                let current = frame[i] - mean;
                let emphasized = current - self.config.preemphasis * prev;
                input[i] = emphasized * self.povey_window[i];
                prev = current;
            }
            input[frame_len..].fill(0.0);

            self.fft
                .process_with_scratch(&mut input, &mut spectrum, &mut scratch)
                .ok()?;

            for (slot, bin) in power.iter_mut().zip(&spectrum) {
                *slot = bin.norm_sqr();
            }

            let row_out = &mut features[frame_idx * n_mel..(frame_idx + 1) * n_mel];
            for (mel_idx, filter_row) in self
                .mel_filters
                .chunks_exact(num_fft_bins)
                .take(n_mel)
                .enumerate()
            {
                let energy: f32 = filter_row.iter().zip(&power).map(|(w, p)| w * p).sum();
                row_out[mel_idx] = energy.max(1e-10).ln();
            }
        }

        if self.config.apply_cmn && num_frames > 1 {
            for mel_idx in 0..n_mel {
                let mean: f32 = (0..num_frames)
                    .map(|f| features[f * n_mel + mel_idx])
                    .sum::<f32>()
                    / num_frames as f32;
                for f in 0..num_frames {
                    features[f * n_mel + mel_idx] -= mean;
                }
            }
        }

        Some((num_frames, n_mel, features))
    }

    fn hz_to_mel(hz: f32) -> f32 {
        1127.0 * (1.0 + hz / 700.0).ln()
    }

    fn build_mel_filterbank(
        num_mel_bins: usize,
        fft_size: usize,
        num_fft_bins: usize,
        sample_rate_hz: f32,
        low_freq_hz: f32,
        high_freq_hz: f32,
    ) -> Vec<f32> {
        let mut filters = vec![0.0_f32; num_mel_bins * num_fft_bins];
        if num_mel_bins == 0 || num_fft_bins == 0 {
            return filters;
        }
        let low_mel = Self::hz_to_mel(low_freq_hz.max(0.0));
        let high_mel = Self::hz_to_mel(high_freq_hz.max(low_freq_hz + 1.0));
        let mel_step = (high_mel - low_mel) / (num_mel_bins + 1) as f32;
        let fft_bin_width = sample_rate_hz / fft_size as f32;

        for bin in 0..num_mel_bins {
            let left_mel = low_mel + bin as f32 * mel_step;
            let center_mel = low_mel + (bin + 1) as f32 * mel_step;
            let right_mel = low_mel + (bin + 2) as f32 * mel_step;
            let row = &mut filters[bin * num_fft_bins..(bin + 1) * num_fft_bins];

            for (fft_idx, slot) in row.iter_mut().enumerate() {
                let freq = fft_idx as f32 * fft_bin_width;
                let mel = Self::hz_to_mel(freq);
                if mel > left_mel && mel < right_mel {
                    let up = (mel - left_mel) / (center_mel - left_mel).max(1e-6);
                    let down = (right_mel - mel) / (right_mel - center_mel).max(1e-6);
                    *slot = up.min(down).max(0.0);
                }
            }
        }
        filters
    }
}

/// ONNX Runtime speaker embedding extractor for WeSpeaker / CAM++ / sherpa-onnx
/// speaker verification models (`feats` `[1, T, 80]` -> `embs` `[1, D]`).
pub struct OrtWeSpeakerEmbedder {
    session: Mutex<Session>,
    fbank: FbankExtractor,
    embedding_dim: usize,
    input_name: String,
    output_name: String,
}

impl OrtWeSpeakerEmbedder {
    /// Load a WeSpeaker / CAM++ ONNX model from disk using the workspace's `ort`
    /// runtime (`intra_threads = 1`).
    pub fn from_file(
        model_path: &Path,
        embedding_dim: usize,
        fbank_config: FbankConfig,
    ) -> Result<Self> {
        let session = Session::builder()?
            .with_intra_threads(1)
            .map_err(ort::Error::<()>::from)?
            .commit_from_file(model_path)
            .with_context(|| {
                format!(
                    "Failed to load speaker embedding ONNX model from {}",
                    model_path.display()
                )
            })?;

        Ok(Self {
            session: Mutex::new(session),
            fbank: FbankExtractor::new(fbank_config),
            embedding_dim,
            input_name: "feats".to_string(),
            output_name: "embs".to_string(),
        })
    }

    /// Override default ONNX tensor names (`"feats"` / `"embs"`) for custom exports.
    pub fn with_tensor_names(
        mut self,
        input_name: impl Into<String>,
        output_name: impl Into<String>,
    ) -> Self {
        self.input_name = input_name.into();
        self.output_name = output_name.into();
        self
    }
}

impl SealedSpanEmbedder for OrtWeSpeakerEmbedder {
    fn embedding_dim(&self) -> usize {
        self.embedding_dim
    }

    fn embed_sealed_pcm(&self, pcm_16khz: &[f32]) -> Result<SpeakerEmbedding> {
        anyhow::ensure!(
            pcm_16khz.len() as u64 >= 12_800,
            "Sealed PCM span too short for speaker embedding: {} samples (< {})",
            pcm_16khz.len(),
            12_800
        );
        let (num_frames, num_mel_bins, features) = self
            .fbank
            .extract(pcm_16khz)
            .context("Failed to extract log-mel filterbank features from sealed span")?;

        let input_array = Array3::from_shape_vec([1, num_frames, num_mel_bins], features)
            .map_err(|e| anyhow::anyhow!("Invalid fbank tensor shape: {e}"))?;
        let input_value = Value::from_array(input_array)?;

        let raw_vec = {
            let mut session = self
                .session
                .lock()
                .map_err(|e| anyhow::anyhow!("Speaker embedder ONNX lock poisoned: {e}"))?;
            let outputs = session.run([(&input_value).into()])?;
            let data: Vec<f32> = if let Some(output) = outputs.get(self.output_name.as_str()) {
                output.try_extract_tensor::<f32>()?.1.to_vec()
            } else {
                let (_, output) = outputs
                    .iter()
                    .next()
                    .context("Speaker embedding ONNX model returned no output tensor")?;
                output.try_extract_tensor::<f32>()?.1.to_vec()
            };
            anyhow::ensure!(
                data.len() == self.embedding_dim,
                "Expected embedding dim {}, model returned {}",
                self.embedding_dim,
                data.len()
            );
            data
        };

        SpeakerEmbedding::from_vector(raw_vec)
            .map_err(|e| anyhow::anyhow!(e))
            .context("Speaker embedding output contained non-finite or zero-norm values")
    }
}

/// Pinned local WeSpeaker ResNet34-LM ONNX model (`Wespeaker/wespeaker-voxceleb-resnet34-LM`, CC-BY-4.0).
pub const WESPEAKER_MODEL_FILENAME: &str = "wespeaker-voxceleb-resnet34-LM.onnx";
pub const WESPEAKER_MODEL_SHA256: &str =
    "7bb2f06e9df17cdf1ef14ee8a15ab08ed28e8d0ef5054ee135741560df2ec068";
pub const WESPEAKER_EMBEDDING_DIM: usize = 256;

static SHARED_WESPEAKER: std::sync::LazyLock<Option<Arc<OrtWeSpeakerEmbedder>>> =
    std::sync::LazyLock::new(|| {
        let path = resolve_wespeaker_model_path()?;
        match OrtWeSpeakerEmbedder::from_file(
            &path,
            WESPEAKER_EMBEDDING_DIM,
            FbankConfig::default(),
        ) {
            Ok(embedder) => {
                tracing::info!(
                    event = "wespeaker_embedder_loaded",
                    path = %path.display(),
                    dim = WESPEAKER_EMBEDDING_DIM,
                    sha256 = WESPEAKER_MODEL_SHA256,
                    "Loaded WeSpeaker ONNX speaker embedder"
                );
                Some(Arc::new(embedder))
            }
            Err(error) => {
                tracing::warn!(
                    event = "wespeaker_embedder_load_failed",
                    path = %path.display(),
                    %error,
                    "WeSpeaker ONNX speaker embedder unavailable"
                );
                None
            }
        }
    });

/// Resolve the local WeSpeaker ONNX model path strictly from `Config::config_dir()/models`
/// so isolated `CODESCRIBE_DATA_DIR` test environments never leak into `HOME`.
pub fn resolve_wespeaker_model_path() -> Option<std::path::PathBuf> {
    let candidate = crate::config::Config::config_dir()
        .join("models")
        .join(WESPEAKER_MODEL_FILENAME);
    candidate.is_file().then_some(candidate)
}

/// Process-wide cached WeSpeaker ONNX embedder (`wespeaker-voxceleb-resnet34-LM.onnx`).
pub fn shared_wespeaker_embedder() -> Option<Arc<OrtWeSpeakerEmbedder>> {
    SHARED_WESPEAKER.clone()
}

/// Per-utterance speaker attribution receipt emitted immediately when an occurrence seals.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UtteranceSpeakerReceipt {
    pub schema: String,
    pub session: String,
    pub capture_epoch: u64,
    pub span_id: u64,
    pub sample_start: u64,
    pub sample_end: u64,
    pub sample_rate_hz: u32,
    pub speaker_label: Option<String>,
    pub decision_kind: String,
    pub cosine_sim: Option<f32>,
    pub dynamic_cluster_count: usize,
    pub word_indices: Vec<usize>,
    pub model_sha256: String,
}

/// Session-level speaker diarization summary written at terminal seal (`STOP`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TakeDiarizationSummary {
    pub schema: String,
    pub session: String,
    pub capture_epoch: u64,
    pub sample_rate_hz: u32,
    pub model_filename: String,
    pub model_sha256: String,
    pub dynamic_cluster_count: usize,
    pub utterances: Vec<UtteranceSpeakerReceipt>,
}

/// Live per-session speaker attribution observer driven by `AppleSealState` on
/// each occurrence seal and finalized at `seal_terminal`. Never mutates `WordSlot`
/// or `AcousticLedger`.
pub struct LiveSessionSpeakerTracker {
    session: String,
    capture_epoch: u64,
    sample_rate_hz: u32,
    tracker: OnlineCentroidTracker,
    attributed_occurrences: BTreeSet<crate::pipeline::acoustic_ledger::OccurrenceIdentity>,
    next_span_id: u64,
    last_cluster_label: Option<(String, u64)>,
    max_carry_gap_samples: u64,
    receipts: Vec<UtteranceSpeakerReceipt>,
}

impl LiveSessionSpeakerTracker {
    pub fn for_session(session: &str, capture_epoch: u64, sample_rate_hz: u32) -> Self {
        let config = CentroidTrackerConfig::for_rate(WESPEAKER_EMBEDDING_DIM, sample_rate_hz);
        let max_carry_gap_samples = config.max_carry_gap_samples;
        let safe_session = if session.is_empty() {
            "unknown-session".to_string()
        } else {
            session.to_string()
        };
        let tracker = OnlineCentroidTracker::new(safe_session.clone(), capture_epoch, config)
            .expect("valid rate-scaled CentroidTrackerConfig");
        Self {
            session: safe_session,
            capture_epoch,
            sample_rate_hz: sample_rate_hz.max(1),
            tracker,
            attributed_occurrences: BTreeSet::new(),
            next_span_id: 0,
            last_cluster_label: None,
            max_carry_gap_samples,
            receipts: Vec::new(),
        }
    }

    /// Attribute a newly sealed occurrence using the shared WeSpeaker ONNX embedder.
    pub fn observe_sealed_occurrence(
        &mut self,
        ledger: &AcousticLedger,
        occurrence: &crate::pipeline::acoustic_ledger::OccurrenceIdentity,
        pcm: &[f32],
        pcm_sample_start: u64,
    ) -> Vec<UtteranceSpeakerReceipt> {
        let Some(embedder) = shared_wespeaker_embedder() else {
            return Vec::new();
        };
        let sample_rate_hz = self.sample_rate_hz;
        self.observe_sealed_occurrence_with(ledger, occurrence, pcm, pcm_sample_start, |slice| {
            let pcm_16k = if sample_rate_hz == 16_000 {
                slice.to_vec()
            } else {
                crate::audio::loader::resample_to_16k(slice, sample_rate_hz)
                    .map_err(|_| AttributionError::InferenceFailed)?
            };
            embedder
                .embed_sealed_pcm(&pcm_16k)
                .map_err(|_| AttributionError::InferenceFailed)
        })
    }

    /// Attribute a newly sealed occurrence with an explicit embedding function.
    pub fn observe_sealed_occurrence_with(
        &mut self,
        ledger: &AcousticLedger,
        occurrence: &crate::pipeline::acoustic_ledger::OccurrenceIdentity,
        pcm: &[f32],
        pcm_sample_start: u64,
        mut embed_fn: impl FnMut(&[f32]) -> Result<SpeakerEmbedding, AttributionError>,
    ) -> Vec<UtteranceSpeakerReceipt> {
        if !ledger.is_sealed(occurrence) || self.attributed_occurrences.contains(occurrence) {
            return Vec::new();
        }
        let Some(slots) = ledger.slots_of(occurrence) else {
            return Vec::new();
        };
        if slots.is_empty() {
            return Vec::new();
        }
        let max_gap = u64::from(self.sample_rate_hz) / 2;
        let mut spans = group_word_slots_into_sealed_spans(slots, max_gap, self.capture_epoch);
        if spans.is_empty() {
            return Vec::new();
        }
        let sample_rate_hz = self.sample_rate_hz;
        let mut emitted = Vec::with_capacity(spans.len());
        for span in &mut spans {
            span.span_id = self.next_span_id;
            self.next_span_id += 1;
            let attribution = self.tracker.attribute_sealed(
                SealedEmbeddingInput {
                    ledger,
                    slots,
                    span,
                    pcm_sample_start,
                    pcm,
                    sample_rate_hz,
                },
                &mut embed_fn,
            );
            let Ok(attr) = attribution else {
                continue;
            };
            let (speaker_label, decision_kind, cosine_sim) = match &attr.decision {
                AttributionDecision::EnrolledSpeaker {
                    speaker_id,
                    cosine_sim,
                } => {
                    self.last_cluster_label = Some((speaker_id.clone(), span.sample_end));
                    (
                        Some(speaker_id.clone()),
                        "enrolled_speaker".to_string(),
                        Some(*cosine_sim),
                    )
                }
                AttributionDecision::DynamicCluster {
                    cluster_id,
                    cosine_sim,
                } => {
                    let label = format!("SPEAKER_{cluster_id:02}");
                    self.last_cluster_label = Some((label.clone(), span.sample_end));
                    (
                        Some(label),
                        "dynamic_cluster".to_string(),
                        Some(*cosine_sim),
                    )
                }
                AttributionDecision::ShortSpanCarryOver {
                    inherited_speaker, ..
                } => {
                    let inherited = inherited_speaker.clone().or_else(|| {
                        self.last_cluster_label.as_ref().and_then(|(lbl, end)| {
                            (span.sample_start.saturating_sub(*end) <= self.max_carry_gap_samples)
                                .then(|| lbl.clone())
                        })
                    });
                    (inherited, "short_span_carry_over".to_string(), None)
                }
                AttributionDecision::RejectedStaleEpoch { .. } => {
                    (None, "rejected_stale_epoch".to_string(), None)
                }
            };
            let receipt = UtteranceSpeakerReceipt {
                schema: "codescribe.utterance-speaker-receipt.v1".to_string(),
                session: self.session.clone(),
                capture_epoch: self.capture_epoch,
                span_id: span.span_id,
                sample_start: span.sample_start,
                sample_end: span.sample_end,
                sample_rate_hz: self.sample_rate_hz,
                speaker_label: speaker_label.clone(),
                decision_kind,
                cosine_sim,
                dynamic_cluster_count: self.tracker.dynamic_cluster_count(),
                word_indices: span.word_indices.clone(),
                model_sha256: WESPEAKER_MODEL_SHA256.to_string(),
            };
            tracing::info!(
                event = "utterance_speaker_attributed",
                session_id = %self.session,
                capture_epoch = self.capture_epoch,
                span_id = receipt.span_id,
                sample_start = receipt.sample_start,
                sample_end = receipt.sample_end,
                speaker_label = ?receipt.speaker_label,
                decision_kind = %receipt.decision_kind,
                cosine_sim = ?receipt.cosine_sim,
                dynamic_clusters = receipt.dynamic_cluster_count,
                "Attributed speaker to sealed utterance span"
            );
            self.append_jsonl_sidecar(&receipt);
            self.receipts.push(receipt.clone());
            emitted.push(receipt);
        }
        self.attributed_occurrences.insert(occurrence.clone());
        emitted
    }

    /// Finalize speaker attribution at `seal_terminal` (`STOP`) and write `~/.codescribe/sessions/<session>.diarization.json`.
    pub fn finalize_take(
        &mut self,
        ledger: &AcousticLedger,
        sealed_occurrences: &[crate::pipeline::acoustic_ledger::OccurrenceIdentity],
        mut resolve_pcm: impl FnMut(u64, u64) -> Option<(u64, Vec<f32>)>,
    ) -> Option<TakeDiarizationSummary> {
        for occ in sealed_occurrences {
            if !self.attributed_occurrences.contains(occ)
                && let Some((pcm_start, pcm)) = resolve_pcm(occ.sample_start, occ.sample_end)
            {
                let _ = self.observe_sealed_occurrence(ledger, occ, &pcm, pcm_start);
            }
        }
        let summary = TakeDiarizationSummary {
            schema: "codescribe.take-diarization.v1".to_string(),
            session: self.session.clone(),
            capture_epoch: self.capture_epoch,
            sample_rate_hz: self.sample_rate_hz,
            model_filename: WESPEAKER_MODEL_FILENAME.to_string(),
            model_sha256: WESPEAKER_MODEL_SHA256.to_string(),
            dynamic_cluster_count: self.tracker.dynamic_cluster_count(),
            utterances: self.receipts.clone(),
        };
        self.write_summary_sidecar(&summary);
        tracing::info!(
            event = "take_diarization_completed",
            session_id = %self.session,
            capture_epoch = self.capture_epoch,
            utterances = summary.utterances.len(),
            dynamic_clusters = summary.dynamic_cluster_count,
            "Completed take speaker diarization summary"
        );
        Some(summary)
    }

    fn valid_sidecar_session(session: &str) -> bool {
        (8..=80).contains(&session.len())
            && session
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    }

    fn append_jsonl_sidecar(&self, receipt: &UtteranceSpeakerReceipt) {
        if !Self::valid_sidecar_session(&self.session) {
            return;
        }
        let sessions_dir = crate::config::Config::config_dir().join("sessions");
        let _ = std::fs::create_dir_all(&sessions_dir);
        let path = sessions_dir.join(format!("{}.diarization.jsonl", self.session));
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            && let Ok(mut line) = serde_json::to_vec(receipt)
        {
            line.push(b'\n');
            let _ = std::io::Write::write_all(&mut file, &line);
        }
    }

    fn write_summary_sidecar(&self, summary: &TakeDiarizationSummary) {
        if !Self::valid_sidecar_session(&self.session) {
            return;
        }
        let sessions_dir = crate::config::Config::config_dir().join("sessions");
        let _ = std::fs::create_dir_all(&sessions_dir);
        let path = sessions_dir.join(format!("{}.diarization.json", self.session));
        if let Ok(bytes) = serde_json::to_vec_pretty(summary) {
            let _ = std::fs::write(path, bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::acoustic_ledger::{
        ObservationIdentity, ObservationProducer, OccurrenceIdentity, SlotWitness, WordSlot,
    };

    fn make_slot(
        session: &str,
        epoch: u64,
        occ_start: u64,
        occ_end: u64,
        start: u64,
        end: u64,
    ) -> WordSlot {
        WordSlot {
            sample_start: start,
            sample_end: end,
            text: "sealed".into(),
            producer: ObservationProducer::Apple,
            observation: ObservationIdentity::new(
                ObservationProducer::Apple,
                1,
                1,
                OccurrenceIdentity::new(session, epoch, occ_start, occ_end),
            ),
            witness: SlotWitness::Unwitnessed,
            confidence: None,
            surface_rewritten: false,
        }
    }

    fn seal_owner_in_ledger(
        ledger: &mut AcousticLedger,
        session: &str,
        epoch: u64,
        occ_start: u64,
        occ_end: u64,
        seq: u64,
    ) -> OccurrenceIdentity {
        use crate::pipeline::acoustic_ledger::{AcousticEvidence, EnergyCalibration};
        let owner = OccurrenceIdentity::new(session, epoch, occ_start, occ_end);
        let calibration = EnergyCalibration::new("diarization-test", 1.0, 1);
        let evidence = AcousticEvidence {
            occurrence: owner.clone(),
            duration_ms: 1_000.0,
            energy_integral: 100.0,
            mean_rms_dbfs: -20.0,
            peak_dbfs: -10.0,
            vad_open_sample: Some(occ_start),
            vad_close_sample: Some(occ_end),
            evidence_calibration_version: calibration.version.clone(),
        };
        assert!(ledger.qualify(&evidence, &calibration).is_qualified());
        ledger.schedule_frontier(owner.clone(), [ObservationProducer::Apple]);
        let obs = ObservationIdentity::new(ObservationProducer::Apple, seq, 1, owner.clone());
        assert!(ledger.admit(&obs, "sealed").grants_mutation());
        assert!(ledger.note_frontier_return(&owner, ObservationProducer::Apple));
        ledger.seal(&owner).expect("closed occurrence seals");
        owner
    }

    fn sealed_ledger_with_slots(
        session: &str,
        epoch: u64,
        occ_start: u64,
        occ_end: u64,
        word_ranges: &[(u64, u64)],
    ) -> (AcousticLedger, Vec<WordSlot>) {
        let mut ledger = AcousticLedger::new();
        let _ = seal_owner_in_ledger(&mut ledger, session, epoch, occ_start, occ_end, 1);
        let slots: Vec<WordSlot> = word_ranges
            .iter()
            .map(|&(s, e)| make_slot(session, epoch, occ_start, occ_end, s, e))
            .collect();
        (ledger, slots)
    }

    #[test]
    fn group_word_slots_respects_gap_and_single_occurrence_owner() {
        let s1 = make_slot("sess", 1, 0, 48_000, 0, 8_000);
        let s2 = make_slot("sess", 1, 0, 48_000, 8_800, 16_000);
        let s3 = make_slot("sess", 1, 0, 48_000, 32_000, 40_000);
        let s4_other_owner = make_slot("sess", 1, 48_000, 64_000, 48_100, 56_000);
        let slots = vec![s1, s2, s3, s4_other_owner];
        let before = slots.clone();

        let spans = group_word_slots_into_sealed_spans(&slots, 1_600, 1);
        assert_eq!(slots, before);
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].word_indices, vec![0, 1]);
        assert_eq!((spans[0].sample_start, spans[0].sample_end), (0, 16_000));
        assert_eq!(spans[1].word_indices, vec![2]);
        assert_eq!(spans[2].word_indices, vec![3]);
    }

    #[test]
    fn unsealed_occurrence_is_refused_before_embed_callback_runs() {
        let ledger = AcousticLedger::new();
        let slots = vec![make_slot("sess", 1, 0, 32_000, 0, 32_000)];
        let spans = group_word_slots_into_sealed_spans(&slots, 1_600, 1);
        let mut tracker =
            OnlineCentroidTracker::new("sess".into(), 1, CentroidTrackerConfig::for_16khz(4))
                .unwrap();
        let pcm = vec![0.1_f32; 32_000];
        let mut called = false;
        let err = tracker.attribute_sealed(
            SealedEmbeddingInput {
                ledger: &ledger,
                slots: &slots,
                span: &spans[0],
                pcm_sample_start: 0,
                pcm: &pcm,
                sample_rate_hz: 16_000,
            },
            |_| {
                called = true;
                SpeakerEmbedding::from_vector(vec![1.0, 0.0, 0.0, 0.0])
            },
        );
        assert_eq!(err, Err(AttributionError::UnsealedOwner));
        assert!(!called);
    }

    #[test]
    fn enrolled_anchor_stays_immutable_dynamic_cluster_updates_and_short_span_carries_over() {
        let (mut ledger, mut slots) =
            sealed_ledger_with_slots("sess", 1, 0, 32_000, &[(0, 32_000)]);
        let _ = seal_owner_in_ledger(&mut ledger, "sess", 1, 33_600, 40_000, 2);
        slots.push(make_slot("sess", 1, 33_600, 40_000, 33_600, 40_000));
        let _ = seal_owner_in_ledger(&mut ledger, "sess", 1, 48_000, 80_000, 3);
        slots.push(make_slot("sess", 1, 48_000, 80_000, 48_000, 80_000));
        let spans = group_word_slots_into_sealed_spans(&slots, 1_600, 1);
        assert_eq!(spans.len(), 3);

        let mut tracker =
            OnlineCentroidTracker::new("sess".into(), 1, CentroidTrackerConfig::for_16khz(4))
                .unwrap();
        tracker
            .enroll(
                "alice".into(),
                SpeakerEmbedding::from_vector(vec![1.0, 0.0, 0.0, 0.0]).unwrap(),
            )
            .unwrap();
        let pcm = vec![0.05_f32; 80_000];

        let r1 = tracker
            .attribute_sealed(
                SealedEmbeddingInput {
                    ledger: &ledger,
                    slots: &slots,
                    span: &spans[0],
                    pcm_sample_start: 0,
                    pcm: &pcm,
                    sample_rate_hz: 16_000,
                },
                |_| SpeakerEmbedding::from_vector(vec![0.98, 0.02, 0.0, 0.0]),
            )
            .unwrap();
        assert!(
            matches!(r1.decision, AttributionDecision::EnrolledSpeaker { ref speaker_id, .. } if speaker_id == "alice")
        );

        let r2 = tracker
            .attribute_sealed(
                SealedEmbeddingInput {
                    ledger: &ledger,
                    slots: &slots,
                    span: &spans[1],
                    pcm_sample_start: 0,
                    pcm: &pcm,
                    sample_rate_hz: 16_000,
                },
                |_| panic!("short span must not invoke embed callback"),
            )
            .unwrap();
        assert_eq!(
            r2.decision,
            AttributionDecision::ShortSpanCarryOver {
                inherited_speaker: Some("alice".into()),
                duration_samples: 6_400,
            }
        );

        let r3 = tracker
            .attribute_sealed(
                SealedEmbeddingInput {
                    ledger: &ledger,
                    slots: &slots,
                    span: &spans[2],
                    pcm_sample_start: 0,
                    pcm: &pcm,
                    sample_rate_hz: 16_000,
                },
                |_| SpeakerEmbedding::from_vector(vec![0.0, 1.0, 0.0, 0.0]),
            )
            .unwrap();
        assert_eq!(
            r3.decision,
            AttributionDecision::DynamicCluster {
                cluster_id: 0,
                cosine_sim: 1.0
            }
        );
        assert_eq!(tracker.dynamic_cluster_count(), 1);
    }

    #[test]
    fn fbank_extractor_produces_zero_mean_cmn_mel_frames() {
        let extractor = FbankExtractor::new(FbankConfig::default());
        let samples: Vec<f32> = (0..16_000)
            .map(|i| ((i as f32) * 2.0 * std::f32::consts::PI * 220.0 / 16_000.0).sin() * 0.25)
            .collect();
        let (num_frames, num_bins, feats) = extractor
            .extract(&samples)
            .expect("1s audio produces fbank");
        assert_eq!(num_frames, 98);
        assert_eq!(num_bins, 80);
        assert_eq!(feats.len(), 98 * 80);
        for bin in 0..num_bins {
            let mean: f32 = (0..num_frames)
                .map(|f| feats[f * num_bins + bin])
                .sum::<f32>()
                / num_frames as f32;
            assert!(mean.abs() < 1e-4, "CMN bin {bin} mean={mean}");
        }
    }

    #[test]
    fn live_session_speaker_tracker_writes_sidecars_hermetically() {
        let temp = tempfile::tempdir().expect("temp data dir");
        let _guard = crate::test_isolation::EnvGuard::set("CODESCRIBE_DATA_DIR", temp.path());
        let session_id = "test-live-diarization-session";
        let (mut ledger, _) = sealed_ledger_with_slots(session_id, 1, 0, 32_000, &[(0, 32_000)]);
        let occ1 = OccurrenceIdentity::new(session_id, 1, 0, 32_000);
        let occ2 = seal_owner_in_ledger(&mut ledger, session_id, 1, 33_600, 40_000, 2);

        let mut live = LiveSessionSpeakerTracker::for_session(session_id, 1, 16_000);
        let signal_a = vec![0.25_f32; 32_000];
        let r1 = live.observe_sealed_occurrence_with(&ledger, &occ1, &signal_a, 0, |_| {
            let mut v = vec![0.0_f32; WESPEAKER_EMBEDDING_DIM];
            v[0] = 1.0;
            SpeakerEmbedding::from_vector(v)
        });
        assert_eq!(r1.len(), 1);
        assert_eq!(r1[0].speaker_label.as_deref(), Some("SPEAKER_00"));
        assert_eq!(r1[0].decision_kind, "dynamic_cluster");

        let short_pcm = vec![0.1_f32; 6_400];
        let r2 = live.observe_sealed_occurrence_with(&ledger, &occ2, &short_pcm, 33_600, |_| {
            panic!("short span must not invoke embed callback")
        });
        assert_eq!(r2.len(), 1);
        assert_eq!(r2[0].speaker_label.as_deref(), Some("SPEAKER_00"));
        assert_eq!(r2[0].decision_kind, "short_span_carry_over");

        let summary = live
            .finalize_take(&ledger, &[occ1, occ2], |_, _| None)
            .expect("summary written");
        assert_eq!(summary.utterances.len(), 2);
        assert_eq!(summary.dynamic_cluster_count, 1);
        assert!(
            temp.path()
                .join("sessions")
                .join(format!("{session_id}.diarization.jsonl"))
                .is_file()
        );
        assert!(
            temp.path()
                .join("sessions")
                .join(format!("{session_id}.diarization.json"))
                .is_file()
        );
    }

    #[test]
    #[ignore = "explicit local ONNX probe; requires CODESCRIBE_WESPEAKER_ONNX_PATH"]
    fn real_wespeaker_onnx_probe_with_explicit_model_path() {
        let model_path = std::env::var("CODESCRIBE_WESPEAKER_ONNX_PATH")
            .map(std::path::PathBuf::from)
            .expect("set CODESCRIBE_WESPEAKER_ONNX_PATH to run real ONNX probe");
        let embedder = OrtWeSpeakerEmbedder::from_file(
            &model_path,
            WESPEAKER_EMBEDDING_DIM,
            FbankConfig::default(),
        )
        .expect("load real WeSpeaker ONNX model");
        let signal_a: Vec<f32> = (0..32_000)
            .map(|i| {
                let t = i as f32 / 16_000.0;
                (2.0 * std::f32::consts::PI * 140.0 * t).sin() * 0.3
            })
            .collect();
        let emb_a = embedder
            .embed_sealed_pcm(&signal_a)
            .expect("embed signal_a");
        assert_eq!(emb_a.dim, WESPEAKER_EMBEDDING_DIM);
    }
}
