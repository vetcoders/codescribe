//! Word-onset evidence from a token decoder that flags word starts
//! (Parakeet TDT greedy path, `parakeet-representation-report.md`, 13 X 2026).
//!
//! What this evidence is: the number of word onsets the decoder emitted over a
//! PCM span, with each emission centred on an encoder frame (80 ms hop) and a
//! stated centre uncertainty. What it is not: a transcript (no text travels
//! here), a CTC posterior (the checkpoint has no CTC head), a probability that
//! a word exists (`confidence` is saturated), a word boundary (frames are
//! coarser than short words) or proof of silence (an unvisited frame says
//! nothing). It answers one question the ledger could not answer from energy
//! alone: how many words were spoken under the union of two producers' claims.
//!
//! The ledger stores every window as recorded. A span covered by two windows
//! that disagree is explicitly unresolved, never averaged.
use serde::{Deserialize, Serialize};

use super::*;

/// Producer name on receipts for this evidence family.
pub const WORD_ONSET_PRODUCER: &str = "parakeet-tdt-word-starts";

/// One token emission on the greedy TDT path. No text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WordOnsetEmission {
    /// Encoder frame index inside the window.
    pub frame: u32,
    /// Frame centre on the capture PCM axis (not the window axis).
    pub centre_sample: u64,
    /// Duration head output, in frames (0..=4 for TDT v3).
    #[serde(default)]
    pub duration_frames: u8,
    /// Tokenizer id; deliberately not resolved to text here.
    pub token_id: u32,
    /// The token starts a word (SentencePiece `▁`).
    pub word_start: bool,
    /// Saturated near 1.0 for most tokens; not calibrated.
    #[serde(default)]
    pub confidence: f32,
}

/// One decoded window of word-onset evidence over a capture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WordOnsetEvidence {
    /// Capture identity. A fixture exported per take carries none; the
    /// loader binds it before the window can answer for a capture.
    #[serde(default)]
    pub session: String,
    #[serde(default)]
    pub capture_epoch: u64,
    /// e.g. "mlx-community/parakeet-tdt-0.6b-v3@ed2b7e8c".
    pub model_id: String,
    #[serde(default)]
    pub weights_sha256: Option<String>,
    /// Axis of `centre_sample`, `pcm_span` and `frame_hop_samples`.
    pub input_sample_rate: u32,
    /// 80 ms at the input rate (3 840 at 48 kHz).
    pub frame_hop_samples: u64,
    /// Centre of frame 0 on the capture axis (= window start, no padding).
    pub frame0_centre_sample: u64,
    /// Half a hop plus unmeasured emission lag; at least half a hop.
    pub centre_uncertainty_samples: u64,
    /// The PCM the decoder actually saw.
    pub pcm_span: (u64, u64),
    /// "greedy_tdt" today.
    pub decode: String,
    #[serde(default)]
    pub runtime: String,
    pub events: Vec<WordOnsetEmission>,
}

/// Verdict of word-onset evidence on one adjudication question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WordOnsetVerdict {
    pub observation: ObservationIdentity,
    pub rule_id: String,
    pub sample_start: u64,
    pub sample_end: u64,
    /// Words the sequence under adjudication claims over the span.
    pub claimed_words: usize,
    /// Word onsets the evidence counted there; `None` = no covering window
    /// or covering windows disagree.
    pub measured_onsets: Option<usize>,
    pub agrees: bool,
}

impl WordOnsetEvidence {
    pub fn matches(&self, session: &str, capture_epoch: u64) -> bool {
        self.session == session && self.capture_epoch == capture_epoch
    }

    /// Whether the decoder saw every sample of the span.
    pub fn covers(&self, start: u64, end: u64) -> bool {
        start < end && self.pcm_span.0 <= start && end <= self.pcm_span.1
    }

    /// Word onsets whose uncertainty interval intersects `[start, end)`.
    /// An onset on the edge counts for both neighbours on purpose: the frame
    /// is coarser than the question, so an edge onset is ambiguous evidence,
    /// not evidence of absence.
    pub fn word_onsets_in(&self, start: u64, end: u64) -> usize {
        self.events
            .iter()
            .filter(|event| event.word_start)
            .filter(|event| {
                let lo = event
                    .centre_sample
                    .saturating_sub(self.centre_uncertainty_samples);
                let hi = event
                    .centre_sample
                    .saturating_add(self.centre_uncertainty_samples);
                lo < end && start < hi
            })
            .count()
    }

    /// Structural sanity: a window that cannot describe its own frames is
    /// not evidence. Returns the first defect.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.pcm_span.0 >= self.pcm_span.1 {
            return Err("empty_pcm_span");
        }
        if self.frame_hop_samples == 0 {
            return Err("zero_frame_hop");
        }
        if self.centre_uncertainty_samples < self.frame_hop_samples / 2 {
            return Err("uncertainty_below_half_hop");
        }
        if self.input_sample_rate == 0 {
            return Err("zero_sample_rate");
        }
        for event in &self.events {
            let expected = self
                .frame0_centre_sample
                .saturating_add(u64::from(event.frame) * self.frame_hop_samples);
            if event.centre_sample != expected {
                return Err("event_centre_off_grid");
            }
            if event.centre_sample < self.pcm_span.0 || event.centre_sample > self.pcm_span.1 {
                return Err("event_outside_pcm_span");
            }
        }
        if self
            .events
            .windows(2)
            .any(|pair| pair[0].frame > pair[1].frame)
        {
            return Err("events_not_monotonic");
        }
        Ok(())
    }
}

impl AcousticLedger {
    /// Record one decoded window. Invalid windows are refused, not stored.
    pub fn record_word_onset_evidence(
        &mut self,
        evidence: WordOnsetEvidence,
    ) -> Result<(), &'static str> {
        evidence.validate()?;
        self.word_onset_evidence.push(evidence);
        Ok(())
    }

    pub fn word_onset_evidence(&self) -> &[WordOnsetEvidence] {
        &self.word_onset_evidence
    }

    pub fn word_onset_verdicts(&self) -> &[WordOnsetVerdict] {
        &self.word_onset_verdicts
    }

    /// Word onsets under a span of this capture. `None` when no recorded
    /// window covers the whole span, or when covering windows disagree; a
    /// disagreement between two decodes of the same PCM is a question for the
    /// Founder, not a number for a matcher.
    pub fn word_onset_count(&self, source: &OccurrenceIdentity) -> Option<usize> {
        let mut counts = self
            .word_onset_evidence
            .iter()
            .filter(|evidence| evidence.matches(&source.session, source.capture_epoch))
            .filter(|evidence| evidence.covers(source.sample_start, source.sample_end))
            .map(|evidence| evidence.word_onsets_in(source.sample_start, source.sample_end));
        let first = counts.next()?;
        counts.all(|count| count == first).then_some(first)
    }

    /// Ask the evidence whether `claimed_words` is the number of words spoken
    /// under the span. Records the verdict; `true` also when no evidence
    /// covers the span, because absence of this witness is not a vote.
    pub(super) fn word_onsets_agree(
        &mut self,
        observation: &ObservationIdentity,
        rule_id: &str,
        sample_start: u64,
        sample_end: u64,
        claimed_words: usize,
    ) -> bool {
        let span = OccurrenceIdentity::new(
            &observation.occurrence.session,
            observation.occurrence.capture_epoch,
            sample_start,
            sample_end,
        );
        let measured_onsets = self.word_onset_count(&span);
        let agrees = measured_onsets.is_none_or(|onsets| onsets == claimed_words);
        self.word_onset_verdicts.push(WordOnsetVerdict {
            observation: observation.clone(),
            rule_id: rule_id.to_string(),
            sample_start,
            sample_end,
            claimed_words,
            measured_onsets,
            agrees,
        });
        agrees
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(frame0: u64, span: (u64, u64), word_starts: &[u32]) -> WordOnsetEvidence {
        let hop = 3_840;
        WordOnsetEvidence {
            session: "take".into(),
            capture_epoch: 1,
            model_id: "mlx-community/parakeet-tdt-0.6b-v3@ed2b7e8c".into(),
            weights_sha256: None,
            input_sample_rate: 48_000,
            frame_hop_samples: hop,
            frame0_centre_sample: frame0,
            centre_uncertainty_samples: hop / 2,
            pcm_span: span,
            decode: "greedy_tdt".into(),
            runtime: String::new(),
            events: word_starts
                .iter()
                .map(|&frame| WordOnsetEmission {
                    frame,
                    centre_sample: frame0 + u64::from(frame) * hop,
                    duration_frames: 1,
                    token_id: 7,
                    word_start: true,
                    confidence: 1.0,
                })
                .collect(),
        }
    }

    #[test]
    fn onsets_are_counted_over_the_union_of_two_claims_not_per_claim() {
        // Report case (b): take 018e2177, five word starts over the union of
        // the Apple and Whisper claims; Apple's own claim holds one.
        let evidence = window(0, (0, 575_488), &[67, 70, 72, 74, 80]);
        assert_eq!(evidence.word_onsets_in(250_560, 337_920), 5);
        assert_eq!(evidence.word_onsets_in(250_560, 328_320), 5);
        assert_eq!(evidence.word_onsets_in(284_640, 337_920), 2);
    }

    #[test]
    fn ledger_reports_none_without_coverage_and_on_disagreement() {
        let mut ledger = AcousticLedger::new();
        let span = OccurrenceIdentity::new("take", 1, 250_560, 337_920);
        assert_eq!(ledger.word_onset_count(&span), None);
        ledger
            .record_word_onset_evidence(window(0, (0, 575_488), &[67, 70, 72, 74, 80]))
            .unwrap();
        assert_eq!(ledger.word_onset_count(&span), Some(5));
        // A window that does not cover the span is not consulted.
        ledger
            .record_word_onset_evidence(window(300_000, (300_000, 575_488), &[1, 5]))
            .unwrap();
        assert_eq!(ledger.word_onset_count(&span), Some(5));
        // Two covering decodes that disagree make the span unresolved.
        ledger
            .record_word_onset_evidence(window(0, (0, 575_488), &[67, 70, 72, 74]))
            .unwrap();
        assert_eq!(ledger.word_onset_count(&span), None);
        let other = OccurrenceIdentity::new("other", 1, 250_560, 337_920);
        assert_eq!(ledger.word_onset_count(&other), None);
    }

    #[test]
    fn invalid_windows_are_refused() {
        let mut ledger = AcousticLedger::new();
        let mut off_grid = window(0, (0, 575_488), &[67]);
        off_grid.events[0].centre_sample += 1;
        assert_eq!(
            ledger.record_word_onset_evidence(off_grid),
            Err("event_centre_off_grid")
        );
        let mut narrow = window(0, (0, 575_488), &[67]);
        narrow.centre_uncertainty_samples = 10;
        assert_eq!(
            ledger.record_word_onset_evidence(narrow),
            Err("uncertainty_below_half_hop")
        );
        assert!(ledger.word_onset_evidence().is_empty());
    }

    /// Emission windows exported from parakeet-mlx 0.5.3 on dragon
    /// (parakeet-representation-report.md, step 2), bound to a capture.
    fn recorded_window(file: &str, session: &str) -> WordOnsetEvidence {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/fixtures")
            .join(file);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
        let mut window: WordOnsetEvidence =
            serde_json::from_str(&text).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
        window.session = session.into();
        window.capture_epoch = 1;
        window.validate().unwrap();
        window
    }

    #[test]
    fn recorded_018e2177_names_five_words_under_the_union_of_both_claims() {
        // Apple [284 640, 337 920] and Whisper [250 560, 328 320] both claim
        // "jest w tej chwili dopięty" 0.7 s apart; the capture holds it once.
        let mut ledger = AcousticLedger::new();
        ledger
            .record_word_onset_evidence(recorded_window(
                "parakeet_word_onsets_018e2177.json",
                "take-018e2177",
            ))
            .unwrap();
        let union = OccurrenceIdentity::new("take-018e2177", 1, 250_560, 337_920);
        assert_eq!(ledger.word_onset_count(&union), Some(5));
        let whisper = OccurrenceIdentity::new("take-018e2177", 1, 250_560, 328_320);
        assert_eq!(ledger.word_onset_count(&whisper), Some(5));
        // Apple's claim alone sits late over the tail of the phrase: an edge
        // onset plus "dopięty". The claim is not where the words are.
        let apple = OccurrenceIdentity::new("take-018e2177", 1, 284_640, 337_920);
        assert_eq!(ledger.word_onset_count(&apple), Some(2));
    }

    #[test]
    fn recorded_36de5e98_has_no_word_under_whispers_to_and_one_under_each_real_word() {
        let mut ledger = AcousticLedger::new();
        ledger
            .record_word_onset_evidence(recorded_window(
                "parakeet_word_onsets_36de5e98.json",
                "take-36de5e98-tail",
            ))
            .unwrap();
        // Whisper's edge-cut "to" (live window 6, fence at 27.0 s of the take).
        let to = OccurrenceIdentity::new("take-36de5e98-tail", 1, 410_880, 416_640);
        assert_eq!(ledger.word_onset_count(&to), Some(0));
        let computer = OccurrenceIdentity::new("take-36de5e98-tail", 1, 385_920, 412_800);
        assert_eq!(ledger.word_onset_count(&computer), Some(1));
        let use_ = OccurrenceIdentity::new("take-36de5e98-tail", 1, 412_800, 444_480);
        assert_eq!(ledger.word_onset_count(&use_), Some(1));
        // "pumpy to" and "Computer Use." both claim two words over the same
        // PCM: the witness counts two there and cannot pick the label. That
        // choice stays with the fence evidence and the trial.
        let dispute = OccurrenceIdentity::new("take-36de5e98-tail", 1, 385_920, 444_480);
        assert_eq!(ledger.word_onset_count(&dispute), Some(2));
    }

    #[test]
    fn absence_of_the_witness_is_not_a_vote() {
        let mut ledger = AcousticLedger::new();
        let owner = OccurrenceIdentity::new("take", 1, 249_120, 370_560);
        let observation = ObservationIdentity::new(ObservationProducer::Whisper, 1, 1, owner);
        assert!(ledger.word_onsets_agree(&observation, "t", 250_560, 337_920, 5));
        assert_eq!(ledger.word_onset_verdicts()[0].measured_onsets, None);
        ledger
            .record_word_onset_evidence(window(0, (0, 575_488), &[67, 70, 72, 74, 80]))
            .unwrap();
        assert!(ledger.word_onsets_agree(&observation, "t", 250_560, 337_920, 5));
        assert!(!ledger.word_onsets_agree(&observation, "t", 250_560, 337_920, 10));
        assert_eq!(ledger.word_onset_verdicts().len(), 3);
        assert_eq!(ledger.word_onset_verdicts()[2].measured_onsets, Some(5));
    }
}
