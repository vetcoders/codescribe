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
    // Identity for each freezed slot, parallel to `freezed`. `LiveAssembly`
    // exposes plain strings (its consumers only ever read text), but layered
    // patches address utterances by id, so the replay has to remember which
    // slot belongs to whom.
    let mut freezed_ids: Vec<u64> = Vec::new();
    let mut preview = String::new();

    for event in events {
        match event {
            EngineEvent::Preview { text, .. } | EngineEvent::Correction { text, .. } => {
                // Open tail only — replace, never freeze.
                preview = text.trim().to_string();
            }
            EngineEvent::UtteranceFinal {
                utterance_id, text, ..
            } => {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    freezed.push(trimmed.to_string());
                    freezed_ids.push(*utterance_id);
                }
                preview.clear();
            }
            // Layer 1+ bounded patch (ADR "never rewrite from zero"). The live
            // assembly is the product's overlay/floor model AND what the parity
            // harness measures, so a layer that patches the committed buffer
            // has to be visible here too — otherwise a layered-on run reads
            // exactly like a layered-off one and the bar gates nothing.
            //
            // `rposition` mirrors the overlay's `lastIndex(where:)`: if an id
            // was sealed twice, both sides patch the same slot.
            EngineEvent::ReplaceRange { utterance_id, .. } => {
                if let Some(index) = freezed_ids.iter().rposition(|id| id == utterance_id) {
                    // Out-of-range windows are dropped, not clamped: a patch
                    // that does not fit the text it claims to target is a
                    // desync, and half-applying it would corrupt the floor.
                    let _ = event.apply_to_committed_text(&mut freezed[index]);
                }
            }
            // `InsertAnnotation` stays out on purpose: annotations are visible
            // decoration ("[…]"), not transcript content, and folding them in
            // would inflate the word-count bars that gate this assembly.
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
    use crate::pipeline::contracts::{EngineEvent, LayerSource};

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

    /// Layer 1 (`ReplaceRange { source: TailPatch }`) must reach the live
    /// assembly, because the assembly IS what the parity harness measures
    /// (`capture_clip_via_device` → `assemble_live_from_events`). While this
    /// event was skipped here, a layered-on run scored identically to a
    /// layered-off one: the bar was blind to the whole layer it was meant to
    /// gate — the P1-01 failure shape (a green gate asserting nothing).
    #[test]
    fn replace_range_patches_the_sealed_utterance_it_targets() {
        let events = vec![
            final_ev(1, "pierwsze zdanie"),
            final_ev(2, "drugie zdanie"),
            EngineEvent::ReplaceRange {
                utterance_id: 2,
                start: 0,
                end: 6,
                text: "trzecie".into(),
                source: LayerSource::TailPatch,
            },
        ];
        let assembly = assemble_live_from_events(&events);
        assert_eq!(
            assembly.freezed,
            vec!["pierwsze zdanie".to_string(), "trzecie zdanie".to_string()],
            "TailPatch must land on its own utterance and leave the others alone"
        );
        assert_eq!(assembly.full_text(), "pierwsze zdanie trzecie zdanie");
        assert_eq!(assembly.streaming_floor(), "pierwsze zdanie trzecie zdanie");
    }

    /// Offsets are char offsets (`EngineEvent::apply_to_committed_text`), not
    /// bytes — Polish diacritics are 2-byte in UTF-8, so a byte-indexed
    /// implementation would slice mid-codepoint here and panic.
    #[test]
    fn replace_range_offsets_are_chars_not_bytes() {
        let events = vec![
            final_ev(1, "zażółć gęślą"),
            EngineEvent::ReplaceRange {
                utterance_id: 1,
                start: 7,
                end: 12,
                text: "jaźń".into(),
                source: LayerSource::TailPatch,
            },
        ];
        let assembly = assemble_live_from_events(&events);
        assert_eq!(assembly.freezed, vec!["zażółć jaźń".to_string()]);
    }

    /// An unbound or out-of-range patch is dropped, never applied to a
    /// neighbour and never allowed to corrupt the floor.
    #[test]
    fn replace_range_out_of_bounds_or_unbound_is_dropped_not_misapplied() {
        let events = vec![
            final_ev(1, "krótkie"),
            // No utterance 9 was ever sealed.
            EngineEvent::ReplaceRange {
                utterance_id: 9,
                start: 0,
                end: 3,
                text: "X".into(),
                source: LayerSource::TailPatch,
            },
            // Utterance 1 exists, but the window overruns its text.
            EngineEvent::ReplaceRange {
                utterance_id: 1,
                start: 0,
                end: 999,
                text: "X".into(),
                source: LayerSource::TailPatch,
            },
        ];
        let assembly = assemble_live_from_events(&events);
        assert_eq!(assembly.freezed, vec!["krótkie".to_string()]);
    }

    /// A patch that arrives before its utterance sealed has nothing to bind to.
    /// It must not retro-apply to the next final that happens to arrive.
    #[test]
    fn replace_range_before_its_final_does_not_retro_apply() {
        let events = vec![
            EngineEvent::ReplaceRange {
                utterance_id: 1,
                start: 0,
                end: 4,
                text: "PATCH".into(),
                source: LayerSource::TailPatch,
            },
            final_ev(1, "tekst po fakcie"),
        ];
        let assembly = assemble_live_from_events(&events);
        assert_eq!(assembly.freezed, vec!["tekst po fakcie".to_string()]);
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
