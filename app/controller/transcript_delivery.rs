//! Delivery-only transcript envelope and session quality receipts.
//!
//! The reducer document stays clean. This module observes engine metadata and
//! renders the configured tag only when the controller has selected a sink.

use crate::config::Config;
use codescribe_core::pipeline::contracts::{EngineEvent, EventSink, TranscriptionConfidenceFlag};
use std::sync::Mutex;

#[derive(Debug, Clone, Default)]
struct DeliveryMetadata {
    mode: Option<&'static str>,
    language: Option<&'static str>,
    avg_logprob: Option<f32>,
    utterance_count: usize,
    confidence_flags: Vec<TranscriptionConfidenceFlag>,
}

/// Passive session observer used only when producing a delivery payload.
#[derive(Debug, Default)]
pub(super) struct TranscriptDeliveryTagger {
    metadata: Mutex<DeliveryMetadata>,
}

impl TranscriptDeliveryTagger {
    pub(super) fn begin(&self, mode: &'static str, language: &'static str) {
        *self
            .metadata
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = DeliveryMetadata {
            mode: Some(mode),
            language: Some(language),
            ..DeliveryMetadata::default()
        };
    }

    pub(super) fn render(
        &self,
        text: &str,
        config: &Config,
        mode_override: Option<&'static str>,
    ) -> String {
        let trimmed = text.trim();
        if trimmed.is_empty() || !config.transcript_tagging_enabled {
            return trimmed.to_string();
        }
        let metadata = self
            .metadata
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        codescribe_core::transcript_tagging::wrap_transcript_with_quality(
            trimmed,
            &config.transcript_tag_template,
            mode_override.or(metadata.mode).unwrap_or("unknown"),
            metadata
                .language
                .unwrap_or_else(|| config.whisper_language.as_str()),
            metadata.avg_logprob,
            &metadata.confidence_flags,
        )
    }
}

impl EventSink for TranscriptDeliveryTagger {
    fn on_event(&self, event: &EngineEvent) {
        let EngineEvent::UtteranceFinal {
            avg_logprob,
            confidence_flags,
            ..
        } = event
        else {
            return;
        };
        let mut metadata = self
            .metadata
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        metadata.utterance_count += 1;
        metadata.avg_logprob = if metadata.utterance_count == 1 {
            *avg_logprob
        } else {
            // The engine exposes utterance averages, not token counts. Any
            // session aggregate would therefore be invented.
            None
        };
        for flag in confidence_flags {
            if !metadata.confidence_flags.contains(flag) {
                metadata.confidence_flags.push(*flag);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codescribe_core::pipeline::contracts::TranscriptSegment;

    fn final_event(
        avg_logprob: Option<f32>,
        flags: Vec<TranscriptionConfidenceFlag>,
    ) -> EngineEvent {
        EngineEvent::UtteranceFinal {
            utterance_id: 1,
            text: "body".into(),
            raw_text: "body".into(),
            start_ts: 0.0,
            end_ts: 1.0,
            segments: Vec::<TranscriptSegment>::new(),
            vad_speech_pct: Some(80.0),
            avg_logprob,
            compression_ratio: None,
            confidence_flags: flags,
        }
    }

    #[test]
    fn disabled_tagging_preserves_delivery_bytes() {
        let tagger = TranscriptDeliveryTagger::default();
        let config = Config {
            transcript_tagging_enabled: false,
            ..Config::default()
        };
        assert_eq!(
            tagger.render("  body  ", &config, Some("dictation")),
            "body"
        );
    }

    #[test]
    fn configured_template_uses_real_session_and_quality_metadata() {
        let tagger = TranscriptDeliveryTagger::default();
        tagger.begin("dictation", "pl");
        tagger.on_event(&final_event(
            Some(-1.4),
            vec![TranscriptionConfidenceFlag::PossibleHallucinationLogprob],
        ));
        let config = Config {
            transcript_tagging_enabled: true,
            transcript_tag_template: "[{mode}|{lang}|{conf}|{flags}] {text}".into(),
            ..Config::default()
        };
        assert_eq!(
            tagger.render("body", &config, None),
            "[dictation|pl|low|possible_hallucination_logprob] body"
        );
    }

    #[test]
    fn missing_quality_metadata_stays_unknown() {
        let tagger = TranscriptDeliveryTagger::default();
        tagger.begin("agent", "en");
        let config = Config {
            transcript_tagging_enabled: true,
            transcript_tag_template: "[{mode}|{lang}|{conf}|{flags}] {text}".into(),
            ..Config::default()
        };
        assert_eq!(
            tagger.render("body", &config, None),
            "[agent|en|unknown|] body"
        );
    }

    #[test]
    fn empty_delivery_never_produces_a_tag_shell() {
        let tagger = TranscriptDeliveryTagger::default();
        let config = Config {
            transcript_tagging_enabled: true,
            ..Config::default()
        };
        assert_eq!(tagger.render(" \n ", &config, Some("dictation")), "");
    }

    #[test]
    fn multiple_utterances_do_not_invent_a_session_confidence_average() {
        let tagger = TranscriptDeliveryTagger::default();
        tagger.begin("dictation", "pl");
        tagger.on_event(&final_event(Some(-0.2), Vec::new()));
        tagger.on_event(&final_event(
            Some(-1.4),
            vec![TranscriptionConfidenceFlag::PossibleHallucinationLogprob],
        ));
        let config = Config {
            transcript_tagging_enabled: true,
            transcript_tag_template: "[{conf}|{flags}] {text}".into(),
            ..Config::default()
        };
        assert_eq!(
            tagger.render("body", &config, None),
            "[unknown|possible_hallucination_logprob] body"
        );
    }
}
