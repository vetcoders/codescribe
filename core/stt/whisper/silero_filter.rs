//! Silero-based post-filter for file-level Whisper transcription.

use crate::pipeline::contracts::{TranscriptSegment, VadClass};
use crate::vad::VadConfig;
use crate::vad::discriminator::VadTimeline;

#[derive(Debug, Clone)]
pub struct SileroFilterOutcome {
    pub text: String,
    pub segments: Vec<TranscriptSegment>,
    pub dropped_count: u32,
    pub dropped_text_samples: Vec<String>,
}

pub fn map_whisper_segments_to_silero(
    segments: &[TranscriptSegment],
    timeline: &VadTimeline,
    config: &VadConfig,
) -> SileroFilterOutcome {
    let mut kept_segments: Vec<TranscriptSegment> = Vec::with_capacity(segments.len());
    let mut dropped_count = 0u32;
    let mut dropped_text_samples = Vec::new();

    for segment in segments {
        let class = timeline
            .dominant_class(segment.start_ts, segment.end_ts)
            .unwrap_or(VadClass::Speech);

        if matches!(class, VadClass::TrailingSilence) && config.tail_drop_enabled {
            dropped_count = dropped_count.saturating_add(1);
            if dropped_text_samples.len() < 3 {
                dropped_text_samples.push(segment.text.clone());
            }
            tracing::debug!(
                target: "tail_silence_filter",
                start_sec = segment.start_ts,
                end_sec = segment.end_ts,
                text = %segment.text,
                "dropping Whisper segment in trailing silence"
            );
            continue;
        }

        let mut normalized = segment.clone();
        normalized.text = normalized.text.trim().to_string();
        if normalized.text.is_empty() {
            continue;
        }

        if let Some(previous) = kept_segments.last_mut() {
            let gap_class = timeline
                .dominant_class(previous.end_ts, normalized.start_ts)
                .unwrap_or(VadClass::Speech);

            match gap_class {
                VadClass::UtteranceGap => {
                    if !normalized.text.starts_with('…') {
                        normalized.text = format!("… {}", normalized.text);
                    }
                }
                VadClass::SentenceBoundary => {
                    if !ends_with_sentence_terminator(&previous.text) {
                        previous.text.push('.');
                    }
                }
                VadClass::Speech | VadClass::TrailingSilence => {}
            }
        }

        kept_segments.push(normalized);
    }

    let text = kept_segments
        .iter()
        .map(|segment| segment.text.trim())
        .filter(|segment: &&str| !segment.is_empty())
        .collect::<Vec<_>>()
        .join(" ");

    SileroFilterOutcome {
        text,
        segments: kept_segments,
        dropped_count,
        dropped_text_samples,
    }
}

fn ends_with_sentence_terminator(text: &str) -> bool {
    matches!(
        text.trim_end().chars().last(),
        Some('.') | Some('!') | Some('?') | Some('…')
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::contracts::VadClass;

    fn config_with_tail_drop(enabled: bool) -> VadConfig {
        VadConfig {
            tail_drop_enabled: enabled,
            ..VadConfig::default()
        }
    }

    fn timeline(classes: &[VadClass]) -> VadTimeline {
        VadTimeline {
            classes: classes.to_vec(),
            window_sec: 0.5,
        }
    }

    fn segment(text: &str, start_ts: f32, end_ts: f32) -> TranscriptSegment {
        TranscriptSegment {
            text: text.to_string(),
            start_ts,
            end_ts,
        }
    }

    #[test]
    fn drops_segments_in_trailing_silence() {
        let outcome = map_whisper_segments_to_silero(
            &[
                segment("To jest początek", 0.0, 0.4),
                segment("Dziękuję za uwagę", 2.0, 2.4),
            ],
            &timeline(&[
                VadClass::Speech,
                VadClass::Speech,
                VadClass::TrailingSilence,
                VadClass::TrailingSilence,
                VadClass::TrailingSilence,
            ]),
            &config_with_tail_drop(true),
        );

        assert_eq!(outcome.dropped_count, 1);
        assert_eq!(outcome.text, "To jest początek");
        assert_eq!(outcome.segments.len(), 1);
    }

    #[test]
    fn keeps_segments_in_speech_regions() {
        let outcome = map_whisper_segments_to_silero(
            &[segment("To jest zwykły segment", 0.0, 0.4)],
            &timeline(&[VadClass::Speech, VadClass::Speech]),
            &config_with_tail_drop(true),
        );

        assert_eq!(outcome.dropped_count, 0);
        assert_eq!(outcome.text, "To jest zwykły segment");
        assert_eq!(outcome.segments.len(), 1);
    }

    #[test]
    fn inserts_ellipsis_for_utterance_gap() {
        let outcome = map_whisper_segments_to_silero(
            &[
                segment("To jest", 0.0, 0.4),
                segment("dalszy ciąg", 1.0, 1.4),
            ],
            &timeline(&[
                VadClass::Speech,
                VadClass::UtteranceGap,
                VadClass::Speech,
                VadClass::Speech,
            ]),
            &config_with_tail_drop(true),
        );

        assert_eq!(outcome.text, "To jest … dalszy ciąg");
        assert_eq!(outcome.segments[1].text, "… dalszy ciąg");
    }

    #[test]
    fn appends_period_for_sentence_boundary_without_terminator() {
        let outcome = map_whisper_segments_to_silero(
            &[
                segment("Pierwsze zdanie", 0.0, 0.4),
                segment("Drugie zdanie", 1.5, 1.9),
            ],
            &timeline(&[
                VadClass::Speech,
                VadClass::SentenceBoundary,
                VadClass::SentenceBoundary,
                VadClass::Speech,
            ]),
            &config_with_tail_drop(true),
        );

        assert_eq!(outcome.text, "Pierwsze zdanie. Drugie zdanie");
        assert_eq!(outcome.segments[0].text, "Pierwsze zdanie.");
    }

    #[test]
    fn tail_drop_disabled_keeps_all_segments() {
        let outcome = map_whisper_segments_to_silero(
            &[
                segment("Główna wypowiedź", 0.0, 0.4),
                segment("Subscribe", 2.0, 2.4),
            ],
            &timeline(&[
                VadClass::Speech,
                VadClass::Speech,
                VadClass::TrailingSilence,
                VadClass::TrailingSilence,
                VadClass::TrailingSilence,
            ]),
            &config_with_tail_drop(false),
        );

        assert_eq!(outcome.dropped_count, 0);
        assert_eq!(outcome.segments.len(), 2);
        assert!(outcome.text.contains("Subscribe"));
    }

    #[test]
    fn dropped_count_matches_dropped_segments() {
        let outcome = map_whisper_segments_to_silero(
            &[
                segment("mowa", 0.0, 0.4),
                segment("Dziękuję", 2.0, 2.4),
                segment("Subscribe", 2.5, 2.9),
            ],
            &timeline(&[
                VadClass::Speech,
                VadClass::Speech,
                VadClass::TrailingSilence,
                VadClass::TrailingSilence,
                VadClass::TrailingSilence,
                VadClass::TrailingSilence,
            ]),
            &config_with_tail_drop(true),
        );

        assert_eq!(outcome.dropped_count, 2);
    }

    #[test]
    fn dropped_text_samples_capped_at_three() {
        let outcome = map_whisper_segments_to_silero(
            &[
                segment("A", 2.0, 2.4),
                segment("B", 2.5, 2.9),
                segment("C", 3.0, 3.4),
                segment("D", 3.5, 3.9),
            ],
            &timeline(&[
                VadClass::TrailingSilence,
                VadClass::TrailingSilence,
                VadClass::TrailingSilence,
                VadClass::TrailingSilence,
                VadClass::TrailingSilence,
                VadClass::TrailingSilence,
                VadClass::TrailingSilence,
                VadClass::TrailingSilence,
            ]),
            &config_with_tail_drop(true),
        );

        assert_eq!(outcome.dropped_count, 4);
        assert_eq!(outcome.dropped_text_samples, vec!["A", "B", "C"]);
    }
}
