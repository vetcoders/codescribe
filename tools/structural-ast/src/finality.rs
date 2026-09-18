//! Closed AST productions for ledger inspection and refused-document authority.
use crate::Grammar;
use syn::{Block, parse_quote};

pub(super) fn ledger(g: &mut Grammar, body: &Block) {
    let expected: Block = parse_quote!({
        let coverage = self.latest_seal_coverage.as_ref().filter(|receipt| {
            receipt.session_id == session && receipt.capture_epoch == capture_epoch
        });
        let pending = !self
            .pending_text_recoveries(session, capture_epoch)
            .is_empty();
        let reason = if pending {
            TerminalFinalityRefusalReason::TextRecoveryPending
        } else if coverage.is_some_and(|receipt| !receipt.status.is_complete()) {
            TerminalFinalityRefusalReason::CoverageRefused
        } else {
            if let Some(receipt) = self.terminal_seals.iter().rev().find(|receipt| {
                receipt.coverage.session == session
                    && receipt.coverage.capture_epoch == capture_epoch
                    && self
                        .evidence
                        .keys()
                        .chain(self.committed.keys())
                        .filter(|range| {
                            range.session == session && range.capture_epoch == capture_epoch
                        })
                        .all(|range| receipt.sealed_occurrences.contains(range))
                    && receipt
                        .sealed_occurrences
                        .iter()
                        .all(|range| self.evidence.contains_key(range))
            }) {
                return TerminalFinality::Sealed(receipt.clone());
            }
            let has_occurrences = self
                .evidence
                .keys()
                .chain(self.committed.keys())
                .any(|range| range.session == session && range.capture_epoch == capture_epoch);
            if !has_occurrences
                && let Some(receipt) = coverage.filter(|receipt| {
                    receipt.status.is_complete()
                        && receipt.speech_samples == 0
                        && receipt.covered_samples == 0
                        && receipt.uncovered_speech_ranges.is_empty()
                        && receipt.availability == "observed"
                        && receipt.observed_samples.is_some()
                })
            {
                return TerminalFinality::ObservedSilence(receipt.clone());
            }
            TerminalFinalityRefusalReason::TerminalReceiptMissing
        };
        TerminalFinality::Refused(TerminalFinalityRefusal {
            session_id: session.to_string(),
            capture_epoch,
            reason,
            coverage: coverage.cloned(),
        })
    });
    g.require(
        body == &expected,
        "BOUNDARY: unsupported identity-bound finality inspection or non-complete receipt refusal",
    );
}

pub(super) fn empty(g: &mut Grammar, body: &Block) {
    let expected: Block = parse_quote!({
        self.evidence.is_empty()
            && self.committed.is_empty()
            && self.pending_text_recovery.is_empty()
            && self.terminal_seals.is_empty()
            && self.latest_seal_coverage.is_none()
    });
    g.require(
        body == &expected,
        "BOUNDARY: unsupported empty-capture evidence predicate",
    );
}

pub(super) fn bus(g: &mut Grammar, body: &Block) {
    let expected: Block = parse_quote!({
        let writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        writer.started
            && !writer.ended
            && !writer.sealed
            && refusal.session_id() == self.session.session_id
            && writer.last_projection.as_ref().is_some_and(|last| {
                last.capture_epoch == refusal.capture_epoch()
                    && last.rendered_text == text
                    && last.seal_coverage
                        == refusal.coverage().map(ProjectedSealCoverageReceipt::from)
            })
    });
    g.require(
        body == &expected,
        "BOUNDARY: unsupported exact refused Bus document predicate",
    );
}

pub(super) fn controller(g: &mut Grammar, body: &Block) {
    let expected: Block = parse_quote!({
        let refusal = error.downcast::<TerminalSealRefused>()?;
        let take_id = self.session_id.read().await.clone();
        if retainable_session_id(take_id.as_deref()) != Some(refusal.finality.session_id()) {
            return Err(anyhow::anyhow!(
                "terminal refusal does not match the active capture"
            ));
        }
        if refusal.committed_text.trim().is_empty() {
            return Err(anyhow::Error::new(refusal));
        }
        let bus = self.active_transcript_bus.read().await.clone();
        if bus.as_ref().is_none_or(|bus| {
            !bus.matches_refused_document(&refusal.finality, &refusal.committed_text)
        }) {
            return Err(anyhow::anyhow!(
                "terminal refusal has no matching authenticated Bus document"
            ));
        }
        match deliver(refusal.committed_text.clone()).await {
            Ok(_) => Ok(ProcessRecordingOutcome {
                transcript_present: true,
                refusal: Some(refusal),
                ..ProcessRecordingOutcome::default()
            }),
            Err(cause) => Err(anyhow::Error::new(StopDeliveryFailure {
                cause,
                refusal: Some(refusal),
            })),
        }
    });
    g.require(
        body == &expected,
        "BOUNDARY: unsupported identity and document authorized refusal handoff",
    );
}
