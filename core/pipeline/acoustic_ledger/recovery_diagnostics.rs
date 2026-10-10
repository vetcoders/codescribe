//! Read-only explanations of ledger recovery and coverage refusals.
//! These snapshots never settle debt or grant transcript authority.

use super::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryPinDiagnostic {
    pub observation: ObservationIdentity,
    pub sample_start: u64,
    pub sample_end: u64,
    pub complete_word: bool,
    pub accepted_observation: bool,
    pub decode: Option<(u64, u64)>,
    pub refusal_reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryAlternativeDiagnostic {
    pub observation: ObservationIdentity,
    pub reason: String,
    pub decode: Option<(u64, u64)>,
    /// The decision for this observation, when retained; not an inferred cause.
    pub decision_ordinal: Option<usize>,
    pub sources: Vec<RecoveryPinDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryDiagnostic {
    pub owner: OccurrenceIdentity,
    pub committed: bool,
    pub pending: bool,
    pub rejected_pins: Vec<RecoveryPinDiagnostic>,
    pub retained_sources: Vec<RecoveryAlternativeDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageDiagnostics {
    pub session_id: String,
    pub capture_epoch: u64,
    /// Measured speech with no committed owner, before recovery exclusion.
    pub missing_committed_speech: Vec<OccurrenceIdentity>,
    /// Existing owners excluded from certification by unresolved recovery.
    pub text_recovery: Vec<RecoveryDiagnostic>,
}

impl AcousticLedger {
    fn recovery_pin_diagnostic(&self, pin: &WordSlot) -> RecoveryPinDiagnostic {
        RecoveryPinDiagnostic {
            observation: pin.observation.clone(),
            sample_start: pin.sample_start,
            sample_end: pin.sample_end,
            complete_word: self.complete_word_slot(pin),
            accepted_observation: self.word_pin_observations.contains(&pin.observation),
            decode: self.decoded_word_windows.get(&pin.observation).copied(),
            refusal_reasons: self
                .slot_alternatives
                .iter()
                .filter(|alternative| alternative.sources.contains(pin))
                .map(|alternative| alternative.reason.to_string())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        }
    }

    pub(crate) fn recovery_diagnostic(&self, owner: &OccurrenceIdentity) -> RecoveryDiagnostic {
        RecoveryDiagnostic {
            owner: owner.clone(),
            committed: self.committed.contains_key(owner),
            pending: self.text_recovery_pending(owner),
            rejected_pins: self
                .rejected_word_pins
                .get(owner)
                .into_iter()
                .flatten()
                .map(|pin| self.recovery_pin_diagnostic(pin))
                .collect(),
            retained_sources: self
                .slot_alternatives
                .iter()
                .filter(|alternative| alternative.observation.occurrence == *owner)
                .filter_map(|alternative| {
                    let sources = alternative
                        .sources
                        .iter()
                        .filter(|source| self.recovery_source_is_pending(alternative, source))
                        .map(|source| self.recovery_pin_diagnostic(source))
                        .collect::<Vec<_>>();
                    if sources.is_empty() {
                        return None;
                    }
                    Some(RecoveryAlternativeDiagnostic {
                        observation: alternative.observation.clone(),
                        reason: alternative.reason.into(),
                        decode: self
                            .decoded_word_windows
                            .get(&alternative.observation)
                            .copied(),
                        decision_ordinal: self
                            .layer_trail()
                            .iter()
                            .rev()
                            .find(|decision| decision.observation == alternative.observation)
                            .map(|decision| decision.ordinal),
                        sources,
                    })
                })
                .collect(),
        }
    }

    pub fn coverage_diagnostics(&self, receipt: &SealCoverageReceipt) -> CoverageDiagnostics {
        let mut missing_committed_speech = Vec::new();
        for range in &receipt.uncovered_speech_ranges {
            if range.session != receipt.session_id || range.capture_epoch != receipt.capture_epoch {
                continue;
            }
            let mut cursor = range.sample_start;
            for owner in self.committed.keys().filter(|owner| {
                owner.session == receipt.session_id && owner.capture_epoch == receipt.capture_epoch
            }) {
                if owner.sample_end <= cursor || owner.sample_start >= range.sample_end {
                    continue;
                }
                if owner.sample_start > cursor {
                    missing_committed_speech.push(OccurrenceIdentity::new(
                        &receipt.session_id,
                        receipt.capture_epoch,
                        cursor,
                        owner.sample_start.min(range.sample_end),
                    ));
                }
                cursor = cursor.max(owner.sample_end).min(range.sample_end);
                if cursor >= range.sample_end {
                    break;
                }
            }
            if cursor < range.sample_end {
                missing_committed_speech.push(OccurrenceIdentity::new(
                    &receipt.session_id,
                    receipt.capture_epoch,
                    cursor,
                    range.sample_end,
                ));
            }
        }
        CoverageDiagnostics {
            session_id: receipt.session_id.clone(),
            capture_epoch: receipt.capture_epoch,
            missing_committed_speech,
            text_recovery: self
                .pending_text_recoveries(&receipt.session_id, receipt.capture_epoch)
                .iter()
                .map(|owner| self.recovery_diagnostic(owner))
                .collect(),
        }
    }
}
