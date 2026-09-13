//! Acoustic progress observer. This owns no occurrence, text, seal or recorder.
//! Text equality here detects a frozen preview, never acoustic identity.

use crate::audio::capture_receipt::AcousticSpeechEvidence;
use crate::pipeline::acoustic_ledger::OccurrenceIdentity;
use crate::pipeline::contracts::SpeechIntegrityPhase;

/// Allow normal recognizer latency, then expose debt while the take is live.
pub(super) const SPEECH_STALL_MS: u64 = 1_200;

#[derive(Default)]
pub(super) struct SpeechProgress {
    speech_samples: u64,
    speech_at_advance: u64,
    sample_at_advance: u64,
    observed_samples: u64,
    available: bool,
    speech_live: bool,
    words: Vec<String>,
    word_end: u64,
}

impl SpeechProgress {
    pub fn observe_speech(&mut self, evidence: &AcousticSpeechEvidence, speech_live: bool) {
        self.available = evidence.availability().observed_samples().is_some();
        self.speech_live = self.available && speech_live;
        if let Some(observed) = evidence.availability().observed_samples() {
            self.observed_samples = observed;
            self.speech_samples = evidence
                .ranges()
                .iter()
                .map(|range| range.sample_end.saturating_sub(range.sample_start))
                .sum();
        }
    }

    /// Punctuation-only changes and repeated callbacks do not repay speech.
    /// Equal words at later physical timestamps are real recognizer progress.
    pub fn observe_apple(&mut self, text: &str, word_end: u64) {
        let words: Vec<String> = text
            .split_whitespace()
            .map(|word| {
                word.chars()
                    .filter(|c| c.is_alphanumeric())
                    .flat_map(char::to_lowercase)
                    .collect::<String>()
            })
            .filter(|word| !word.is_empty())
            .collect();
        if !words.is_empty() && (words != self.words || word_end > self.word_end) {
            self.speech_at_advance = self.speech_samples;
            self.sample_at_advance = self.observed_samples;
        }
        self.words = words;
        self.word_end = self.word_end.max(word_end);
    }

    pub fn debt_ms(&self, sample_rate: u32) -> u64 {
        self.speech_samples
            .saturating_sub(self.speech_at_advance)
            .saturating_mul(1_000)
            / u64::from(sample_rate.max(1))
    }

    pub fn recovered_through(&mut self, sample_end: u64, evidence: &AcousticSpeechEvidence) {
        if sample_end > self.sample_at_advance {
            self.sample_at_advance = sample_end;
            self.speech_at_advance = evidence
                .ranges()
                .iter()
                .map(|range| {
                    range
                        .sample_end
                        .min(sample_end)
                        .saturating_sub(range.sample_start)
                })
                .sum();
        }
    }

    pub fn phase(&self, sample_rate: u32) -> SpeechIntegrityPhase {
        if !self.available {
            SpeechIntegrityPhase::Unavailable
        } else if self.debt_ms(sample_rate) >= SPEECH_STALL_MS {
            SpeechIntegrityPhase::Stalled
        } else if self.speech_live && !self.words.is_empty() {
            SpeechIntegrityPhase::Tracking
        } else {
            SpeechIntegrityPhase::Listening
        }
    }

    /// Only speech physically inside this closed occurrence can owe its repair.
    pub fn occurrence_has_debt(
        &self,
        occurrence: &OccurrenceIdentity,
        evidence: &AcousticSpeechEvidence,
        sample_rate: u32,
    ) -> bool {
        if !evidence
            .identity()
            .matches(&occurrence.session, occurrence.capture_epoch)
            || evidence.availability().observed_samples().is_none()
        {
            return false;
        }
        let after = occurrence.sample_start.max(self.sample_at_advance);
        let samples: u64 = evidence
            .ranges()
            .iter()
            .map(|range| {
                range
                    .sample_end
                    .min(occurrence.sample_end)
                    .saturating_sub(range.sample_start.max(after))
            })
            .sum();
        samples.saturating_mul(1_000) / u64::from(sample_rate.max(1)) >= SPEECH_STALL_MS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::capture_receipt::{AcousticAvailability, CaptureEvidenceIdentity};
    use crate::stt::tail_provider::TailSampleRange;

    fn speech(end: u64, extent: u64) -> AcousticSpeechEvidence {
        AcousticSpeechEvidence::measured(
            CaptureEvidenceIdentity::new("take", 0),
            "silero_boundaries",
            AcousticAvailability::Observed {
                observed_samples: extent,
            },
            vec![TailSampleRange {
                session: "take".into(),
                capture_epoch: 0,
                sample_start: 0,
                sample_end: end,
            }],
        )
    }

    #[test]
    fn speech_stalls_but_silence_never_accumulates_debt() {
        let mut progress = SpeechProgress::default();
        progress.observe_speech(&speech(200, 200), true);
        progress.observe_apple("Pierwsze słowo", 200);
        progress.observe_speech(&speech(2_600, 2_600), true);
        assert_eq!(progress.debt_ms(1_000), 2_400);
        assert_eq!(progress.phase(1_000), SpeechIntegrityPhase::Stalled);
        progress.observe_speech(&speech(2_600, 12_600), false);
        assert_eq!(progress.debt_ms(1_000), 2_400);
        assert!(progress.occurrence_has_debt(
            &OccurrenceIdentity::new("take", 0, 0, 2_600),
            &speech(2_600, 12_600),
            1_000
        ));
        assert!(!progress.occurrence_has_debt(
            &OccurrenceIdentity::new("other", 0, 0, 2_600),
            &speech(2_600, 12_600),
            1_000
        ));
    }

    #[test]
    fn repeated_callback_and_punctuation_cannot_hide_stall_but_new_word_can() {
        let mut progress = SpeechProgress::default();
        progress.observe_speech(&speech(100, 100), true);
        progress.observe_apple("Iwo", 100);
        progress.observe_speech(&speech(2_500, 2_500), true);
        progress.observe_apple("Iwo!", 100);
        assert_eq!(progress.debt_ms(1_000), 2_400);
        progress.observe_apple("Iwo Iwo", 2_500);
        assert_eq!(progress.debt_ms(1_000), 0);
        assert_eq!(progress.phase(1_000), SpeechIntegrityPhase::Tracking);
    }
}
