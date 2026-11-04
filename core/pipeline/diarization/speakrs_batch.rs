//! Batch speaker metadata for one 16 kHz mono PCM epoch.
//!
//! Upstream API: avencera/speakrs tag v0.7.0, src/pipeline.rs,
//! src/pipeline/config.rs and src/clustering/ahc.rs. No automatic model downloads.
//! Callers must align slots and PCM to the same epoch and sample-zero origin.
//! Rebased excerpts require an explicit offset before joining.
//!
//! speakrs code is Apache-2.0. Its default pyannote-derived segmentation weights
//! need separate license/provenance verification. Production bundles require
//! reviewed CC-BY-4.0, Apache-2.0 or OpenMDW-1.1 weights and their notices.
//! No upstream benchmark number is a measurement of this experiment.

#[cfg(feature = "diarization-speakrs")]
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
#[cfg(feature = "diarization-speakrs")]
use std::path::Path;
use std::time::Duration;

pub const SAMPLE_RATE_HZ: u32 = 16_000;
pub const SPEAKRS_CODE_LICENSE: &str = "Apache-2.0";
pub const MODEL_LICENSE_NOTE: &str = "Default pyannote segmentation weights require separate license verification; \
     production weights must be reviewed CC-BY-4.0 / Apache-2.0 / OpenMDW-1.1.";

#[derive(Debug, Clone, PartialEq)]
pub struct SpeakerSegment {
    pub sample_start: u64,
    pub sample_end: u64,
    pub speaker_id: String,
    /// Zero means unavailable, not a calibrated low speaker probability.
    pub confidence: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WordSpeakerAttribution {
    pub word_index: usize,
    pub sample_start: u64,
    pub sample_end: u64,
    pub speaker_id: Option<String>,
    pub overlap_samples: u64,
    pub confidence: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiarizationMetrics {
    pub audio_duration_s: f64,
    pub wall_time_s: f64,
    /// Processing seconds / audio seconds, never the reciprocal RTFx.
    pub rtf: f64,
    pub peak_rss_bytes: Option<u64>,
    pub der_pct: Option<f64>,
    pub rttm_reference_used: bool,
}

impl DiarizationMetrics {
    /// Construct only from actual observations. RSS must come from an external
    /// process measurement; absence stays None. Model-load timing is the
    /// caller's responsibility and must be stated in the benchmark protocol.
    pub fn from_observation(
        audio_samples: usize,
        wall_time: Duration,
        peak_rss_bytes: Option<u64>,
        score: &DerScore,
    ) -> Result<Self, String> {
        if audio_samples == 0 || wall_time.is_zero() {
            return Err("positive audio duration and observed wall time are required".into());
        }
        let audio_duration_s = audio_samples as f64 / f64::from(SAMPLE_RATE_HZ);
        let wall_time_s = wall_time.as_secs_f64();
        Ok(Self {
            audio_duration_s,
            wall_time_s,
            rtf: wall_time_s / audio_duration_s,
            peak_rss_bytes,
            der_pct: score.der_pct,
            rttm_reference_used: score.rttm_reference_used,
        })
    }
}

pub trait BatchDiarizer {
    fn diarize_pcm16k(
        &self,
        samples: &[f32],
        sample_rate_hz: u32,
    ) -> Result<Vec<SpeakerSegment>, String>;
}

/// Provenance is a durable model-manifest/receipt reference, not a license
/// inferred from speakrs' own code license. Each model family is reviewed.
#[derive(Debug, Clone)]
pub struct ReviewedWeightLicenses {
    pub segmentation: String,
    pub embedding: String,
    pub plda: String,
    pub provenance: String,
}

impl ReviewedWeightLicenses {
    fn validate(&self) -> Result<(), String> {
        let permitted = ["CC-BY-4.0", "Apache-2.0", "OpenMDW-1.1"];
        if self.provenance.trim().is_empty()
            || [&self.segmentation, &self.embedding, &self.plda]
                .iter()
                .any(|license| !permitted.contains(&license.as_str()))
        {
            return Err(MODEL_LICENSE_NOTE.into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct SpeakrsBatchConfig {
    /// Upstream AHC dendrogram distance; finite and non-negative, not a probability.
    pub clustering_threshold: f32,
    /// v0.7.0 has no clustering speaker cap. This is an output postcondition:
    /// excess speakers return an error; they are never silently merged/dropped.
    pub max_speakers: Option<usize>,
    pub use_coreml: bool,
    pub weight_licenses: Option<ReviewedWeightLicenses>,
}

impl Default for SpeakrsBatchConfig {
    fn default() -> Self {
        Self {
            clustering_threshold: 0.6,
            max_speakers: None,
            use_coreml: true,
            weight_licenses: None,
        }
    }
}

impl SpeakrsBatchConfig {
    pub fn validate(&self) -> Result<(), String> {
        if !self.clustering_threshold.is_finite() || self.clustering_threshold < 0.0 {
            return Err("clustering_threshold must be finite and non-negative".into());
        }
        if self.max_speakers == Some(0) {
            return Err("max_speakers must be positive when supplied".into());
        }
        self.weight_licenses
            .as_ref()
            .ok_or_else(|| MODEL_LICENSE_NOTE.to_owned())?
            .validate()
    }
}

/// RefCell satisfies the immutable BatchDiarizer interface while upstream run
/// requires mutable scratch buffers. This handle is intentionally not Sync;
/// it is a post-pass component, never a new recorder or background worker.
#[cfg(feature = "diarization-speakrs")]
pub struct SpeakrsBatchDiarizer {
    pipeline: RefCell<speakrs::OwnedDiarizationPipeline>,
    config: SpeakrsBatchConfig,
}

#[cfg(feature = "diarization-speakrs")]
impl SpeakrsBatchDiarizer {
    pub fn from_local_models(
        models_dir: &Path,
        config: SpeakrsBatchConfig,
    ) -> Result<Self, String> {
        config.validate()?;
        let mode = if config.use_coreml {
            speakrs::ExecutionMode::CoreMl
        } else {
            speakrs::ExecutionMode::Cpu
        };
        let mut upstream = speakrs::PipelineConfig::for_mode(mode);
        let ahc = speakrs::AhcConfig::new(config.clustering_threshold)
            .map_err(|error| error.to_string())?;
        upstream.clustering = upstream.clustering.with_ahc(ahc);
        let pipeline = speakrs::PipelineBuilder::from_dir(models_dir, mode)
            .and_then(|builder| builder.pipeline(upstream).build())
            .map_err(|error| error.to_string())?;
        Ok(Self {
            pipeline: RefCell::new(pipeline),
            config,
        })
    }
}

#[cfg(feature = "diarization-speakrs")]
impl BatchDiarizer for SpeakrsBatchDiarizer {
    fn diarize_pcm16k(
        &self,
        samples: &[f32],
        sample_rate_hz: u32,
    ) -> Result<Vec<SpeakerSegment>, String> {
        if sample_rate_hz != SAMPLE_RATE_HZ {
            return Err("expected mono f32 PCM at exactly 16000 Hz; resample upstream".into());
        }
        if samples.iter().any(|sample| !sample.is_finite()) {
            return Err("PCM contains non-finite samples".into());
        }
        if samples.is_empty() {
            return Ok(Vec::new());
        }
        let sample_count = u64::try_from(samples.len())
            .map_err(|_| "PCM length does not fit the sample clock".to_owned())?;
        let mut pipeline = self
            .pipeline
            .try_borrow_mut()
            .map_err(|_| "diarization handle is already in use".to_owned())?;
        let result = pipeline.run(samples).map_err(|error| error.to_string())?;
        let mut segments = Vec::new();
        for turn in result.discrete_diarization.to_segments() {
            if turn.end <= turn.start {
                return Err("upstream returned an empty or reversed speaker turn".into());
            }
            let sample_start = seconds_to_samples(turn.start)?;
            let sample_end = seconds_to_samples(turn.end)?.min(sample_count);
            // The upstream frame grid may extend beyond the final input sample.
            // Clip only the speaker metadata, never the input PCM or word slots.
            if sample_start >= sample_end {
                continue;
            }
            let segment = SpeakerSegment {
                sample_start,
                sample_end,
                speaker_id: turn.speaker,
                // Upstream Segment exposes start, end and speaker only.
                confidence: 0.0,
            };
            validate_segment(&segment)?;
            segments.push(segment);
        }
        validate_speaker_limit(&segments, self.config.max_speakers)?;
        Ok(segments)
    }
}

#[cfg_attr(not(feature = "diarization-speakrs"), allow(dead_code))]
fn validate_speaker_limit(segments: &[SpeakerSegment], limit: Option<usize>) -> Result<(), String> {
    if let Some(limit) = limit {
        let count = segments
            .iter()
            .map(|segment| &segment.speaker_id)
            .collect::<BTreeSet<_>>()
            .len();
        if count > limit {
            return Err(format!(
                "diarizer returned {count} speakers, exceeding limit {limit}"
            ));
        }
    }
    Ok(())
}

/// One output per slot, in original order. The largest individual segment
/// overlap wins; ties use higher confidence then the earlier input segment.
/// Unknown assignments use None / zero overlap / zero confidence. Malformed
/// segments are ineligible; an invalid threshold makes every slot unknown.
/// No text comparison, identity mutation, nearest-speaker fill or gap bridging.
pub fn assign_speakers_to_word_slots(
    slots: &[crate::pipeline::acoustic_ledger::WordSlot],
    segments: &[SpeakerSegment],
    min_overlap_ratio: f32,
) -> Vec<WordSpeakerAttribution> {
    slots
        .iter()
        .enumerate()
        .map(|(word_index, slot)| {
            let mut attribution = WordSpeakerAttribution {
                word_index,
                sample_start: slot.sample_start,
                sample_end: slot.sample_end,
                speaker_id: None,
                overlap_samples: 0,
                confidence: 0.0,
            };
            let length = slot.sample_end.saturating_sub(slot.sample_start);
            if length == 0
                || !min_overlap_ratio.is_finite()
                || !(0.0..=1.0).contains(&min_overlap_ratio)
            {
                return attribution;
            }
            for segment in segments {
                if validate_segment(segment).is_err() {
                    continue;
                }
                let overlap = slot
                    .sample_end
                    .min(segment.sample_end)
                    .saturating_sub(slot.sample_start.max(segment.sample_start));
                if overlap == 0 || overlap as f64 / (length as f64) < f64::from(min_overlap_ratio) {
                    continue;
                }
                if overlap > attribution.overlap_samples
                    || (overlap == attribution.overlap_samples
                        && segment.confidence > attribution.confidence)
                {
                    attribution.speaker_id = Some(segment.speaker_id.clone());
                    attribution.overlap_samples = overlap;
                    attribution.confidence = segment.confidence;
                }
            }
            attribution
        })
        .collect()
}

fn valid_token(value: &str) -> bool {
    !value.is_empty()
        && value != "<NA>"
        && !value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
}

fn validate_segment(segment: &SpeakerSegment) -> Result<(), String> {
    if segment.sample_end <= segment.sample_start
        || !valid_token(&segment.speaker_id)
        || !segment.confidence.is_finite()
        || !(0.0..=1.0).contains(&segment.confidence)
    {
        return Err("invalid speaker interval, identifier or confidence".into());
    }
    Ok(())
}

fn seconds_to_samples(seconds: f64) -> Result<u64, String> {
    let samples = (seconds * f64::from(SAMPLE_RATE_HZ)).round();
    // The strict upper bound avoids f64 rounding u64::MAX upward to 2^64.
    if !seconds.is_finite() || seconds < 0.0 || samples >= u64::MAX as f64 {
        return Err("invalid or overflowing timestamp".into());
    }
    Ok(samples as u64)
}

/// RTTM uses millisecond precision per the cell contract. Sub-half-millisecond
/// durations are rejected rather than exported as zero-length speech. Rounding
/// can lose at most 0.5 ms independently in onset and duration.
pub fn export_rttm(uri: &str, segments: &[SpeakerSegment]) -> Result<String, String> {
    if !valid_token(uri) {
        return Err("RTTM URI must be one non-placeholder token".into());
    }
    let mut output = String::new();
    for segment in segments {
        validate_segment(segment)?;
        let start_s = segment.sample_start as f64 / f64::from(SAMPLE_RATE_HZ);
        let duration_s =
            (segment.sample_end - segment.sample_start) as f64 / f64::from(SAMPLE_RATE_HZ);
        if (duration_s * 1000.0).round() == 0.0 {
            return Err(
                "segment duration cannot be represented at RTTM millisecond precision".into(),
            );
        }
        writeln!(
            output,
            "SPEAKER {uri} 1 {start_s:.3} {duration_s:.3} <NA> <NA> {} <NA> <NA>",
            segment.speaker_id
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(output)
}

/// Parse SPEAKER rows for exactly one recording and channel 1. Wrong URI is an
/// error, not an empty reference with a misleading score. Other recording rows
/// are excluded, never mixed into this recording's time axis.
pub fn parse_rttm(text: &str, uri: &str) -> Result<Vec<SpeakerSegment>, String> {
    if !valid_token(uri) {
        return Err("invalid RTTM URI".into());
    }
    let mut segments = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() != 10 || fields[0] != "SPEAKER" {
            return Err(format!("malformed SPEAKER row at line {}", index + 1));
        }
        if fields[1] != uri {
            continue;
        }
        if fields[2] != "1" {
            return Err(format!("expected mono channel 1 at line {}", index + 1));
        }
        let start_s = fields[3]
            .parse::<f64>()
            .map_err(|error| error.to_string())?;
        let duration_s = fields[4]
            .parse::<f64>()
            .map_err(|error| error.to_string())?;
        if !duration_s.is_finite() || duration_s <= 0.0 {
            return Err(format!("invalid RTTM duration at line {}", index + 1));
        }
        let segment = SpeakerSegment {
            sample_start: seconds_to_samples(start_s)?,
            sample_end: seconds_to_samples(start_s + duration_s)?,
            speaker_id: fields[7].to_owned(),
            confidence: 0.0,
        };
        validate_segment(&segment)?;
        segments.push(segment);
    }
    if segments.is_empty() {
        return Err(format!("no SPEAKER rows for recording {uri}"));
    }
    Ok(segments)
}

/// Collar 0, overlap included, full union of reference and hypothesis support;
/// no UEM masking. Counts are speaker-samples, so overlapping reference speakers
/// each contribute to the denominator. Repeated same-speaker rows form a union.
#[derive(Debug, Clone, PartialEq)]
pub struct DerScore {
    pub false_alarm_samples: u128,
    pub missed_speech_samples: u128,
    pub speaker_confusion_samples: u128,
    pub reference_speaker_samples: u128,
    pub der_pct: Option<f64>,
    pub rttm_reference_used: bool,
}

impl DerScore {
    fn unavailable() -> Self {
        Self {
            false_alarm_samples: 0,
            missed_speech_samples: 0,
            speaker_confusion_samples: 0,
            reference_speaker_samples: 0,
            der_pct: None,
            rttm_reference_used: false,
        }
    }
}

/// Caller supplies an actual public RTTM reference and its provenance URL.
/// This function does not fetch URLs or certify annotation provenance. An
/// oracle/pseudo-reference must never be passed as public ground truth.
/// Missing reference remains None; supplied malformed references fail closed.
pub fn calculate_der(
    hypothesis: &[SpeakerSegment],
    public_rttm: Option<&str>,
    uri: &str,
    public_reference_url: Option<&str>,
) -> Result<DerScore, String> {
    let Some(rttm) = public_rttm else {
        return Ok(DerScore::unavailable());
    };
    let url =
        public_reference_url.ok_or_else(|| "public RTTM provenance is required".to_owned())?;
    if !(url.starts_with("https://") || url.starts_with("http://"))
        || url.split("://").nth(1).is_none_or(|host| host.is_empty())
    {
        return Err("public RTTM provenance must be an HTTP(S) URL".into());
    }
    score_segments(&parse_rttm(rttm, uri)?, hypothesis)
}

struct ScoringInterval {
    duration: u64,
    reference: Vec<usize>,
    hypothesis: Vec<usize>,
}

fn score_segments(
    reference: &[SpeakerSegment],
    hypothesis: &[SpeakerSegment],
) -> Result<DerScore, String> {
    for segment in reference.iter().chain(hypothesis) {
        validate_segment(segment)?;
    }
    let labels = |segments: &[SpeakerSegment]| -> BTreeMap<String, usize> {
        let names: BTreeSet<_> = segments
            .iter()
            .map(|segment| segment.speaker_id.clone())
            .collect();
        names
            .into_iter()
            .enumerate()
            .map(|(index, name)| (name, index))
            .collect()
    };
    let reference_labels = labels(reference);
    let hypothesis_labels = labels(hypothesis);
    let size = reference_labels.len().max(hypothesis_labels.len());
    // Bound the cubic label-assignment work and square allocation on input.
    if size > 256 {
        return Err("DER experiment supports at most 256 distinct speakers per recording".into());
    }
    let mut events = Vec::new();
    for (is_reference, segments, labels) in [
        (true, reference, &reference_labels),
        (false, hypothesis, &hypothesis_labels),
    ] {
        for segment in segments {
            let label = labels[&segment.speaker_id];
            events.push((segment.sample_start, is_reference, label, true));
            events.push((segment.sample_end, is_reference, label, false));
        }
    }
    events.sort_unstable_by_key(|event| event.0);
    let mut reference_counts = vec![0_usize; reference_labels.len()];
    let mut hypothesis_counts = vec![0_usize; hypothesis_labels.len()];
    let mut intervals = Vec::new();
    let mut previous = 0;
    let mut cursor = 0;
    while cursor < events.len() {
        let time = events[cursor].0;
        if time > previous {
            intervals.push(ScoringInterval {
                duration: time - previous,
                reference: reference_counts
                    .iter()
                    .enumerate()
                    .filter_map(|(label, count)| (*count > 0).then_some(label))
                    .collect(),
                hypothesis: hypothesis_counts
                    .iter()
                    .enumerate()
                    .filter_map(|(label, count)| (*count > 0).then_some(label))
                    .collect(),
            });
        }
        while cursor < events.len() && events[cursor].0 == time {
            let (_, is_reference, label, start) = events[cursor];
            let counts = if is_reference {
                &mut reference_counts
            } else {
                &mut hypothesis_counts
            };
            if start {
                counts[label] += 1;
            } else {
                counts[label] -= 1;
            }
            cursor += 1;
        }
        previous = time;
    }
    let mut weights = vec![vec![0_u64; size]; size];
    for interval in &intervals {
        for &reference in &interval.reference {
            for &hypothesis in &interval.hypothesis {
                weights[reference][hypothesis] += interval.duration;
            }
        }
    }
    let mapping = maximum_weight_assignment(&weights);
    let mut score = DerScore {
        rttm_reference_used: true,
        ..DerScore::unavailable()
    };
    for interval in intervals {
        let nr = interval.reference.len();
        let nh = interval.hypothesis.len();
        let correct = interval
            .hypothesis
            .iter()
            .filter(|&&hypothesis| interval.reference.contains(&mapping[hypothesis]))
            .count();
        let duration = u128::from(interval.duration);
        score.reference_speaker_samples += duration * nr as u128;
        score.missed_speech_samples += duration * nr.saturating_sub(nh) as u128;
        score.false_alarm_samples += duration * nh.saturating_sub(nr) as u128;
        score.speaker_confusion_samples += duration * (nr.min(nh) - correct) as u128;
    }
    if score.reference_speaker_samples > 0 {
        let errors = score.false_alarm_samples
            + score.missed_speech_samples
            + score.speaker_confusion_samples;
        score.der_pct = Some(100.0 * errors as f64 / score.reference_speaker_samples as f64);
    }
    Ok(score)
}

/// Exact maximum-weight one-to-one mapping, hypothesis column -> reference row.
/// Square padding handles unequal speaker counts. Hungarian minimization uses
/// integer potentials so tied/sample-sized costs do not depend on float error.
fn maximum_weight_assignment(weights: &[Vec<u64>]) -> Vec<usize> {
    let size = weights.len();
    let maximum = weights.iter().flatten().copied().max().unwrap_or(0);
    let mut row_potential = vec![0_i128; size + 1];
    let mut col_potential = vec![0_i128; size + 1];
    let mut matched_row = vec![0_usize; size + 1];
    let mut predecessor = vec![0_usize; size + 1];
    for row in 1..=size {
        matched_row[0] = row;
        let mut column = 0;
        let mut minimum = vec![i128::MAX / 4; size + 1];
        let mut used = vec![false; size + 1];
        loop {
            used[column] = true;
            let active_row = matched_row[column];
            let mut delta = i128::MAX / 4;
            let mut next = 0;
            for candidate in 1..=size {
                if used[candidate] {
                    continue;
                }
                let cost = i128::from(maximum)
                    - i128::from(weights[active_row - 1][candidate - 1])
                    - row_potential[active_row]
                    - col_potential[candidate];
                if cost < minimum[candidate] {
                    minimum[candidate] = cost;
                    predecessor[candidate] = column;
                }
                if minimum[candidate] < delta {
                    delta = minimum[candidate];
                    next = candidate;
                }
            }
            for candidate in 0..=size {
                if used[candidate] {
                    row_potential[matched_row[candidate]] += delta;
                    col_potential[candidate] -= delta;
                } else {
                    minimum[candidate] -= delta;
                }
            }
            column = next;
            if matched_row[column] == 0 {
                break;
            }
        }
        loop {
            let previous = predecessor[column];
            matched_row[column] = matched_row[previous];
            column = previous;
            if column == 0 {
                break;
            }
        }
    }
    matched_row.into_iter().skip(1).map(|row| row - 1).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::acoustic_ledger::{
        ObservationIdentity, ObservationProducer, OccurrenceIdentity, SlotWitness, WordSlot,
    };

    fn segment(start: u64, end: u64, speaker: &str, confidence: f32) -> SpeakerSegment {
        SpeakerSegment {
            sample_start: start,
            sample_end: end,
            speaker_id: speaker.to_owned(),
            confidence,
        }
    }

    fn slot(start: u64, end: u64) -> WordSlot {
        WordSlot {
            sample_start: start,
            sample_end: end,
            text: "same word".into(),
            producer: ObservationProducer::Apple,
            observation: ObservationIdentity::new(
                ObservationProducer::Apple,
                1,
                1,
                OccurrenceIdentity::new("test", 0, start, end),
            ),
            witness: SlotWitness::Unwitnessed,
            confidence: None,
            surface_rewritten: false,
        }
    }

    #[test]
    fn repeated_words_keep_independent_pcm_attribution_and_all_ledger_fields() {
        let words = vec![slot(0, 100), slot(100, 200)];
        let before = words.clone();
        let output = assign_speakers_to_word_slots(
            &words,
            &[segment(0, 100, "A", 0.0), segment(100, 200, "B", 0.0)],
            1.0,
        );
        assert_eq!(output[0].speaker_id.as_deref(), Some("A"));
        assert_eq!(output[1].speaker_id.as_deref(), Some("B"));
        assert_eq!(output[1].word_index, 1);
        assert_eq!(words, before);
    }

    #[test]
    fn overlap_precedes_confidence_then_input_index_breaks_ties() {
        let words = [slot(0, 100)];
        let segments = [
            segment(0, 50, "short", 1.0),
            segment(0, 80, "early", 0.8),
            segment(0, 80, "later", 0.8),
            segment(0, 80, "lower", 0.7),
        ];
        let output = assign_speakers_to_word_slots(&words, &segments, 0.5);
        assert_eq!(output[0].speaker_id.as_deref(), Some("early"));
        assert_eq!(output[0].overlap_samples, 80);
        let output = assign_speakers_to_word_slots(
            &words,
            &[segment(0, 80, "early", 0.1), segment(0, 80, "higher", 0.9)],
            0.5,
        );
        assert_eq!(output[0].speaker_id.as_deref(), Some("higher"));
    }

    #[test]
    fn touching_invalid_and_under_threshold_spans_stay_unknown() {
        let words = [slot(0, 100), slot(100, 100), slot(200, 100)];
        let segments = [
            segment(100, 200, "touch", 1.0),
            segment(0, 50, "half", 0.8),
            segment(0, 100, "nan", f32::NAN),
            segment(0, 100, "bad", -1.0),
        ];
        assert!(
            assign_speakers_to_word_slots(&words, &segments, 0.51)
                .iter()
                .all(|word| word.speaker_id.is_none())
        );
        for threshold in [f32::NAN, -0.1, 1.1] {
            assert!(
                assign_speakers_to_word_slots(&words, &segments, threshold)
                    .iter()
                    .all(|word| word.speaker_id.is_none())
            );
        }
    }

    #[test]
    fn rttm_round_trip_and_recording_channel_validation() {
        let segments = [segment(16_000, 40_000, "SPEAKER_00", 0.0)];
        let rttm = export_rttm("meeting", &segments).unwrap();
        assert_eq!(
            rttm,
            "SPEAKER meeting 1 1.000 1.500 <NA> <NA> SPEAKER_00 <NA> <NA>\n"
        );
        assert_eq!(parse_rttm(&rttm, "meeting").unwrap(), segments);
        assert!(parse_rttm(&rttm, "other").is_err());
        assert!(parse_rttm(&rttm.replace("meeting 1", "meeting 2"), "meeting").is_err());
        assert!(parse_rttm(&rttm.replace("1.500", "NaN"), "meeting").is_err());
        assert!(export_rttm("meeting\ninjected", &segments).is_err());
        assert!(export_rttm("meeting", &[segment(0, 1, "A", 0.0)]).is_err());
    }

    #[test]
    fn der_maps_anonymous_labels_and_unions_duplicate_reference_rows() {
        let reference = [
            segment(0, 100, "A", 0.0),
            segment(0, 100, "A", 0.0),
            segment(100, 200, "B", 0.0),
        ];
        let hypothesis = [segment(0, 100, "Y", 0.0), segment(100, 200, "X", 0.0)];
        let score = score_segments(&reference, &hypothesis).unwrap();
        assert_eq!(score.der_pct, Some(0.0));
        assert_eq!(score.reference_speaker_samples, 200);
        assert_eq!(
            maximum_weight_assignment(&[vec![9, 8], vec![8, 0]]),
            vec![1, 0]
        );
    }

    #[test]
    fn der_accounts_for_miss_false_alarm_and_confusion() {
        let reference = [
            segment(0, 16_000, "A", 0.0),
            segment(16_000, 32_000, "B", 0.0),
        ];
        let hypothesis = [
            segment(0, 8_000, "X", 0.0),
            segment(16_000, 24_000, "X", 0.0),
            segment(32_000, 40_000, "X", 0.0),
        ];
        let score = score_segments(&reference, &hypothesis).unwrap();
        assert_eq!(score.reference_speaker_samples, 32_000);
        assert_eq!(score.missed_speech_samples, 16_000);
        assert_eq!(score.false_alarm_samples, 8_000);
        assert_eq!(score.speaker_confusion_samples, 8_000);
        assert_eq!(score.der_pct, Some(100.0));
    }

    #[test]
    fn der_includes_overlap_and_handles_empty_hypothesis() {
        let reference = [segment(0, 100, "A", 0.0), segment(0, 100, "B", 0.0)];
        let score = score_segments(&reference, &[segment(0, 100, "X", 0.0)]).unwrap();
        assert_eq!(score.reference_speaker_samples, 200);
        assert_eq!(score.missed_speech_samples, 100);
        assert_eq!(score.der_pct, Some(50.0));
        assert_eq!(
            score_segments(&reference, &[]).unwrap().der_pct,
            Some(100.0)
        );
    }

    #[test]
    fn missing_public_reference_never_manufactures_der() {
        let score = calculate_der(&[], None, "meeting", None).unwrap();
        assert_eq!(score.der_pct, None);
        assert!(!score.rttm_reference_used);
        assert!(calculate_der(&[], Some("bad RTTM"), "meeting", None).is_err());
        let metrics =
            DiarizationMetrics::from_observation(32_000, Duration::from_secs(1), None, &score)
                .unwrap();
        assert_eq!(metrics.rtf, 0.5);
        assert_eq!(metrics.peak_rss_bytes, None);
        assert_eq!(metrics.der_pct, None);
        assert!(!metrics.rttm_reference_used);
        assert!(DiarizationMetrics::from_observation(0, Duration::ZERO, None, &score).is_err());
    }

    #[test]
    fn timestamp_conversion_and_output_speaker_limit_fail_closed() {
        for timestamp in [f64::NAN, f64::INFINITY, -0.1, u64::MAX as f64] {
            assert!(seconds_to_samples(timestamp).is_err());
        }
        let segments = [segment(0, 100, "A", 0.0), segment(100, 200, "B", 0.0)];
        assert!(validate_speaker_limit(&segments, Some(1)).is_err());
        assert!(validate_speaker_limit(&segments, Some(2)).is_ok());
    }

    #[test]
    fn configuration_requires_per_model_license_review() {
        let mut config = SpeakrsBatchConfig::default();
        assert!(config.validate().is_err());
        config.weight_licenses = Some(ReviewedWeightLicenses {
            segmentation: "CC-BY-4.0".into(),
            embedding: "Apache-2.0".into(),
            plda: "OpenMDW-1.1".into(),
            provenance: "reviewed-manifest-sha256".into(),
        });
        assert!(config.validate().is_ok());
        config.max_speakers = Some(0);
        assert!(config.validate().is_err());
        config.max_speakers = None;
        config.clustering_threshold = f32::NAN;
        assert!(config.validate().is_err());
    }
}
