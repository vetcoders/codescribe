//! W13-6B — span-based overlay highlights (lexicon corrections + speech gaps).
//!
//! The canvas stays append-only. Highlights are a read-only layer over
//! provenance that already exists after W13-3A: integer PCM sample ranges
//! plus the char offsets the overlay already receives on `ReplaceRange`.
//! Seconds never live here.
//!
//! Lane flag [`OVERLAY_HIGHLIGHTS_ENV`] is **default OFF**.

use crate::stt::tail_provider::{TailSampleRange, TimedTailSegment};

/// Opt-in gate for the overlay highlight layer. Unset / `0` / `false` / `off`
/// / `no` keep the shipped canvas unstyled.
pub const OVERLAY_HIGHLIGHTS_ENV: &str = "CODESCRIBE_OVERLAY_HIGHLIGHTS";

/// Visible gap glyph for a Silero-speech span that landed no words.
pub const SPEECH_GAP_MARKER: &str = "∅";

/// Kind of a canvas highlight. Typed evidence, not a confidence score.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayHighlightKind {
    /// A lexicon rewrite already applied to committed text.
    LexiconCorrected,
    /// Silero heard speech; no engine word landed in the span (pustka).
    SpeechGap,
}

impl OverlayHighlightKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LexiconCorrected => "lexicon_corrected",
            Self::SpeechGap => "speech_gap",
        }
    }
}

/// One highlight keyed by utterance identity and a half-open sample range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayHighlight {
    pub kind: OverlayHighlightKind,
    pub utterance_id: u64,
    /// Inclusive UTF-8 char start inside the utterance text *after* the edit.
    pub char_start: u64,
    /// Exclusive UTF-8 char end inside the utterance text *after* the edit.
    pub char_end: u64,
    pub range: TailSampleRange,
    /// Text the lexicon replaced (empty on a speech gap).
    pub before: String,
    /// Text now on the canvas (gap marker for a pustka).
    pub after: String,
}

/// Parse the overlay-highlights flag. `None` (unset) is OFF.
pub fn parse_overlay_highlights_flag(raw: Option<&str>) -> bool {
    match raw {
        Some(value) => {
            let value = value.trim().to_ascii_lowercase();
            matches!(value.as_str(), "1" | "true" | "yes" | "on")
        }
        None => false,
    }
}

/// Whether the highlight layer is armed. Default OFF.
pub fn overlay_highlights_enabled() -> bool {
    parse_overlay_highlights_flag(std::env::var(OVERLAY_HIGHLIGHTS_ENV).ok().as_deref())
}

/// Build a lexicon-correction highlight from a `ReplaceRange` already on the
/// bridge (`source = Lexicon`) plus optional 3A sample identity.
pub fn lexicon_corrected_highlight(
    utterance_id: u64,
    char_start: u64,
    replacement: &str,
    before: &str,
    range: TailSampleRange,
) -> Option<OverlayHighlight> {
    if replacement.trim().is_empty() {
        return None;
    }
    let char_end = char_start.saturating_add(replacement.chars().count() as u64);
    Some(OverlayHighlight {
        kind: OverlayHighlightKind::LexiconCorrected,
        utterance_id,
        char_start,
        char_end,
        range,
        before: before.to_string(),
        after: replacement.to_string(),
    })
}

/// A Silero-bounded speech span with no word evidence is a pustka.
///
/// Words whose sample range overlaps `speech` count as coverage. An empty
/// word list, or words that all sit outside the speech range, yields a gap.
pub fn speech_gap_highlight(
    utterance_id: u64,
    speech: TailSampleRange,
    words: &[TimedTailSegment],
) -> Option<OverlayHighlight> {
    if speech.sample_end <= speech.sample_start {
        return None;
    }
    let covered = words
        .iter()
        .any(|word| ranges_overlap(&speech, &word.range));
    if covered {
        return None;
    }
    Some(OverlayHighlight {
        kind: OverlayHighlightKind::SpeechGap,
        utterance_id,
        char_start: 0,
        char_end: 0,
        range: speech,
        before: String::new(),
        after: SPEECH_GAP_MARKER.to_string(),
    })
}

/// Empty `UtteranceFinal` after measured speech is the Swift-side pustka
/// signal (data already crossing the bridge: `on_final` + VAD / speech_pct).
pub fn empty_final_speech_gap(
    utterance_id: u64,
    text: &str,
    speech_was_active: bool,
    speech_pct: Option<f32>,
    range: TailSampleRange,
) -> Option<OverlayHighlight> {
    if !text.trim().is_empty() {
        return None;
    }
    let heard = speech_was_active || speech_pct.is_some_and(|pct| pct > 0.0);
    if !heard {
        return None;
    }
    speech_gap_highlight(utterance_id, range, &[])
}

fn ranges_overlap(left: &TailSampleRange, right: &TailSampleRange) -> bool {
    left.session == right.session
        && left.capture_epoch == right.capture_epoch
        && left.sample_start < right.sample_end
        && right.sample_start < left.sample_end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(start: u64, end: u64) -> TailSampleRange {
        TailSampleRange {
            session: "s1".into(),
            capture_epoch: 1,
            sample_start: start,
            sample_end: end,
        }
    }

    #[test]
    fn highlight_flag_defaults_off_and_accepts_truthy() {
        assert!(!parse_overlay_highlights_flag(None));
        assert!(!parse_overlay_highlights_flag(Some("")));
        assert!(!parse_overlay_highlights_flag(Some("0")));
        assert!(!parse_overlay_highlights_flag(Some("off")));
        assert!(!parse_overlay_highlights_flag(Some("false")));
        assert!(parse_overlay_highlights_flag(Some("1")));
        assert!(parse_overlay_highlights_flag(Some("ON")));
        assert!(parse_overlay_highlights_flag(Some(" true ")));
    }

    #[test]
    fn lexicon_highlight_pins_char_span_and_sample_range() {
        let highlight =
            lexicon_corrected_highlight(7, 4, "Junie", "uni agentka", range(16_000, 24_000))
                .expect("replacement");
        assert_eq!(highlight.kind, OverlayHighlightKind::LexiconCorrected);
        assert_eq!(highlight.utterance_id, 7);
        assert_eq!(highlight.char_start, 4);
        assert_eq!(highlight.char_end, 9);
        assert_eq!(highlight.before, "uni agentka");
        assert_eq!(highlight.after, "Junie");
        assert_eq!(highlight.range.sample_start, 16_000);
        assert_eq!(highlight.range.sample_end, 24_000);
        assert_eq!(highlight.kind.as_str(), "lexicon_corrected");
    }

    #[test]
    fn lexicon_highlight_rejects_empty_replacement() {
        assert!(lexicon_corrected_highlight(1, 0, "   ", "x", range(0, 10)).is_none());
    }

    #[test]
    fn speech_gap_when_silero_range_has_no_overlapping_words() {
        let words = [TimedTailSegment {
            text: "hello".into(),
            range: range(0, 1_000),
        }];
        let gap = speech_gap_highlight(3, range(8_000, 16_000), &words).expect("pustka");
        assert_eq!(gap.kind, OverlayHighlightKind::SpeechGap);
        assert_eq!(gap.after, SPEECH_GAP_MARKER);
        assert_eq!(gap.range.sample_start, 8_000);
        assert!(speech_gap_highlight(3, range(0, 500), &words).is_none());
        assert!(speech_gap_highlight(3, range(10, 10), &[]).is_none());
    }

    #[test]
    fn empty_final_becomes_gap_only_after_measured_speech() {
        assert!(empty_final_speech_gap(1, "słowo", true, Some(0.8), range(0, 100)).is_none());
        assert!(empty_final_speech_gap(1, "  ", false, None, range(0, 100)).is_none());
        assert!(empty_final_speech_gap(1, "", false, Some(0.0), range(0, 100)).is_none());
        let from_vad = empty_final_speech_gap(2, "", true, None, range(100, 200)).expect("vad");
        assert_eq!(from_vad.kind, OverlayHighlightKind::SpeechGap);
        let from_pct = empty_final_speech_gap(3, " \n", false, Some(0.4), range(200, 400))
            .expect("speech_pct");
        assert_eq!(from_pct.utterance_id, 3);
    }
}
