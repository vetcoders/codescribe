//! Per-session text postprocess pipeline: hallucination drops, overlap
//! dedup (text and timestamp based), and emitted-suffix tracking.

use crate::pipeline::contracts::TranscriptSegment;
use crate::pipeline::dedup::{strip_segment_overlap, strip_suffix_overlap_live};
use crate::pipeline::stream_postprocess::StreamPostProcessor;

use super::quality_gate::is_hallucination_with_quality;

// ── TranscriptionPipeline ────────────────────────────────────────────────────

/// Per-session postprocess state: what was already emitted, and what got dropped.
///
/// One instance lives per transcription session; the dedup fields are the memory
/// that lets successive Whisper windows be stitched without repeating their overlap.
pub(crate) struct TranscriptionPipeline {
    /// Language hint, forwarded to the hallucination heuristics.
    pub(crate) language: Option<String>,
    /// Lexicon and cleanup stage applied to each surviving utterance.
    pub(crate) postprocessor: StreamPostProcessor,
    /// Tail of the last emitted text, used for text-based overlap dedup.
    pub(crate) last_suffix: String,
    /// End timestamp of the newest emitted segment, used for timestamp dedup.
    pub(crate) last_segment_end_ts: Option<f32>,
    /// Count of utterances rejected as hallucinations (telemetry).
    pub(crate) hallucination_drops: u64,
    /// Count of utterances that became empty after overlap stripping (telemetry).
    pub(crate) overlap_strips: u64,
}

/// Reason a postprocess step dropped content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PostprocessDrop {
    /// Rejected by the hallucination heuristics before any dedup ran.
    Hallucination,
    /// Nothing remained after overlap with already-emitted text was stripped.
    OverlapEmpty,
    /// Text was empty after lexicon + cleanup (NOT semantic gate — utterance path
    /// never applies the embedding-based gate).
    FilteredEmpty,
}

impl TranscriptionPipeline {
    /// Start a fresh pipeline with empty dedup memory and zeroed counters.
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

    /// Strip the part of `text` that repeats the previously emitted suffix.
    pub(crate) fn strip_overlap(&self, text: &str) -> String {
        strip_suffix_overlap_live(&self.last_suffix, text)
    }

    /// Prefer timestamp-based dedup when segments carry usable end times, else
    /// fall back to [`Self::strip_overlap`].
    ///
    /// Returns the stripped text and the newest segment end timestamp, when the
    /// timestamp path was the one that applied.
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
        self.postprocess_with_reason_and_segments_with_quality(text, segments, None)
    }

    /// Segment-aware postprocess with engine confidence metadata.
    pub(crate) fn postprocess_with_reason_and_segments_with_quality(
        &mut self,
        text: &str,
        segments: &[TranscriptSegment],
        avg_logprob: Option<f32>,
    ) -> Result<String, PostprocessDrop> {
        if is_hallucination_with_quality(text, self.language.as_deref(), avg_logprob) {
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

    /// Remember the last 50 characters of emitted text as the next dedup anchor.
    ///
    /// Walks back by `char_indices` so the boundary lands on a character, not a
    /// byte — the transcripts are routinely non-ASCII.
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
