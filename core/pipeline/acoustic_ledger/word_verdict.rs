//! No-speech adjudication on exact word PCM using the session's existing
//! Silero measurement. This module never creates a VAD or opens capture.

use super::*;
use crate::audio::capture_receipt::DIGITAL_ZERO_ABS;
use crate::pipeline::streaming::silero_fusion::SILERO_BOUNDARIES_PRODUCER;

pub const WORD_NO_SPEECH_RULE: &str = "word-no-speech/digital-zero+silero/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordVerdict {
    ConfirmedNoSpeech,
    SpeechEvidence,
    /// Sound was measured but has no classification proving absence of speech.
    SoundUndetermined,
    Undetermined,
}

/// Only the adjudicator constructs a receipt. An empty VAD range set cannot
/// be presented directly as a deletion permission.
#[derive(Debug, Clone, PartialEq)]
pub struct WordVerdictReceipt {
    target: OccurrenceIdentity,
    verdict: WordVerdict,
    rule_version: &'static str,
    peak_threshold: f64,
    energy_per_sample_threshold: f64,
    energy_integral: Option<f64>,
    peak_abs: Option<f64>,
    measured_samples: u64,
    silero_observed_samples: Option<u64>,
    /// Silero's measured speech intervals (opening and closing sample edges).
    vad_ranges: Vec<OccurrenceIdentity>,
    reason: &'static str,
    /// A future event classifier may attach evidence here. Unknown sound
    /// never acquires a cough or laughter label from this adjudicator.
    non_speech_evidence: Option<crate::pipeline::contracts::NonSpeechEvidence>,
}

impl WordVerdictReceipt {
    pub fn target(&self) -> &OccurrenceIdentity {
        &self.target
    }
    pub fn verdict(&self) -> WordVerdict {
        self.verdict
    }
    pub fn rule_version(&self) -> &'static str {
        self.rule_version
    }
    pub fn energy_integral(&self) -> Option<f64> {
        self.energy_integral
    }
    pub fn peak_abs(&self) -> Option<f64> {
        self.peak_abs
    }
    pub fn thresholds(&self) -> (f64, f64) {
        (self.peak_threshold, self.energy_per_sample_threshold)
    }
    pub fn measured_samples(&self) -> u64 {
        self.measured_samples
    }
    pub fn silero_observed_samples(&self) -> Option<u64> {
        self.silero_observed_samples
    }
    pub fn vad_ranges(&self) -> &[OccurrenceIdentity] {
        &self.vad_ranges
    }
    pub fn reason(&self) -> &'static str {
        self.reason
    }
    pub fn non_speech_evidence(&self) -> Option<crate::pipeline::contracts::NonSpeechEvidence> {
        self.non_speech_evidence
    }
}

/// `pcm_range` authenticates the exact slice supplied by the capture owner.
/// Whole-sentence energy cannot substitute for that slice. Silero availability
/// and epoch are checked independently, before any no-speech verdict.
pub fn adjudicate_word_pcm(
    target: &OccurrenceIdentity,
    pcm_range: &OccurrenceIdentity,
    pcm: &[f32],
    silero: &AcousticSpeechEvidence,
) -> WordVerdictReceipt {
    let mut receipt = WordVerdictReceipt {
        target: target.clone(),
        verdict: WordVerdict::Undetermined,
        rule_version: WORD_NO_SPEECH_RULE,
        peak_threshold: f64::from(DIGITAL_ZERO_ABS),
        energy_per_sample_threshold: f64::from(DIGITAL_ZERO_ABS).powi(2),
        energy_integral: None,
        peak_abs: None,
        measured_samples: pcm.len() as u64,
        silero_observed_samples: silero.availability().observed_samples(),
        vad_ranges: silero
            .ranges()
            .iter()
            .map(OccurrenceIdentity::from)
            .collect(),
        reason: "evidence_unavailable",
        non_speech_evidence: None,
    };
    if !target.is_anchored() || target != pcm_range || pcm.len() as u64 != target.sample_len() {
        receipt.reason = "pcm_range_or_completeness_mismatch";
        return receipt;
    }
    if silero.producer() != SILERO_BOUNDARIES_PRODUCER
        || !silero
            .identity()
            .matches(&target.session, target.capture_epoch)
        || receipt
            .silero_observed_samples
            .is_none_or(|extent| extent < target.sample_end)
        || receipt.vad_ranges.iter().any(|range| {
            !range.same_capture(target)
                || !range.is_anchored()
                || range.sample_end > receipt.silero_observed_samples.unwrap_or(0)
        })
    {
        receipt.reason = "silero_identity_or_completeness_mismatch";
        return receipt;
    }
    if pcm.iter().any(|sample| !sample.is_finite()) {
        receipt.reason = "invalid_pcm";
        return receipt;
    }
    let energy = pcm
        .iter()
        .map(|sample| f64::from(*sample).powi(2))
        .sum::<f64>();
    let peak = pcm
        .iter()
        .map(|sample| f64::from(sample.abs()))
        .fold(0.0, f64::max);
    receipt.energy_integral = Some(energy);
    receipt.peak_abs = Some(peak);
    if receipt.vad_ranges.iter().any(|range| {
        range.sample_start < target.sample_end && target.sample_start < range.sample_end
    }) {
        receipt.verdict = WordVerdict::SpeechEvidence;
        receipt.reason = "silero_speech_intersects_word";
    } else if peak <= receipt.peak_threshold
        && energy <= receipt.energy_per_sample_threshold * target.sample_len() as f64
    {
        receipt.verdict = WordVerdict::ConfirmedNoSpeech;
        receipt.reason = "complete_digital_zero_pcm_and_silero_no_speech";
    } else {
        receipt.verdict = WordVerdict::SoundUndetermined;
        receipt.reason = "sound_without_proof_of_no_speech";
    }
    receipt
}

#[derive(Debug, Clone, PartialEq)]
pub struct WordDeletionReceipt {
    pub operation: SlotOperationReceipt,
    pub verdict: WordVerdictReceipt,
}

impl AcousticLedger {
    pub fn word_deletions(&self) -> &[WordDeletionReceipt] {
        &self.word_deletions
    }

    /// Delete one unprotected, current source only with the adjudicator's
    /// positive no-speech receipt on precisely its source PCM.
    pub fn remove_word_with_verdict(
        &mut self,
        observation: &ObservationIdentity,
        target: &SlotTarget,
        verdict: &WordVerdictReceipt,
    ) -> Result<WordDeletionReceipt, SlotOperationRefusal> {
        let sources =
            self.resolve_slot_targets(observation, std::slice::from_ref(target), false)?;
        if sources[0].producer == ObservationProducer::ManualHuman {
            return Err(SlotOperationRefusal::ProtectedHuman);
        }
        let ranges = self.slot_source_ranges(&sources[0]);
        if verdict.verdict != WordVerdict::ConfirmedNoSpeech
            || ranges.as_slice() != std::slice::from_ref(&verdict.target)
        {
            return Err(SlotOperationRefusal::InvalidEvidence);
        }
        let operation = SlotOperationReceipt {
            observation: observation.clone(),
            kind: SlotOperationKind::Delete,
            sources,
            outputs: Vec::new(),
            source_ranges: ranges,
            rule_id: verdict.rule_version.to_string(),
        };
        self.commit_slot_operation(operation.clone());
        let receipt = WordDeletionReceipt {
            operation,
            verdict: verdict.clone(),
        };
        self.word_deletions.push(receipt.clone());
        Ok(receipt)
    }
}
