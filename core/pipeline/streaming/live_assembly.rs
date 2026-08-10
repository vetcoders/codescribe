//! Product live assembly: freezed sealed utterances + open interim tail.
//!
//! This is the Codescribe engine contract for overlay and delivery floor:
//! - each non-empty `UtteranceFinal` freezes a segment (append)
//! - `Preview` / `Correction` only replace the open tail
//! - full live text = freezed segments joined + optional open preview
//!
//! Without multi-final freeze+append there is no dictation engine — only a
//! single tail fragment.
//!
//! **Gaps in Apple live text are not engine failure.** Under-generation at
//! uncertainty is the product canvas: Whisper over-gen / human / lexicon fill
//! those loci (Teacher: Needs attention → lexicon). Engine success = multi-seal
//! freezed+append spanning the spoken arc — not perfect WER vs human.

use crate::pipeline::contracts::EngineEvent;

/// Result of replaying engine events into the product live model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveAssembly {
    /// Sealed utterance segments in order (freezed list).
    pub freezed: Vec<String>,
    /// Open interim tail (may be empty after a seal).
    pub preview: String,
}

impl LiveAssembly {
    /// Full live text shown to the user (freezed + open preview).
    pub fn full_text(&self) -> String {
        let mut parts = self.freezed.clone();
        let preview = self.preview.trim();
        if !preview.is_empty() {
            parts.push(preview.to_string());
        }
        parts.join(" ")
    }

    /// Number of sealed non-empty utterance finals.
    pub fn sealed_count(&self) -> usize {
        self.freezed.len()
    }

    /// Streaming floor at stop: freezed only (no open interim).
    pub fn streaming_floor(&self) -> String {
        self.freezed.join(" ")
    }
}

/// Build product live assembly from a sequence of engine events.
///
/// Pure, STT-agnostic — unit-testable without Metal / Apple / Whisper.
pub fn assemble_live_from_events(events: &[EngineEvent]) -> LiveAssembly {
    let mut freezed: Vec<String> = Vec::new();
    let mut preview = String::new();

    for event in events {
        match event {
            EngineEvent::Preview { text, .. } | EngineEvent::Correction { text, .. } => {
                // Open tail only — replace, never freeze.
                preview = text.trim().to_string();
            }
            EngineEvent::UtteranceFinal { text, .. } => {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    freezed.push(trimmed.to_string());
                }
                preview.clear();
            }
            EngineEvent::NoSpeech { .. } => {
                preview.clear();
            }
            _ => {}
        }
    }

    LiveAssembly { freezed, preview }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::contracts::EngineEvent;

    fn final_ev(id: u64, text: &str) -> EngineEvent {
        EngineEvent::UtteranceFinal {
            utterance_id: id,
            text: text.into(),
            raw_text: text.into(),
            start_ts: 0.0,
            end_ts: 1.0,
            segments: vec![],
            vad_speech_pct: None,
            avg_logprob: None,
            compression_ratio: None,
            quality_gate_dropped: false,
            confidence_flags: vec![],
        }
    }

    #[test]
    fn multi_final_freezed_append_builds_full_transcript() {
        let events = vec![
            EngineEvent::Preview {
                rev: 1,
                text: "pierwsze".into(),
            },
            final_ev(1, "pierwsze zdanie"),
            EngineEvent::Preview {
                rev: 2,
                text: "drugie".into(),
            },
            EngineEvent::Preview {
                rev: 3,
                text: "drugie zdanie live".into(),
            },
            final_ev(2, "drugie zdanie"),
            EngineEvent::Preview {
                rev: 4,
                text: "trzecie w toku".into(),
            },
        ];
        let assembly = assemble_live_from_events(&events);
        assert_eq!(assembly.sealed_count(), 2, "must freeze each sealed final");
        assert_eq!(
            assembly.freezed,
            vec!["pierwsze zdanie".to_string(), "drugie zdanie".to_string()]
        );
        assert_eq!(assembly.preview, "trzecie w toku");
        assert_eq!(
            assembly.full_text(),
            "pierwsze zdanie drugie zdanie trzecie w toku"
        );
        assert_eq!(assembly.streaming_floor(), "pierwsze zdanie drugie zdanie");
    }

    #[test]
    fn single_final_tail_shape_is_detectable_as_engine_failure_bar() {
        // Known broken product shape: one short sealed final for a long clip.
        let events = vec![final_ev(1, "o Esterna przepisze krople")];
        let assembly = assemble_live_from_events(&events);
        assert_eq!(assembly.sealed_count(), 1);
        assert!(
            assembly.full_text().chars().count() < 40,
            "tail-only fixture for gating bar"
        );
    }

    #[test]
    fn preview_replaces_open_tail_without_freezing() {
        let events = vec![
            EngineEvent::Preview {
                rev: 1,
                text: "a".into(),
            },
            EngineEvent::Preview {
                rev: 2,
                text: "a b c".into(),
            },
        ];
        let assembly = assemble_live_from_events(&events);
        assert_eq!(assembly.sealed_count(), 0);
        assert_eq!(assembly.full_text(), "a b c");
    }
}
