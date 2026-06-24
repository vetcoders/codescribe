//! Per-session text postprocess pipeline: hallucination drops, overlap
//! dedup (text and timestamp based), and emitted-suffix tracking.

use crate::pipeline::contracts::TranscriptSegment;
use crate::pipeline::dedup::{strip_segment_overlap, strip_suffix_overlap_live};
use crate::pipeline::stream_postprocess::StreamPostProcessor;

use super::quality_gate::is_hallucination;

// ── TranscriptionPipeline ────────────────────────────────────────────────────

pub(crate) struct TranscriptionPipeline {
    pub(crate) language: Option<String>,
    pub(crate) postprocessor: StreamPostProcessor,
    pub(crate) last_suffix: String,
    pub(crate) last_segment_end_ts: Option<f32>,
    pub(crate) hallucination_drops: u64,
    pub(crate) overlap_strips: u64,
}

/// Reason a postprocess step dropped content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
            last_segment_end_ts: None,
            hallucination_drops: 0,
            overlap_strips: 0,
        }
    }

    pub(crate) fn strip_overlap(&self, text: &str) -> String {
        strip_suffix_overlap_live(&self.last_suffix, text)
    }

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
        if is_hallucination(text, self.language.as_deref()) {
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
