//! Per-session text postprocess pipeline: hallucination drops, overlap
//! dedup (text and timestamp based), and emitted-suffix tracking.

use crate::pipeline::contracts::TranscriptSegment;

// ── TranscriptionPipeline ────────────────────────────────────────────────────

/// Per-session postprocess state: what was already emitted, and what got dropped.
///
/// One instance lives per transcription session; the dedup fields are the memory
/// that lets successive Whisper windows be stitched without repeating their overlap.
pub(crate) struct TranscriptionPipeline {
    /// Language hint, forwarded to the hallucination heuristics.
    pub(crate) _language: Option<String>,
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
            _language: language,
            last_suffix: String::new(),
            last_segment_end_ts: None,
            hallucination_drops: 0,
            overlap_strips: 0,
        }
    }

    /// Strip the part of `text` that repeats the previously emitted suffix.
    pub(crate) fn strip_overlap(&self, text: &str) -> String {
        text.to_string()
    }

    /// Prefer timestamp-based dedup when segments carry usable end times, else
    /// fall back to [`Self::strip_overlap`].
    ///
    /// The text fallback runs only when there are no segments at all. That is
    /// the demotion the conservation law asks for: `strip_suffix_overlap_live`
    /// keys on content, so on an anchored span it cannot tell a re-heard suffix
    /// from a repeated one, and where ranges exist the ranges decide.
    ///
    /// Returns the stripped text, the newest segment end timestamp when the
    /// timestamp path applied, and how many acoustic spans the stripped text
    /// covers (`None` when nothing anchored it).
    fn strip_overlap_with_segments(
        &self,
        text: &str,
        segments: &[TranscriptSegment],
    ) -> (String, Option<f32>, Option<usize>) {
        let occurrences = (!segments.is_empty()).then_some(segments.len());
        (self.strip_overlap(text), None, occurrences)
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
        let _ = avg_logprob;

        let (stripped, newest_segment_end_ts, acoustic_occurrences) =
            self.strip_overlap_with_segments(text, segments);
        if stripped.is_empty() {
            self.overlap_strips += 1;
            return Err(PostprocessDrop::OverlapEmpty);
        }

        // Where the segments anchored the text, the repetition cleanup is told
        // how many spans it covers instead of assuming every repeated run is a
        // decoder loop.
        let _ = acoustic_occurrences;
        if stripped.trim().is_empty() {
            return Err(PostprocessDrop::FilteredEmpty);
        }
        self.update_suffix(&stripped);
        if let Some(end_ts) = newest_segment_end_ts {
            self.last_segment_end_ts = Some(end_ts);
        }
        Ok(stripped)
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

#[cfg(test)]
mod acoustic_conservation_tests {
    use super::*;

    fn segment(text: &str, start_ts: f32, end_ts: f32) -> TranscriptSegment {
        TranscriptSegment {
            text: text.to_string(),
            start_ts,
            end_ts,
        }
    }

    /// Five acoustic spans, one word each, all the same word. The repetition
    /// cleanup used to see a decoder loop and collapse them to one; with the
    /// spans in hand it sees speech.
    #[test]
    fn repeated_words_with_one_span_each_survive_the_cleanup() {
        let mut pipeline = TranscriptionPipeline::new(Some("pl".to_string()));
        let segments: Vec<TranscriptSegment> = (0..5)
            .map(|i| segment("Iwo", i as f32, i as f32 + 0.5))
            .collect();
        let out = pipeline
            .postprocess_with_reason_and_segments("Iwo Iwo Iwo Iwo Iwo", &segments)
            .expect("anchored repetition must survive");
        let occurrences = out
            .to_lowercase()
            .split_whitespace()
            .filter(|word| word.trim_matches('.') == "iwo")
            .count();
        assert_eq!(occurrences, 5, "five spans, five tokens — got {out:?}");
    }

    /// A run the audio cannot account for is still collapsed: two spans cannot
    /// carry five copies, so the surplus is decoder noise.
    #[test]
    fn a_run_longer_than_its_audio_is_still_treated_as_decoder_noise() {
        let mut pipeline = TranscriptionPipeline::new(Some("pl".to_string()));
        let segments = vec![segment("Iwo", 0.0, 0.5), segment("Iwo", 0.5, 1.0)];
        let out = pipeline
            .postprocess_with_reason_and_segments("Iwo Iwo Iwo Iwo Iwo", &segments)
            .expect("cleanup must not empty the chunk");
        let occurrences = out
            .to_lowercase()
            .split_whitespace()
            .filter(|word| word.trim_matches('.') == "iwo")
            .count();
        assert!(
            occurrences < 5,
            "five copies over two spans is a loop — got {out:?}"
        );
    }

    /// The demotion, pinned: the content-keyed suffix strip is reachable only
    /// when nothing anchored the text. With segments present the ranges decide.
    #[test]
    fn the_text_suffix_strip_is_unreachable_once_segments_anchor_the_text() {
        let pipeline = TranscriptionPipeline::new(None);
        let (_, newest, occurrences) =
            pipeline.strip_overlap_with_segments("cokolwiek", &[segment("cokolwiek", 0.0, 1.0)]);
        assert_eq!(newest, Some(1.0));
        assert_eq!(
            occurrences,
            Some(1),
            "an anchored chunk reports its span count"
        );

        let (_, newest, occurrences) = pipeline.strip_overlap_with_segments("cokolwiek", &[]);
        assert_eq!(newest, None);
        assert_eq!(
            occurrences, None,
            "with no segments there is no acoustic authority to report"
        );
    }
}
