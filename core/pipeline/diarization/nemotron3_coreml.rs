//! PCM-only word attribution and bounded streaming requests, without inference.
//!
//! Coordinates are half-open mono 16 kHz sample ranges in one capture epoch.
//! Callers must supply WordSlots projected onto that same timebase. Speaker slots
//! are anonymous session labels. Nothing here edits a word or mints an occurrence.

use std::collections::VecDeque;

pub const NEMOTRON_SAMPLE_RATE: u32 = 16_000;
pub const NEMOTRON_FRAME_SAMPLES: u64 = 160;
pub const NEMOTRON_SPEAKER_SLOTS: usize = 8;
pub const IDENTITY_SPEAKER_MAP: [u8; 8] = [0, 1, 2, 3, 4, 5, 6, 7];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nemotron3Profile {
    Offline,
    Low1040ms,
    Fast1040ms,
    Fast32_2880ms,
}

impl Nemotron3Profile {
    pub const fn preset_name(self) -> &'static str {
        match self {
            Self::Offline => "offline",
            Self::Low1040ms => "low",
            Self::Fast1040ms => "fast",
            Self::Fast32_2880ms => "fast32",
        }
    }

    /// New audio scored by one call; the rest of the window is right context.
    pub const fn core_samples(self) -> usize {
        match self {
            Self::Offline => 435_200,
            Self::Low1040ms | Self::Fast1040ms => 11_520,
            Self::Fast32_2880ms => 40_960,
        }
    }

    pub const fn right_context_samples(self) -> usize {
        match self {
            Self::Offline => 51_200,
            Self::Low1040ms | Self::Fast1040ms | Self::Fast32_2880ms => 5_120,
        }
    }

    /// Input-buffer budget only. This is not measured end-to-end latency.
    pub const fn window_samples(self) -> usize {
        self.core_samples() + self.right_context_samples()
    }
}

/// Historical card name: these are independent probabilities, not raw logits.
/// Each value is finite and in [0, 1]; their sum need not be one.
#[derive(Debug, Clone, PartialEq)]
pub struct NemotronFrameLogits {
    pub frame_index: u64,
    pub sample_start: u64,
    pub sample_end: u64,
    pub speaker_probs: [f32; 8],
}

#[derive(Debug, Clone, PartialEq)]
pub struct WordNemotronAttribution {
    pub word_index: usize,
    pub sample_start: u64,
    pub sample_end: u64,
    pub primary_speaker_slot: Option<u8>,
    pub primary_posterior: f32,
    pub secondary_speaker_slot: Option<u8>,
    pub secondary_posterior: f32,
    pub is_overlap: bool,
}

impl WordNemotronAttribution {
    fn unknown(word_index: usize, sample_start: u64, sample_end: u64) -> Self {
        Self {
            word_index,
            sample_start,
            sample_end,
            primary_speaker_slot: None,
            primary_posterior: 0.0,
            secondary_speaker_slot: None,
            secondary_posterior: 0.0,
            is_overlap: false,
        }
    }
}

fn valid_probability(value: f32) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

/// Validate an ordered, possibly sparse frame timeline. Duplicate/overlapping
/// evidence is refused rather than counted twice. Only the final frame may be
/// shorter than 160 samples. Frame indices use one epoch-local 10 ms grid.
fn valid_frame_timeline(frames: &[NemotronFrameLogits]) -> bool {
    let Some(first) = frames.first() else {
        return true;
    };
    let mut previous_end = first.sample_start;
    let mut previous_index = None;
    for (index, frame) in frames.iter().enumerate() {
        let Some(length) = frame.sample_end.checked_sub(frame.sample_start) else {
            return false;
        };
        let expected_start = frame
            .frame_index
            .checked_sub(first.frame_index)
            .and_then(|offset| offset.checked_mul(NEMOTRON_FRAME_SAMPLES))
            .and_then(|offset| first.sample_start.checked_add(offset));
        if length == 0
            || length > NEMOTRON_FRAME_SAMPLES
            || (length < NEMOTRON_FRAME_SAMPLES && index + 1 != frames.len())
            || expected_start != Some(frame.sample_start)
            || frame.sample_start < previous_end
            || previous_index.is_some_and(|old| frame.frame_index <= old)
            || !frame.speaker_probs.iter().copied().all(valid_probability)
        {
            return false;
        }
        previous_end = frame.sample_end;
        previous_index = Some(frame.frame_index);
    }
    true
}

/// Integrate p(slot) * intersecting PCM samples over each existing WordSlot.
///
/// The denominator is the WHOLE word range: missing frames contribute zero
/// evidence, so sparse coverage cannot inflate a posterior. Ties choose the
/// lowest slot. Silence remains unknown even with a zero activity threshold.
/// Invalid thresholds or a malformed timeline yield unknown for every word.
///
/// A secondary label requires both its word posterior to meet the overlap
/// threshold and an intersecting frame with both selected speakers active.
/// Sequential A/B turns within a word therefore do not manufacture overlap.
/// Call once per epoch with session-canonical speaker columns (see the ring).
pub fn attribute_word_slots_from_nemotron_frames(
    slots: &[crate::pipeline::acoustic_ledger::WordSlot],
    frames: &[NemotronFrameLogits],
    active_threshold: f32,
    overlap_threshold: f32,
) -> Vec<WordNemotronAttribution> {
    let valid = valid_probability(active_threshold)
        && valid_probability(overlap_threshold)
        && valid_frame_timeline(frames);
    slots
        .iter()
        .enumerate()
        .map(|(index, word)| {
            if valid {
                attribute_range(
                    index,
                    word.sample_start,
                    word.sample_end,
                    frames,
                    active_threshold,
                    overlap_threshold,
                )
            } else {
                WordNemotronAttribution::unknown(index, word.sample_start, word.sample_end)
            }
        })
        .collect()
}

fn attribute_range(
    word_index: usize,
    start: u64,
    end: u64,
    frames: &[NemotronFrameLogits],
    active_threshold: f32,
    overlap_threshold: f32,
) -> WordNemotronAttribution {
    let mut result = WordNemotronAttribution::unknown(word_index, start, end);
    if end <= start {
        return result;
    }
    let first = frames.partition_point(|frame| frame.sample_end <= start);
    let intersecting = &frames[first..];
    let mut integrals = [0.0_f64; NEMOTRON_SPEAKER_SLOTS];
    for frame in intersecting
        .iter()
        .take_while(|frame| frame.sample_start < end)
    {
        let samples = frame.sample_end.min(end) - frame.sample_start.max(start);
        for (sum, probability) in integrals.iter_mut().zip(frame.speaker_probs) {
            *sum += f64::from(probability) * samples as f64;
        }
    }
    let means = integrals.map(|sum| (sum / (end - start) as f64) as f32);
    let mut primary = 0;
    for (slot, probability) in means.iter().enumerate().skip(1) {
        if *probability > means[primary] {
            primary = slot;
        }
    }
    if means[primary] <= 0.0 || means[primary] < active_threshold {
        return result;
    }
    result.primary_speaker_slot = Some(primary as u8);
    result.primary_posterior = means[primary];
    let mut secondary = usize::from(primary == 0);
    for (slot, probability) in means.iter().enumerate() {
        if slot != primary && *probability > means[secondary] {
            secondary = slot;
        }
    }
    let coactive = intersecting
        .iter()
        .take_while(|frame| frame.sample_start < end)
        .any(|frame| {
            frame.speaker_probs[primary] > 0.0
                && frame.speaker_probs[primary] >= active_threshold
                && frame.speaker_probs[secondary] > 0.0
                && frame.speaker_probs[secondary] >= overlap_threshold
        });
    if means[secondary] > 0.0 && means[secondary] >= overlap_threshold && coactive {
        result.secondary_speaker_slot = Some(secondary as u8);
        result.secondary_posterior = means[secondary];
        result.is_overlap = true;
    }
    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NemotronInputError {
    OfflineNeedsPostPass,
    StaleEpoch,
    EpochMustAdvance,
    InvalidSpeakerMap,
    SpeakerMapChanged,
    InputFinished,
    RequestInFlight,
    NoRequestInFlight,
    RequestMismatch,
    NonContiguousAudio,
    NonFiniteAudio,
    BufferFull,
    SampleOverflow,
    InvalidFrameBatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NemotronRingPhase {
    Buffering,
    WaitingForFrames,
    Draining,
    Finished,
}

/// Owned request for the future Swift actor. Trailing zeros are model padding,
/// never captured PCM. `valid_samples` includes any real right context;
/// only [sample_start, core_sample_end) may be emitted as activity frames.
#[derive(Debug, Clone)]
pub struct NemotronAudioWindow {
    pub session_epoch: u64,
    pub request_id: u64,
    pub profile: Nemotron3Profile,
    pub source_to_session: [u8; 8],
    pub first_frame_index: u64,
    pub sample_start: u64,
    pub core_sample_end: u64,
    pub valid_samples: usize,
    pub is_final: bool,
    pub pcm_16khz: Vec<f32>,
}

/// Bridge columns are still source-ordered here; the ring remaps exactly once.
#[derive(Debug, Clone)]
pub struct NemotronFrameBatch {
    pub session_epoch: u64,
    pub request_id: u64,
    pub source_to_session: [u8; 8],
    pub frames: Vec<NemotronFrameLogits>,
}

fn valid_speaker_map(mapping: [u8; 8]) -> bool {
    let mut seen = [false; NEMOTRON_SPEAKER_SLOTS];
    for slot in mapping {
        let Some(entry) = seen.get_mut(usize::from(slot)) else {
            return false;
        };
        if *entry {
            return false;
        }
        *entry = true;
    }
    true
}

/// Full coverage of one request, with an exactly clipped final 10 ms frame.
pub(super) fn validate_output_frames(
    frames: &[NemotronFrameLogits],
    sample_start: u64,
    sample_end: u64,
    first_frame_index: u64,
) -> Result<(), NemotronInputError> {
    let span = sample_end
        .checked_sub(sample_start)
        .ok_or(NemotronInputError::InvalidFrameBatch)?;
    let count = span.div_ceil(NEMOTRON_FRAME_SAMPLES);
    if u64::try_from(frames.len()).ok() != Some(count) {
        return Err(NemotronInputError::InvalidFrameBatch);
    }
    first_frame_index
        .checked_add(count)
        .ok_or(NemotronInputError::SampleOverflow)?;
    let mut start = sample_start;
    for (index, frame) in (first_frame_index..).zip(frames.iter()) {
        let end = start.saturating_add(NEMOTRON_FRAME_SAMPLES).min(sample_end);
        if frame.frame_index != index
            || frame.sample_start != start
            || frame.sample_end != end
            || !frame.speaker_probs.iter().copied().all(valid_probability)
        {
            return Err(NemotronInputError::InvalidFrameBatch);
        }
        start = end;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct InFlightWindow {
    request_id: u64,
    core_samples: usize,
}

/// One serialized, bounded PCM request stream; no recorder or model lives here.
/// Fast32 retains at most 46_080 samples; low/fast retain at most 16_640.
/// Feed at most remaining_capacity(), request a window, accept its batch, repeat.
/// A full ring applies backpressure; it never evicts unscored audio.
///
/// FluidAudio's arrival-ordered columns retain their meaning via its Swift-owned
/// cache/FIFO state. The fixed bijection is pinned for an entire epoch. Changed
/// mappings are refused: probabilities alone cannot prove a speaker permutation.
#[derive(Debug)]
pub struct StreamingNemotronRing {
    profile: Nemotron3Profile,
    session_epoch: u64,
    source_to_session: [u8; 8],
    pending_pcm: VecDeque<f32>,
    received_end: u64,
    next_core_start: u64,
    next_frame_index: u64,
    next_request_id: u64,
    in_flight: Option<InFlightWindow>,
    input_finished: bool,
}

impl StreamingNemotronRing {
    pub fn new(
        profile: Nemotron3Profile,
        session_epoch: u64,
        sample_origin: u64,
        source_to_session: [u8; 8],
    ) -> Result<Self, NemotronInputError> {
        if profile == Nemotron3Profile::Offline {
            return Err(NemotronInputError::OfflineNeedsPostPass);
        }
        if !valid_speaker_map(source_to_session) {
            return Err(NemotronInputError::InvalidSpeakerMap);
        }
        Ok(Self {
            profile,
            session_epoch,
            source_to_session,
            pending_pcm: VecDeque::with_capacity(profile.window_samples()),
            received_end: sample_origin,
            next_core_start: sample_origin,
            next_frame_index: 0,
            next_request_id: 0,
            in_flight: None,
            input_finished: false,
        })
    }

    pub fn phase(&self) -> NemotronRingPhase {
        if self.in_flight.is_some() {
            NemotronRingPhase::WaitingForFrames
        } else if !self.input_finished {
            NemotronRingPhase::Buffering
        } else if self.pending_pcm.is_empty() {
            NemotronRingPhase::Finished
        } else {
            NemotronRingPhase::Draining
        }
    }

    pub fn remaining_capacity(&self) -> usize {
        self.profile.window_samples() - self.pending_pcm.len()
    }

    /// All input validation precedes mutation; rejected audio can be retried.
    pub fn push_audio(
        &mut self,
        session_epoch: u64,
        sample_start: u64,
        samples: &[f32],
    ) -> Result<(), NemotronInputError> {
        self.check_epoch(session_epoch)?;
        if self.input_finished {
            return Err(NemotronInputError::InputFinished);
        }
        if self.in_flight.is_some() {
            return Err(NemotronInputError::RequestInFlight);
        }
        if sample_start != self.received_end {
            return Err(NemotronInputError::NonContiguousAudio);
        }
        if samples.len() > self.remaining_capacity() {
            return Err(NemotronInputError::BufferFull);
        }
        if !samples.iter().all(|sample| sample.is_finite()) {
            return Err(NemotronInputError::NonFiniteAudio);
        }
        let count = u64::try_from(samples.len()).map_err(|_| NemotronInputError::SampleOverflow)?;
        let end = sample_start
            .checked_add(count)
            .ok_or(NemotronInputError::SampleOverflow)?;
        self.pending_pcm.extend(samples.iter().copied());
        self.received_end = end;
        Ok(())
    }

    /// Stop marks an input boundary without holding a microphone. The caller
    /// continues requesting final windows until phase() is Finished.
    pub fn finish_input(&mut self, session_epoch: u64) -> Result<(), NemotronInputError> {
        self.check_epoch(session_epoch)?;
        self.input_finished = true;
        Ok(())
    }

    pub fn next_window(&mut self) -> Result<Option<NemotronAudioWindow>, NemotronInputError> {
        if self.in_flight.is_some() {
            return Err(NemotronInputError::RequestInFlight);
        }
        if self.pending_pcm.is_empty() || (!self.input_finished && self.remaining_capacity() != 0) {
            return Ok(None);
        }
        let next_id = self
            .next_request_id
            .checked_add(1)
            .ok_or(NemotronInputError::SampleOverflow)?;
        let core_samples = self.pending_pcm.len().min(self.profile.core_samples());
        let core_sample_end = self
            .next_core_start
            .checked_add(core_samples as u64)
            .ok_or(NemotronInputError::SampleOverflow)?;
        self.next_frame_index
            .checked_add((core_samples as u64).div_ceil(NEMOTRON_FRAME_SAMPLES))
            .ok_or(NemotronInputError::SampleOverflow)?;
        let mut pcm_16khz: Vec<_> = self.pending_pcm.iter().copied().collect();
        pcm_16khz.resize(self.profile.window_samples(), 0.0);
        let window = NemotronAudioWindow {
            session_epoch: self.session_epoch,
            request_id: self.next_request_id,
            profile: self.profile,
            source_to_session: self.source_to_session,
            first_frame_index: self.next_frame_index,
            sample_start: self.next_core_start,
            core_sample_end,
            valid_samples: self.pending_pcm.len(),
            is_final: self.input_finished,
            pcm_16khz,
        };
        self.in_flight = Some(InFlightWindow {
            request_id: self.next_request_id,
            core_samples,
        });
        self.next_request_id = next_id;
        Ok(Some(window))
    }

    /// Validate the entire reply before draining PCM or advancing any watermark.
    /// Old epochs, duplicate replies, gaps, padded frames, NaNs and map changes
    /// cannot silently revise already returned activity.
    pub fn accept_batch(
        &mut self,
        batch: NemotronFrameBatch,
    ) -> Result<Vec<NemotronFrameLogits>, NemotronInputError> {
        self.check_epoch(batch.session_epoch)?;
        let flight = self
            .in_flight
            .ok_or(NemotronInputError::NoRequestInFlight)?;
        if batch.request_id != flight.request_id {
            return Err(NemotronInputError::RequestMismatch);
        }
        if batch.source_to_session != self.source_to_session {
            return Err(NemotronInputError::SpeakerMapChanged);
        }
        let core_end = self.next_core_start + flight.core_samples as u64;
        validate_output_frames(
            &batch.frames,
            self.next_core_start,
            core_end,
            self.next_frame_index,
        )?;
        let mut frames = batch.frames;
        for frame in &mut frames {
            let mut canonical = [0.0; NEMOTRON_SPEAKER_SLOTS];
            for (source, target) in self.source_to_session.iter().enumerate() {
                canonical[usize::from(*target)] = frame.speaker_probs[source];
            }
            frame.speaker_probs = canonical;
        }
        drop(self.pending_pcm.drain(..flight.core_samples));
        self.next_core_start = core_end;
        self.next_frame_index += frames.len() as u64;
        self.in_flight = None;
        Ok(frames)
    }

    /// Epochs are strictly increasing, including cancellation/restart. Reset the
    /// Swift actor/cache as well before submitting any request of this epoch.
    pub fn reset(
        &mut self,
        new_epoch: u64,
        sample_origin: u64,
        source_to_session: [u8; 8],
    ) -> Result<(), NemotronInputError> {
        if new_epoch <= self.session_epoch {
            return Err(NemotronInputError::EpochMustAdvance);
        }
        let replacement = Self::new(self.profile, new_epoch, sample_origin, source_to_session)?;
        *self = replacement;
        Ok(())
    }

    fn check_epoch(&self, session_epoch: u64) -> Result<(), NemotronInputError> {
        if session_epoch != self.session_epoch {
            return Err(NemotronInputError::StaleEpoch);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::acoustic_ledger::{
        ObservationIdentity, ObservationProducer, OccurrenceIdentity, SlotWitness, WordSlot,
    };

    fn word(start: u64, end: u64) -> WordSlot {
        WordSlot {
            sample_start: start,
            sample_end: end,
            text: "same word".into(),
            producer: ObservationProducer::Apple,
            observation: ObservationIdentity::new(
                ObservationProducer::Apple,
                1,
                1,
                OccurrenceIdentity::new("diarization-fixture", 9, start, end),
            ),
            witness: SlotWitness::Unwitnessed,
            confidence: None,
            surface_rewritten: false,
        }
    }

    fn frame(index: u64, first: f32, second: f32) -> NemotronFrameLogits {
        NemotronFrameLogits {
            frame_index: index,
            sample_start: index * 160,
            sample_end: (index + 1) * 160,
            speaker_probs: [first, second, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        }
    }

    fn ring() -> StreamingNemotronRing {
        StreamingNemotronRing::new(Nemotron3Profile::Fast32_2880ms, 9, 0, IDENTITY_SPEAKER_MAP)
            .unwrap()
    }

    fn reply(window: &NemotronAudioWindow) -> NemotronFrameBatch {
        let count = (window.core_sample_end - window.sample_start).div_ceil(160);
        let frames = (0..count)
            .map(|offset| NemotronFrameLogits {
                frame_index: window.first_frame_index + offset,
                sample_start: window.sample_start + offset * 160,
                sample_end: (window.sample_start + (offset + 1) * 160).min(window.core_sample_end),
                speaker_probs: [0.9, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            })
            .collect();
        NemotronFrameBatch {
            session_epoch: window.session_epoch,
            request_id: window.request_id,
            source_to_session: window.source_to_session,
            frames,
        }
    }

    #[test]
    fn repeated_text_keeps_two_pcm_slots_and_leaves_ledger_words_unchanged() {
        let words = vec![word(0, 160), word(160, 320)];
        let before = words.clone();
        let result = attribute_word_slots_from_nemotron_frames(
            &words,
            &[frame(0, 0.9, 0.0), frame(1, 0.0, 0.9)],
            0.5,
            0.5,
        );
        assert_eq!(words, before);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].word_index, 0);
        assert_eq!(result[1].word_index, 1);
        assert_eq!(result[0].primary_speaker_slot, Some(0));
        assert_eq!(result[1].primary_speaker_slot, Some(1));
        assert_eq!((result[1].sample_start, result[1].sample_end), (160, 320));
    }

    #[test]
    fn word_edges_use_sample_weighted_integrals() {
        let result = attribute_word_slots_from_nemotron_frames(
            &[word(150, 300)],
            &[frame(0, 0.2, 0.8), frame(1, 0.9, 0.1)],
            0.5,
            0.5,
        );
        assert_eq!(result[0].primary_speaker_slot, Some(0));
        assert!((result[0].primary_posterior - 128.0 / 150.0).abs() < 0.00001);
    }

    #[test]
    fn missing_audio_and_silence_do_not_become_confident_speakers() {
        let sparse = attribute_word_slots_from_nemotron_frames(
            &[word(0, 1600)],
            &[frame(0, 1.0, 0.0)],
            0.5,
            0.5,
        );
        assert_eq!(sparse[0].primary_speaker_slot, None);
        let silence = attribute_word_slots_from_nemotron_frames(
            &[word(0, 160), word(160, 160)],
            &[frame(0, 0.0, 0.0)],
            0.0,
            0.0,
        );
        assert!(
            silence
                .iter()
                .all(|value| value.primary_speaker_slot.is_none())
        );
    }

    #[test]
    fn sequential_turns_do_not_manufacture_overlap_but_coactivity_does() {
        let turns = attribute_word_slots_from_nemotron_frames(
            &[word(0, 320)],
            &[frame(0, 0.9, 0.0), frame(1, 0.0, 0.9)],
            0.4,
            0.4,
        );
        assert_eq!(turns[0].primary_speaker_slot, Some(0));
        assert!(!turns[0].is_overlap);
        let overlap = attribute_word_slots_from_nemotron_frames(
            &[word(0, 160)],
            &[frame(0, 0.9, 0.8)],
            0.5,
            0.5,
        );
        assert_eq!(overlap[0].secondary_speaker_slot, Some(1));
        assert!(overlap[0].is_overlap);
    }

    #[test]
    fn duplicate_frames_and_nan_thresholds_refuse_attribution() {
        let duplicate = attribute_word_slots_from_nemotron_frames(
            &[word(0, 160)],
            &[frame(0, 1.0, 0.0), frame(0, 1.0, 0.0)],
            0.5,
            0.5,
        );
        assert_eq!(duplicate[0].primary_speaker_slot, None);
        let nan = attribute_word_slots_from_nemotron_frames(
            &[word(0, 160)],
            &[frame(0, 1.0, 0.0)],
            f32::NAN,
            0.5,
        );
        assert_eq!(nan[0].primary_speaker_slot, None);
    }

    #[test]
    fn profiles_keep_core_stride_distinct_from_buffer_latency() {
        assert_eq!(Nemotron3Profile::Fast32_2880ms.core_samples(), 40_960);
        assert_eq!(Nemotron3Profile::Fast32_2880ms.window_samples(), 46_080);
        assert_eq!(Nemotron3Profile::Low1040ms.window_samples(), 16_640);
        assert_eq!(Nemotron3Profile::Fast1040ms.window_samples(), 16_640);
        assert!(matches!(
            StreamingNemotronRing::new(Nemotron3Profile::Offline, 9, 0, IDENTITY_SPEAKER_MAP),
            Err(NemotronInputError::OfflineNeedsPostPass)
        ));
    }

    #[test]
    fn backpressure_and_noncontiguous_audio_are_transactional() {
        let mut ring = ring();
        assert_eq!(
            ring.push_audio(9, 0, &vec![0.0; 46_081]),
            Err(NemotronInputError::BufferFull)
        );
        assert_eq!(ring.remaining_capacity(), 46_080);
        assert_eq!(
            ring.push_audio(9, 1, &[0.0]),
            Err(NemotronInputError::NonContiguousAudio)
        );
        assert_eq!(
            ring.push_audio(9, 0, &[f32::NAN]),
            Err(NemotronInputError::NonFiniteAudio)
        );
        ring.push_audio(9, 0, &vec![0.0; 46_080]).unwrap();
        let window = ring.next_window().unwrap().unwrap();
        assert_eq!(window.core_sample_end, 40_960);
        assert_eq!(ring.phase(), NemotronRingPhase::WaitingForFrames);
        ring.accept_batch(reply(&window)).unwrap();
        assert_eq!(ring.remaining_capacity(), 40_960);
        ring.push_audio(9, 46_080, &vec![0.0; 40_960]).unwrap();
        assert_eq!(ring.next_window().unwrap().unwrap().sample_start, 40_960);
    }

    #[test]
    fn rejected_output_does_not_advance_and_duplicate_reply_cannot_replay() {
        let mut ring = ring();
        ring.push_audio(9, 0, &vec![0.0; 46_080]).unwrap();
        let window = ring.next_window().unwrap().unwrap();
        let mut bad = reply(&window);
        bad.frames[0].speaker_probs[0] = f32::NAN;
        assert_eq!(
            ring.accept_batch(bad),
            Err(NemotronInputError::InvalidFrameBatch)
        );
        assert_eq!(ring.remaining_capacity(), 0);
        ring.accept_batch(reply(&window)).unwrap();
        assert_eq!(
            ring.accept_batch(reply(&window)),
            Err(NemotronInputError::NoRequestInFlight)
        );
    }

    #[test]
    fn epoch_reset_rejects_old_callback_even_with_same_request_id() {
        let mut ring = ring();
        ring.push_audio(9, 0, &vec![0.0; 46_080]).unwrap();
        let old = ring.next_window().unwrap().unwrap();
        ring.reset(10, 8000, IDENTITY_SPEAKER_MAP).unwrap();
        ring.push_audio(10, 8000, &vec![0.0; 46_080]).unwrap();
        let new = ring.next_window().unwrap().unwrap();
        assert_eq!(old.request_id, new.request_id);
        assert_eq!(
            ring.accept_batch(reply(&old)),
            Err(NemotronInputError::StaleEpoch)
        );
        assert_eq!(
            ring.accept_batch(reply(&new)).unwrap()[0].sample_start,
            8000
        );
        assert_eq!(
            ring.reset(10, 0, IDENTITY_SPEAKER_MAP),
            Err(NemotronInputError::EpochMustAdvance)
        );
    }

    #[test]
    fn fixed_permutation_maps_once_and_changed_mapping_is_refused() {
        let mapping = [2, 1, 0, 3, 4, 5, 6, 7];
        let mut ring =
            StreamingNemotronRing::new(Nemotron3Profile::Fast1040ms, 9, 0, mapping).unwrap();
        ring.push_audio(9, 0, &vec![0.0; 16_640]).unwrap();
        let window = ring.next_window().unwrap().unwrap();
        let mut changed = reply(&window);
        changed.source_to_session = IDENTITY_SPEAKER_MAP;
        assert_eq!(
            ring.accept_batch(changed),
            Err(NemotronInputError::SpeakerMapChanged)
        );
        let frames = ring.accept_batch(reply(&window)).unwrap();
        assert_eq!(frames[0].speaker_probs[2], 0.9);
        assert_eq!(frames[0].speaker_probs[0], 0.0);
        assert!(matches!(
            StreamingNemotronRing::new(Nemotron3Profile::Fast1040ms, 9, 0, [0; 8]),
            Err(NemotronInputError::InvalidSpeakerMap)
        ));
    }

    #[test]
    fn final_tail_clips_padding_and_completes_without_new_audio() {
        let mut ring = ring();
        ring.push_audio(9, 0, &[0.5; 161]).unwrap();
        assert!(ring.next_window().unwrap().is_none());
        ring.finish_input(9).unwrap();
        let window = ring.next_window().unwrap().unwrap();
        assert_eq!(window.valid_samples, 161);
        assert_eq!(window.pcm_16khz.len(), 46_080);
        assert!(window.is_final);
        assert!(window.pcm_16khz[161..].iter().all(|sample| *sample == 0.0));
        let frames = ring.accept_batch(reply(&window)).unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!((frames[1].sample_start, frames[1].sample_end), (160, 161));
        assert_eq!(ring.phase(), NemotronRingPhase::Finished);
        assert!(ring.next_window().unwrap().is_none());
        assert_eq!(
            ring.push_audio(9, 161, &[0.0]),
            Err(NemotronInputError::InputFinished)
        );
    }
}
