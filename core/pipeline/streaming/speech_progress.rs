//! Acoustic progress observer. This owns no occurrence, text, seal or recorder.
//! Text equality here detects a frozen preview, never acoustic identity.

use std::ops::Range;

use crate::audio::capture_receipt::{AcousticSpeechEvidence, CaptureEvidenceIdentity};
use crate::pipeline::acoustic_ledger::OccurrenceIdentity;
use crate::pipeline::contracts::SpeechIntegrityPhase;

/// Allow normal recognizer latency, then expose debt while the take is live.
pub(super) const SPEECH_STALL_MS: u64 = 1_200;

pub(super) struct SpeechProgress {
    identity: CaptureEvidenceIdentity,
    sample_rate: u32,
    sample_at_advance: u64,
    observed_samples: u64,
    available: bool,
    speech_live: bool,
    words: Vec<String>,
    word_end: u64,
    live_speech: Vec<Range<u64>>,
    suspect_speech: Vec<Range<u64>>,
    recovered_speech: Vec<Range<u64>>,
}

impl SpeechProgress {
    pub fn new(session: impl Into<String>, capture_epoch: u64, sample_rate: u32) -> Self {
        Self {
            identity: CaptureEvidenceIdentity::new(session, capture_epoch),
            sample_rate,
            sample_at_advance: 0,
            observed_samples: 0,
            available: false,
            speech_live: false,
            words: Vec::new(),
            word_end: 0,
            live_speech: Vec::new(),
            suspect_speech: Vec::new(),
            recovered_speech: Vec::new(),
        }
    }

    fn validated_observed_samples(&self, evidence: &AcousticSpeechEvidence) -> Option<u64> {
        if self.sample_rate == 0 || &self.identity != evidence.identity() {
            return None;
        }
        let observed = evidence.availability().observed_samples()?;
        evidence
            .ranges()
            .iter()
            .all(|range| {
                self.identity.matches(&range.session, range.capture_epoch)
                    && range.sample_start < range.sample_end
                    && range.sample_end <= observed
            })
            .then_some(observed)
    }

    pub fn observe_speech(
        &mut self,
        evidence: &AcousticSpeechEvidence,
        speech_live: bool,
        sample_rate: u32,
    ) {
        if &self.identity != evidence.identity() {
            return;
        }
        let Some(observed) = self.validated_observed_samples(evidence) else {
            self.available = false;
            self.speech_live = false;
            return;
        };
        if sample_rate != self.sample_rate {
            self.available = false;
            self.speech_live = false;
            return;
        }
        // An older cumulative callback must not rewind live or retained debt.
        if observed < self.observed_samples {
            return;
        }
        self.available = true;
        self.speech_live = speech_live;
        self.observed_samples = observed;
        let ranges = normalize_progress_ranges(
            evidence
                .ranges()
                .iter()
                .filter_map(|range| {
                    let start = range.sample_start.max(self.sample_at_advance);
                    (start < range.sample_end).then_some(start..range.sample_end)
                })
                .collect(),
        );
        self.live_speech = subtract_progress_ranges(&ranges, &self.recovered_speech);
        if self.debt_ms(sample_rate) >= SPEECH_STALL_MS {
            self.suspect_speech.extend(self.live_speech.iter().cloned());
            self.suspect_speech =
                normalize_progress_ranges(std::mem::take(&mut self.suspect_speech));
        }
    }

    /// Text progress can resume the live indicator, not certify earlier audio.
    /// Punctuation-only changes and repeated callbacks do not advance it.
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
            self.sample_at_advance = self.observed_samples;
            self.live_speech.clear();
        }
        self.words = words;
        self.word_end = self.word_end.max(word_end);
    }

    pub fn debt_ms(&self, sample_rate: u32) -> u64 {
        if sample_rate == 0 || sample_rate != self.sample_rate {
            return 0;
        }
        self.live_speech
            .iter()
            .map(|range| range.end - range.start)
            .sum::<u64>()
            .saturating_mul(1_000)
            / u64::from(sample_rate)
    }

    /// Called only after the ledger admits recovery of this exact occurrence.
    /// Keep acknowledged ranges so cumulative evidence cannot resurrect them.
    pub fn recovered_occurrence(
        &mut self,
        occurrence: &OccurrenceIdentity,
        evidence: &AcousticSpeechEvidence,
    ) {
        if !self
            .identity
            .matches(&occurrence.session, occurrence.capture_epoch)
            || occurrence.sample_start >= occurrence.sample_end
            || self
                .validated_observed_samples(evidence)
                .is_none_or(|end| occurrence.sample_end > end)
        {
            return;
        }
        let recovered = occurrence.sample_start..occurrence.sample_end;
        self.live_speech =
            subtract_progress_ranges(&self.live_speech, std::slice::from_ref(&recovered));
        self.suspect_speech =
            subtract_progress_ranges(&self.suspect_speech, std::slice::from_ref(&recovered));
        self.recovered_speech.push(recovered);
        self.recovered_speech =
            normalize_progress_ranges(std::mem::take(&mut self.recovered_speech));
    }

    pub fn phase(&self, sample_rate: u32) -> SpeechIntegrityPhase {
        if !self.available || sample_rate == 0 || sample_rate != self.sample_rate {
            SpeechIntegrityPhase::Unavailable
        } else if self.debt_ms(sample_rate) >= SPEECH_STALL_MS {
            SpeechIntegrityPhase::Stalled
        } else if self.speech_live && !self.words.is_empty() {
            SpeechIntegrityPhase::Tracking
        } else {
            SpeechIntegrityPhase::Listening
        }
    }

    /// A threshold-qualified suspect interval remains owed across text progress.
    /// Any physical intersection owes verification, even across a short boundary.
    pub fn occurrence_has_debt(
        &self,
        occurrence: &OccurrenceIdentity,
        evidence: &AcousticSpeechEvidence,
        sample_rate: u32,
    ) -> bool {
        if sample_rate == 0
            || sample_rate != self.sample_rate
            || &self.identity != evidence.identity()
            || !self
                .identity
                .matches(&occurrence.session, occurrence.capture_epoch)
            || occurrence.sample_start >= occurrence.sample_end
        {
            return false;
        }
        self.suspect_speech
            .iter()
            .any(|range| range.start < occurrence.sample_end && occurrence.sample_start < range.end)
    }
}

fn normalize_progress_ranges(mut ranges: Vec<Range<u64>>) -> Vec<Range<u64>> {
    ranges.sort_unstable_by_key(|range| range.start);
    let mut merged: Vec<Range<u64>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        if let Some(last) = merged.last_mut()
            && range.start <= last.end
        {
            last.end = last.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    merged
}

/// Both inputs are sorted disjoint unions; advance monotonically through them.
fn subtract_progress_ranges(ranges: &[Range<u64>], removed: &[Range<u64>]) -> Vec<Range<u64>> {
    let mut result = Vec::new();
    let mut index = 0;
    for range in ranges {
        let mut start = range.start;
        while index < removed.len() && removed[index].end <= start {
            index += 1;
        }
        while index < removed.len() && removed[index].start < range.end {
            if removed[index].start > start {
                result.push(start..removed[index].start.min(range.end));
            }
            start = start.max(removed[index].end);
            if start >= range.end {
                break;
            }
            index += 1;
        }
        if start < range.end {
            result.push(start..range.end);
        }
    }
    result
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
        let mut progress = SpeechProgress::new("take", 0, 1_000);
        progress.observe_speech(&speech(200, 200), true, 1_000);
        progress.observe_apple("Pierwsze słowo", 200);
        progress.observe_speech(&speech(2_600, 2_600), true, 1_000);
        assert_eq!(progress.debt_ms(1_000), 2_400);
        assert_eq!(progress.phase(1_000), SpeechIntegrityPhase::Stalled);
        progress.observe_speech(&speech(2_600, 12_600), false, 1_000);
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
    fn apple_resumption_does_not_clear_acoustic_recovery_obligation() {
        let mut progress = SpeechProgress::new("take", 0, 1_000);
        progress.observe_speech(&speech(200, 200), true, 1_000);
        progress.observe_apple("Pierwsze słowa", 200);
        progress.observe_speech(&speech(2_600, 2_600), true, 1_000);
        assert_eq!(progress.phase(1_000), SpeechIntegrityPhase::Stalled);
        progress.observe_apple("Pierwsze słowa i koniec", 2_600);
        assert_eq!(progress.phase(1_000), SpeechIntegrityPhase::Tracking);
        assert_eq!(progress.debt_ms(1_000), 0);
        assert!(progress.occurrence_has_debt(
            &OccurrenceIdentity::new("take", 0, 0, 2_600),
            &speech(2_600, 2_600),
            1_000,
        ));
    }

    #[test]
    fn exact_recovery_keeps_earlier_suspect_occurrence() {
        let evidence = speech(2_600, 2_600);
        let a = OccurrenceIdentity::new("take", 0, 0, 1_300);
        let b = OccurrenceIdentity::new("take", 0, 1_300, 2_600);
        let mut progress = SpeechProgress::new("take", 0, 1_000);
        progress.observe_speech(&evidence, true, 1_000);
        progress.recovered_occurrence(&b, &evidence);
        assert_eq!(progress.debt_ms(1_000), 1_300);
        assert!(progress.occurrence_has_debt(&a, &evidence, 1_000));
        assert!(!progress.occurrence_has_debt(&b, &evidence, 1_000));
        progress.recovered_occurrence(&b, &evidence);
        progress.observe_speech(&evidence, true, 1_000);
        assert_eq!(progress.debt_ms(1_000), 1_300);
        assert!(!progress.occurrence_has_debt(&b, &evidence, 1_000));
        progress.observe_apple("Iwo Iwo", 2_600);
        assert_eq!(progress.debt_ms(1_000), 0);
        assert!(progress.occurrence_has_debt(&a, &evidence, 1_000));
        progress.recovered_occurrence(&a, &evidence);
        progress.observe_speech(&evidence, true, 1_000);
        assert!(!progress.occurrence_has_debt(&a, &evidence, 1_000));
        assert_eq!(progress.recovered_speech, vec![0..2_600]);
    }

    #[test]
    fn threshold_crossing_spans_short_occurrences() {
        let mut ranges = speech(700, 1_600).ranges().to_vec();
        ranges.push(TailSampleRange {
            session: "take".into(),
            capture_epoch: 0,
            sample_start: 900,
            sample_end: 1_600,
        });
        let evidence = AcousticSpeechEvidence::measured(
            CaptureEvidenceIdentity::new("take", 0),
            "silero_boundaries",
            AcousticAvailability::Observed {
                observed_samples: 1_600,
            },
            ranges,
        );
        let mut progress = SpeechProgress::new("take", 0, 1_000);
        progress.observe_speech(&evidence, true, 1_000);
        assert_eq!(progress.debt_ms(1_000), 1_400);
        progress.observe_apple("Iwo Iwo", 1_600);
        for (start, end, owed) in [(0, 700, true), (700, 900, false), (900, 1_600, true)] {
            assert_eq!(
                progress.occurrence_has_debt(
                    &OccurrenceIdentity::new("take", 0, start, end),
                    &evidence,
                    1_000,
                ),
                owed
            );
        }
    }

    #[test]
    fn duplicate_speech_does_not_inflate_debt() {
        let range = speech(700, 7_000).ranges()[0].clone();
        let evidence = AcousticSpeechEvidence::measured(
            CaptureEvidenceIdentity::new("take", 0),
            "silero_boundaries",
            AcousticAvailability::Observed {
                observed_samples: 7_000,
            },
            vec![range.clone(), range],
        );
        let mut progress = SpeechProgress::new("take", 0, 1_000);
        for _ in 0..8 {
            progress.observe_speech(&evidence, false, 1_000);
            assert_eq!(progress.debt_ms(1_000), 700);
            assert!(progress.suspect_speech.is_empty());
        }
        progress.observe_apple("Iwo", 700);
        progress.observe_speech(&evidence, false, 1_000);
        assert_eq!(progress.debt_ms(1_000), 0);
        assert!(progress.suspect_speech.is_empty());
    }

    #[test]
    fn invalid_or_foreign_evidence_cannot_erase_suspicion() {
        let evidence = speech(2_600, 2_600);
        let own = OccurrenceIdentity::new("take", 0, 0, 2_600);
        let mut progress = SpeechProgress::new("take", 0, 1_000);
        progress.observe_speech(&evidence, true, 1_000);
        for (session, epoch) in [("other", 0), ("take", 1)] {
            let foreign = AcousticSpeechEvidence::measured(
                CaptureEvidenceIdentity::new(session, epoch),
                "silero_boundaries",
                AcousticAvailability::Observed {
                    observed_samples: 10_000,
                },
                Vec::new(),
            );
            progress.observe_speech(&foreign, false, 1_000);
            progress.recovered_occurrence(&own, &foreign);
            progress.recovered_occurrence(
                &OccurrenceIdentity::new(session, epoch, 0, 2_600),
                &evidence,
            );
            assert_eq!(progress.debt_ms(1_000), 2_600);
            assert!(progress.occurrence_has_debt(&own, &evidence, 1_000));
        }
        progress.observe_speech(&speech(200, 200), false, 1_000);
        progress.recovered_occurrence(&own, &speech(200, 200));
        assert_eq!(progress.debt_ms(1_000), 2_600);
        for (session, epoch, start, end) in [
            ("take", 0, 300, 200),
            ("take", 0, 200, 200),
            ("take", 0, 0, 2_601),
            ("other", 0, 0, 2_600),
            ("take", 1, 0, 2_600),
        ] {
            let invalid = AcousticSpeechEvidence::measured(
                CaptureEvidenceIdentity::new("take", 0),
                "silero_boundaries",
                AcousticAvailability::Observed {
                    observed_samples: 2_600,
                },
                vec![TailSampleRange {
                    session: session.into(),
                    capture_epoch: epoch,
                    sample_start: start,
                    sample_end: end,
                }],
            );
            progress.observe_speech(&invalid, true, 1_000);
            progress.recovered_occurrence(&own, &invalid);
            assert_eq!(progress.phase(1_000), SpeechIntegrityPhase::Unavailable);
            assert!(progress.occurrence_has_debt(&own, &evidence, 1_000));
        }
        progress.observe_speech(&evidence, true, 0);
        assert_eq!(progress.phase(1_000), SpeechIntegrityPhase::Unavailable);
        progress.observe_speech(&evidence, true, 1_000);
        assert_eq!(progress.phase(1_000), SpeechIntegrityPhase::Stalled);
        let mut unbound = SpeechProgress::new("take", 0, 0);
        unbound.observe_speech(&evidence, true, 0);
        assert!(!unbound.available);
        assert!(unbound.suspect_speech.is_empty());
        let mut bound_before_input = SpeechProgress::new("other", 1, 1_000);
        bound_before_input.observe_speech(&evidence, true, 1_000);
        assert!(!bound_before_input.available);
        assert!(bound_before_input.suspect_speech.is_empty());
        assert!(bound_before_input.identity.matches("other", 1));
    }

    #[test]
    fn interval_subtraction_preserves_uncovered_samples() {
        let ranges = [0..10, 20..30, 40..50];
        for removed in [5..25, 0..50, 10..20]
            .into_iter()
            .map(|range| std::iter::once(range).collect::<Vec<_>>())
            .chain(std::iter::once(vec![3..8, 9..42]))
        {
            let actual = subtract_progress_ranges(&ranges, &removed);
            let actual_samples = actual.into_iter().flatten().collect::<Vec<_>>();
            let expected = ranges
                .iter()
                .cloned()
                .flatten()
                .filter(|sample| !removed.iter().any(|range| range.contains(sample)))
                .collect::<Vec<_>>();
            assert_eq!(actual_samples, expected);
        }
    }

    #[test]
    fn repeated_callback_and_punctuation_cannot_hide_stall_but_new_word_can() {
        let mut progress = SpeechProgress::new("take", 0, 1_000);
        progress.observe_speech(&speech(100, 100), true, 1_000);
        progress.observe_apple("Iwo", 100);
        progress.observe_speech(&speech(2_500, 2_500), true, 1_000);
        progress.observe_apple("Iwo!", 100);
        assert_eq!(progress.debt_ms(1_000), 2_400);
        progress.observe_apple("Iwo Iwo", 2_500);
        assert_eq!(progress.debt_ms(1_000), 0);
        assert_eq!(progress.phase(1_000), SpeechIntegrityPhase::Tracking);
    }
}
