//! Live fence authority: clock-lie accounting, cumulative-final admission,
//! observation frontiers, terminal finality and seals.
//!
//! Committed prefixes freeze here. A cumulative engine final may restate them
//! but never re-own them ([`CumulativeFinalAdmission`]); a frontier must
//! return before an occurrence seals; a sealed occurrence accepts no later
//! engine mutation, and time never runs backwards across a seal.

use super::*;

/// Decision for a cumulative engine final that restates committed text.
///
/// The Apple live lane emits finals whose text restates the whole phrase while
/// the window it declares covers only the newest audio. Splitting restated from
/// novel is therefore a *text* operation, and this enum bounds how much
/// authority that text operation is allowed to have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CumulativeFinalAdmission {
    /// The declared window is disjoint from every committed occurrence: this is
    /// new audio, the whole callback is novel, and text alignment is not
    /// consulted at all.
    WhollyNovel,
    /// The window overlaps committed occurrences. Alignment is authorized, but
    /// only up to the number of canvas words those occurrences actually hold —
    /// a restatement may not claim more occurrences than exist.
    AlignInside {
        /// Canvas words, counted from the commit cursor backwards, that the
        /// overlapped occurrences account for.
        canvas_words_under_authority: usize,
    },
    /// No usable anchor on either side. Legacy text behaviour stands, because
    /// there is no acoustic authority for it to violate.
    NoAnchor {
        /// Why authority is absent.
        reason: NoAuthorityReason,
    },
}

impl CumulativeFinalAdmission {
    /// Decide how much of the canvas a cumulative final may claim to restate.
    ///
    /// `window` is the range the callback declares. `committed` pairs each
    /// canvas span with the number of canvas words it contributed, newest last.
    /// Spans whose range is unanchored are treated as absent evidence, never as
    /// objections.
    pub fn decide(window: &OccurrenceIdentity, committed: &[(OccurrenceIdentity, usize)]) -> Self {
        if !window.is_anchored() {
            return Self::NoAnchor {
                reason: NoAuthorityReason::ZeroWidth,
            };
        }
        let comparable: Vec<&(OccurrenceIdentity, usize)> = committed
            .iter()
            .filter(|(occurrence, _)| occurrence.is_anchored() && occurrence.same_capture(window))
            .collect();
        if comparable.is_empty() {
            // Nothing anchored to compare against. The legacy text lane keeps
            // its behaviour rather than being handed a false verdict.
            return Self::NoAnchor {
                reason: NoAuthorityReason::NoRange,
            };
        }
        let mut words_under_authority = 0usize;
        let mut touched = false;
        // Walk newest-first: authority extends backwards from the commit cursor
        // only while the occurrences keep sharing audio with the window.
        for (occurrence, words) in comparable.iter().rev() {
            match window.relate(occurrence) {
                OccurrenceRelation::Same | OccurrenceRelation::Overlapping { .. } => {
                    touched = true;
                    words_under_authority += words;
                }
                OccurrenceRelation::Disjoint => break,
                OccurrenceRelation::Unanchored | OccurrenceRelation::DifferentCapture => continue,
            }
        }
        if !touched {
            return Self::WhollyNovel;
        }
        Self::AlignInside {
            canvas_words_under_authority: words_under_authority,
        }
    }

    /// Authority for a *cumulative* producer, whose text restates more audio
    /// than the window it declares.
    ///
    /// The Apple live lane emits segment-less finals that restate the whole
    /// phrase while the window they carry covers only the newest audio. Such a
    /// window cannot bound the alignment, because the producer under-declares
    /// by construction — believing it would mark the whole restatement as new
    /// audio and commit every occurrence twice.
    ///
    /// What does bound the alignment is the finality bar. A live callback may
    /// realign occurrences the live lane still holds open, and has no authority
    /// over anything already past `transcript_sealed`. `open_occurrences` is
    /// that set, each paired with the number of canvas words it contributed.
    pub fn for_cumulative_restatement(open_occurrences: &[(OccurrenceIdentity, usize)]) -> Self {
        let words: usize = open_occurrences.iter().map(|(_, words)| words).sum();
        if words == 0 {
            return Self::NoAnchor {
                reason: NoAuthorityReason::NoRange,
            };
        }
        Self::AlignInside {
            canvas_words_under_authority: words,
        }
    }

    /// Slice the canvas down to the region the decision authorises.
    ///
    /// Alignment runs *after* authority is established and only inside the
    /// authorised span; a textual match outside it is a coincidence, not
    /// identity, and must not be reachable by the matcher at all.
    pub fn authorized_canvas<'c>(&self, canvas: &'c [&'c str]) -> &'c [&'c str] {
        match self {
            Self::WhollyNovel => &[],
            Self::AlignInside {
                canvas_words_under_authority,
            } => {
                let take = (*canvas_words_under_authority).min(canvas.len());
                &canvas[canvas.len() - take..]
            }
            Self::NoAnchor { .. } => canvas,
        }
    }

    /// Clamp a text matcher's answer to what acoustic evidence permits.
    ///
    /// This is where string-prefix and overlap heuristics lose their authority: their answer is an
    /// alignment *hint*, and on an anchored span it may never exceed the number
    /// of committed occurrences the window actually overlaps.
    pub fn clamp_known_prefix(&self, matcher_known_words: usize) -> usize {
        match self {
            Self::WhollyNovel => 0,
            Self::AlignInside {
                canvas_words_under_authority,
            } => matcher_known_words.min(*canvas_words_under_authority),
            Self::NoAnchor { .. } => matcher_known_words,
        }
    }

    /// Stable label for logs.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::WhollyNovel => "wholly_novel",
            Self::AlignInside { .. } => "align_inside",
            Self::NoAnchor { .. } => "no_anchor",
        }
    }
}

// ---------------------------------------------------------------------------
// Observation frontier
// ---------------------------------------------------------------------------

/// Which scheduled producers may still speak about a range.
///
/// * inputs: the schedule of producers the session actually dispatched for a
///   range, and their returns.
/// * outputs: the closed/open verdict the seal depends on.
/// * invariants: closure means *no scheduled producer can still return a valid
///   observation*. It is not a timeout and not a guess about lateness; an
///   unreturned producer keeps the frontier open however long it takes. This is
///   the N6 shape: VAD closed, frontier still open, therefore not sealed.
/// * intended W2 consumers: the streaming session scheduler, which reports
///   dispatch and completion, and never the seal itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationFrontier {
    /// Range the frontier is kept for.
    pub coverage: OccurrenceIdentity,
    pub(super) scheduled: BTreeSet<ObservationProducer>,
    pub(super) returned: BTreeSet<ObservationProducer>,
}

impl ObservationFrontier {
    /// Open a frontier for the producers a session actually scheduled.
    pub fn scheduled(
        coverage: OccurrenceIdentity,
        producers: impl IntoIterator<Item = ObservationProducer>,
    ) -> Self {
        Self {
            coverage,
            scheduled: producers.into_iter().collect(),
            returned: BTreeSet::new(),
        }
    }

    /// Extend an open launch contract without discarding returns already
    /// recorded for synchronous observers.
    fn schedule(&mut self, producer: ObservationProducer) -> bool {
        self.scheduled.insert(producer)
    }

    /// Record that a scheduled producer has returned everything it will return.
    pub fn record_return(&mut self, producer: ObservationProducer) {
        if self.scheduled.contains(&producer) {
            self.returned.insert(producer);
        }
    }

    /// Producers that were scheduled and have not finished.
    pub fn open_producers(&self) -> Vec<ObservationProducer> {
        self.scheduled.difference(&self.returned).copied().collect()
    }

    /// Whether every scheduled producer has finished.
    pub fn is_closed(&self) -> bool {
        self.scheduled.is_subset(&self.returned)
    }

    /// Stable label for receipts and logs.
    pub fn as_str(&self) -> &'static str {
        if self.is_closed() { "closed" } else { "open" }
    }
}

// ---------------------------------------------------------------------------
// Seal
// ---------------------------------------------------------------------------

/// Why a range could not be sealed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealRefusal {
    /// No energy-qualified occurrence exists for the range.
    NotQualified,
    /// VAD never emitted a closing boundary at or after the last sample.
    VadDidNotClose,
    /// A scheduled producer can still return a valid observation.
    FrontierOpen,
    /// No frontier was ever scheduled for the range, so closure is unknown.
    /// Unknown is not closed.
    FrontierUnknown,
    /// All scheduled observers returned, but none supplied a usable label.
    LabelMissing,
    /// Speech outlasted its provisional label and has not been recovered.
    TextRecoveryPending,
    /// An admitted observation for the range has no decision receipt.
    ObservationsWithoutReceipts,
    /// The terminal seal was asked for while an occurrence in the epoch is
    /// still unsealed.
    OccurrenceStillOpen,
    /// Measured speech still contains a material uncovered range.
    CoverageIncomplete,
}

/// Inspection of existing terminal evidence, not a request to create it.
#[derive(Debug, Clone, PartialEq)]
pub enum TerminalFinality {
    Sealed(LedgerSealReceipt),
    ObservedSilence(SealCoverageReceipt),
    Refused(TerminalFinalityRefusal),
}

impl TerminalFinality {
    pub fn into_refusal(self) -> Option<TerminalFinalityRefusal> {
        match self {
            Self::Refused(refusal) => Some(refusal),
            Self::Sealed(_) | Self::ObservedSilence(_) => None,
        }
    }
}

/// Why existing evidence does not authorize a terminal transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalFinalityRefusalReason {
    CoverageRefused,
    TextRecoveryPending,
    TerminalReceiptMissing,
}

impl TerminalFinalityRefusalReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CoverageRefused => "coverage_refused",
            Self::TextRecoveryPending => "text_recovery_pending",
            Self::TerminalReceiptMissing => "terminal_receipt_missing",
        }
    }
}

/// Ledger-owned, identity-bound refusal. Coverage retains its measured meaning.
#[derive(Debug, Clone, PartialEq)]
pub struct TerminalFinalityRefusal {
    session_id: String,
    capture_epoch: u64,
    reason: TerminalFinalityRefusalReason,
    coverage: Option<SealCoverageReceipt>,
}

impl TerminalFinalityRefusal {
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    pub fn capture_epoch(&self) -> u64 {
        self.capture_epoch
    }
    pub fn reason(&self) -> TerminalFinalityRefusalReason {
        self.reason
    }
    pub fn coverage(&self) -> Option<&SealCoverageReceipt> {
        self.coverage.as_ref()
    }
}

impl SealRefusal {
    /// Stable label for receipts and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotQualified => "not_qualified",
            Self::VadDidNotClose => "vad_did_not_close",
            Self::FrontierOpen => "frontier_open",
            Self::FrontierUnknown => "frontier_unknown",
            Self::LabelMissing => "label_missing",
            Self::TextRecoveryPending => "text_recovery_pending",
            Self::ObservationsWithoutReceipts => "observations_without_receipts",
            Self::OccurrenceStillOpen => "occurrence_still_open",
            Self::CoverageIncomplete => "coverage_incomplete",
        }
    }
}

/// Finality scope minted by the ledger; independent of occurrence count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerSealScope {
    Occurrence,
    Terminal,
}

impl LedgerSealScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Occurrence => "occurrence",
            Self::Terminal => "terminal",
        }
    }
}

/// An immutable ledger fact: this coverage is finished.
///
/// * inputs: the qualified serial, the VAD closing boundary, the closed
///   [`ObservationFrontier`], and the complete decision trail for the range.
/// * outputs: the fence every later automatic producer meets in
///   [`AcousticLedger::admit`], and the terminal seal a session ends on.
/// * invariants: a seal is never lifted, downgraded, or recomputed. One receipt
///   shape covers both a single occurrence and a whole session/epoch, because a
///   seal is a statement about a coverage, and a coverage is a coordinate.
/// * intended W2 consumers: the Transcript Bus terminal seal event and the
///   reducer's finality projection — both of which read this fact rather than
///   deciding finality themselves.
#[derive(Debug, Clone, PartialEq)]
pub struct LedgerSealReceipt {
    /// Lexical settlement is independent of acoustic coverage and finality.
    pub word_finality: Vec<word_adjudication::WordFinality>,
    /// Ledger-minted scope. Cardinality never determines finality.
    pub scope: LedgerSealScope,
    /// Stable identifier for cross-referencing from a projection.
    pub receipt_id: String,
    /// Coordinate the seal covers.
    pub coverage: OccurrenceIdentity,
    /// Occurrences the seal makes final: one for occurrence scope, one or more
    /// for terminal scope. The length does not determine the scope.
    pub sealed_occurrences: Vec<OccurrenceIdentity>,
    /// Serials of those occurrences, in the same order.
    pub serials: Vec<AcousticSerial>,
    /// VAD closing boundary that permitted the seal.
    pub vad_close_sample: u64,
    /// The frontier as it stood, closed, at seal time.
    pub frontier: ObservationFrontier,
    /// Ordinals of the decision trail entries the seal makes final.
    pub layer_trail_ordinals: Vec<usize>,
}

impl LedgerSealReceipt {
    /// Stable state label for receipts and logs. A seal only ever has one.
    pub fn state(&self) -> &'static str {
        "sealed"
    }

    /// Whether the ledger minted occurrence finality, including single-entry terminals.
    pub fn is_occurrence_seal(&self) -> bool {
        self.scope == LedgerSealScope::Occurrence
    }
}

impl AcousticLedger {
    /// Read issued finality for exactly one capture; never mint a seal here.
    pub fn terminal_finality(&self, session: &str, capture_epoch: u64) -> TerminalFinality {
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
    }

    /// No ledger fact can conflict with an empty, zero-sample capture outcome.
    pub fn has_no_capture_facts(&self) -> bool {
        self.evidence.is_empty()
            && self.committed.is_empty()
            && self.pending_text_recovery.is_empty()
            && self.terminal_seals.is_empty()
            && self.latest_seal_coverage.is_none()
    }

    /// Spans whose character rate over the declared range is a clock-lie.
    pub fn clock_lie_count(&self) -> usize {
        self.clock_lie_occurrences.len()
    }

    /// Whether this occurrence was flagged at admission.
    pub fn is_clock_lie_span(&self, occurrence: &OccurrenceIdentity) -> bool {
        self.clock_lie_occurrences.contains(occurrence)
    }

    /// The latest stored receipt named a clock-lie on the span it kept.
    pub fn latest_receipt_names_clock_lie(&self) -> bool {
        self.trail.last().is_some_and(|entry| entry.clock_lie)
    }

    /// Milliseconds since the unix epoch of the first terminal seal.
    pub fn transcript_seal_timestamp_ms(&self) -> Option<u64> {
        self.transcript_seal_timestamp_ms
    }

    pub(super) fn note_clock_lie(
        &mut self,
        observation: &ObservationIdentity,
        decision: &MutationReceipt,
    ) -> bool {
        let Some(rate) = self.capture_rate_hz.filter(|rate| *rate > 0) else {
            return false;
        };
        // Flags describe currently held text, never an offer or display-only
        // stub. Replacements and retirement remove their obsolete flags.
        self.clock_lie_occurrences.retain(|occurrence| {
            self.committed.get(occurrence).is_some_and(|held| {
                is_clock_lie(
                    held.label.chars().count(),
                    occurrence.sample_len() as f32 / rate as f32,
                )
            })
        });
        if !matches!(
            decision,
            MutationReceipt::Insert { .. }
                | MutationReceipt::Correct { .. }
                | MutationReceipt::Preserve { .. }
        ) {
            return false;
        }
        let occurrence = &observation.occurrence;
        let Some(held) = self.committed.get(occurrence) else {
            return false;
        };
        let flagged = occurrence.is_anchored()
            && is_clock_lie(
                held.label.chars().count(),
                occurrence.sample_len() as f32 / rate as f32,
            );
        if flagged {
            self.clock_lie_occurrences.insert(occurrence.clone());
        }
        flagged
    }

    // -- observation frontier ----------------------------------------------

    /// Declare which producers the session actually scheduled for a range.
    ///
    /// Rescheduling replaces the frontier: a range whose producer set changed
    /// has a new closure question, not an amended old one.
    pub fn schedule_frontier(
        &mut self,
        coverage: OccurrenceIdentity,
        producers: impl IntoIterator<Item = ObservationProducer>,
    ) {
        let frontier = ObservationFrontier::scheduled(coverage.clone(), producers);
        crate::pipeline::trail::record(
            &coverage,
            crate::pipeline::trail::TrailEvent::Frontier {
                occurrence: coverage.clone(),
                operation: "schedule".into(),
                producers: frontier.scheduled.iter().copied().collect(),
            },
        );
        self.frontiers.insert(coverage, frontier);
    }

    /// Add one observer only after its concrete job has taken ownership of the
    /// exact occurrence. Existing returns are preserved; configuration intent
    /// alone must never call this operation. Returns `false` when the base
    /// frontier is absent, already closed, already sealed, or already contains
    /// that producer (including a producer that has already returned).
    pub fn schedule_observer(
        &mut self,
        coverage: OccurrenceIdentity,
        producer: ObservationProducer,
    ) -> bool {
        if self.seals.contains_key(&coverage) {
            return false;
        }
        let Some(frontier) = self.frontiers.get_mut(&coverage) else {
            return false;
        };
        if frontier.is_closed() {
            return false;
        }
        let scheduled = frontier.schedule(producer);
        if scheduled {
            crate::pipeline::trail::record(
                &coverage,
                crate::pipeline::trail::TrailEvent::Frontier {
                    occurrence: coverage.clone(),
                    operation: "observer".into(),
                    producers: vec![producer],
                },
            );
        }
        scheduled
    }

    /// A completed lexical trial can make words ready after the acoustic
    /// producers have all returned. Register only this optional formatter;
    /// ordinary producers retain the no-reopening rule in `schedule_observer`.
    pub(crate) fn schedule_settled_formatter(&mut self, coverage: OccurrenceIdentity) -> bool {
        if self.is_sealed(&coverage)
            || self.text_recovery_pending(&coverage)
            || !self.word_labels_settled(&coverage)
            || self
                .text_of(&coverage)
                .is_none_or(|label| label.trim().is_empty())
        {
            return false;
        }
        let Some(frontier) = self.frontiers.get_mut(&coverage) else {
            return false;
        };
        if !frontier.is_closed() || !frontier.schedule(ObservationProducer::Formatter) {
            return false;
        }
        crate::pipeline::trail::record(
            &coverage,
            crate::pipeline::trail::TrailEvent::Frontier {
                occurrence: coverage.clone(),
                operation: "settled_formatter".into(),
                producers: vec![ObservationProducer::Formatter],
            },
        );
        true
    }

    /// Account for real Apple words arriving after all earlier jobs returned
    /// without a label. Only this exact qualified occurrence may gain its first
    /// Apple and slice-local Lexicon observations. No old return is removed and
    /// no producer may run twice; successful seals and ordinary scheduling keep
    /// their existing fences. The caller already holds the words and immediately
    /// admits Apple followed by its Lexicon no-change observation.
    pub fn schedule_late_apple_label(
        &mut self,
        observation: &ObservationIdentity,
        text: &str,
    ) -> bool {
        let occurrence = &observation.occurrence;
        if observation.producer != ObservationProducer::Apple
            || text.trim().is_empty()
            || !self
                .serial_of(occurrence)
                .is_some_and(AcousticSerial::vad_closed)
            || self.is_sealed(occurrence)
            || self.committed.contains_key(occurrence)
            || self.answered.contains(observation)
        {
            return false;
        }
        let Some(frontier) = self.frontiers.get_mut(occurrence) else {
            return false;
        };
        if !frontier.is_closed()
            || !frontier.returned.contains(&ObservationProducer::Whisper)
            || frontier.scheduled.contains(&ObservationProducer::Apple)
            || frontier.scheduled.contains(&ObservationProducer::Lexicon)
        {
            return false;
        }
        frontier.schedule(ObservationProducer::Apple);
        frontier.schedule(ObservationProducer::Lexicon);
        true
    }

    /// Record that a scheduled producer finished with a range.
    ///
    /// Returns `true` only for the transition from open to closed. Repeated
    /// completion is idempotent and cannot cause a second seal emission. A
    /// range with no scheduled frontier answers `false`: unknown is never
    /// closed.
    pub fn note_frontier_return(
        &mut self,
        coverage: &OccurrenceIdentity,
        producer: ObservationProducer,
    ) -> bool {
        crate::pipeline::trail::record(
            coverage,
            crate::pipeline::trail::TrailEvent::Frontier {
                occurrence: coverage.clone(),
                operation: "return".into(),
                producers: vec![producer],
            },
        );
        let closed = match self.frontiers.get_mut(coverage) {
            Some(frontier) => {
                let was_closed = frontier.is_closed();
                frontier.record_return(producer);
                !was_closed && frontier.is_closed()
            }
            None => false,
        };
        self.reconcile_returned_word_debt(coverage);
        closed
    }

    /// The frontier kept for a range, if one was scheduled.
    pub fn frontier_of(&self, coverage: &OccurrenceIdentity) -> Option<&ObservationFrontier> {
        self.frontiers.get(coverage)
    }

    // -- seal ---------------------------------------------------------------

    /// Assemble the seal for one occurrence, or say exactly why it cannot seal.
    pub(super) fn mint_seal(
        &self,
        occurrence: &OccurrenceIdentity,
    ) -> Result<LedgerSealReceipt, SealRefusal> {
        if self.text_recovery_pending(occurrence) {
            return Err(SealRefusal::TextRecoveryPending);
        }
        if self.word_trials_pending(occurrence) || self.has_deferred_anchor_words(occurrence) {
            return Err(SealRefusal::FrontierOpen);
        }
        let serial = self
            .evidence
            .get(occurrence)
            .ok_or(SealRefusal::NotQualified)?;
        if !serial.vad_closed() {
            return Err(SealRefusal::VadDidNotClose);
        }
        let frontier = self
            .frontiers
            .get(occurrence)
            .ok_or(SealRefusal::FrontierUnknown)?;
        if !frontier.is_closed() {
            return Err(SealRefusal::FrontierOpen);
        }
        if self
            .text_of(occurrence)
            .is_none_or(|text| text.trim().is_empty())
        {
            return Err(SealRefusal::LabelMissing);
        }
        let owner = self.owner_history.get(occurrence);
        let answered = owner.map_or(0, |owner| owner.answered);
        let ordinals = owner.map_or_else(Vec::new, |owner| owner.trail_ordinals.clone());
        if ordinals.len() != answered {
            return Err(SealRefusal::ObservationsWithoutReceipts);
        }
        Ok(LedgerSealReceipt {
            word_finality: self.word_finality(occurrence),
            receipt_id: Self::seal_id(LedgerSealScope::Occurrence, occurrence),
            scope: LedgerSealScope::Occurrence,
            coverage: occurrence.clone(),
            sealed_occurrences: vec![occurrence.clone()],
            serials: vec![serial.clone()],
            vad_close_sample: serial.vad_close_sample.unwrap_or(occurrence.sample_end),
            frontier: frontier.clone(),
            layer_trail_ordinals: ordinals,
        })
    }

    /// Seal one occurrence. Idempotent: an already-sealed range returns the
    /// receipt it was sealed with, never a fresher one.
    pub fn seal(
        &mut self,
        occurrence: &OccurrenceIdentity,
    ) -> Result<&LedgerSealReceipt, SealRefusal> {
        if !self.seals.contains_key(occurrence) {
            let receipt = self.mint_seal(occurrence)?;
            crate::pipeline::trail::record(
                occurrence,
                crate::pipeline::trail::TrailEvent::OccurrenceSealed {
                    occurrence: occurrence.clone(),
                    receipt_id: receipt.receipt_id.clone(),
                    word_finality: receipt.word_finality.clone(),
                },
            );
            self.seals.insert(occurrence.clone(), receipt);
            self.unsealed_qualified.remove(occurrence);
            crate::pipeline::occurrence_slot_receipt::emit(self, occurrence);
            self.close_word_components(occurrence);
        }
        self.seals
            .get(occurrence)
            .ok_or(SealRefusal::ObservationsWithoutReceipts)
    }

    /// Seal a whole session/capture epoch once every occurrence in it is sealed.
    ///
    /// The terminal seal is a statement about the same kind of thing an
    /// occurrence seal is — a coverage — so it carries the same receipt shape
    /// rather than minting a second finality vocabulary.
    pub fn seal_terminal(
        &mut self,
        session: &str,
        capture_epoch: u64,
    ) -> Result<LedgerSealReceipt, SealRefusal> {
        if !self
            .pending_text_recoveries(session, capture_epoch)
            .is_empty()
        {
            return Err(SealRefusal::TextRecoveryPending);
        }
        // Two distinct refusals, both terminal-blocking: measured uncovered
        // speech, and no authenticated measurement at all. Absence of evidence
        // may not certify a seal simply because it is not `Incomplete`.
        if self.latest_seal_coverage.as_ref().is_some_and(|coverage| {
            coverage.session_id == session
                && coverage.capture_epoch == capture_epoch
                && (coverage.status == SealCoverageStatus::Incomplete
                    || coverage.status.unavailable_reason().is_some())
        }) {
            return Err(SealRefusal::CoverageIncomplete);
        }
        let in_epoch: Vec<OccurrenceIdentity> = self
            .evidence
            .keys()
            .filter(|occurrence| {
                occurrence.session == session && occurrence.capture_epoch == capture_epoch
            })
            .cloned()
            .collect();
        if in_epoch.is_empty() {
            return Err(SealRefusal::NotQualified);
        }
        for occurrence in &in_epoch {
            self.seal(occurrence)?;
        }
        let mut serials = Vec::with_capacity(in_epoch.len());
        let mut ordinals = Vec::new();
        let mut vad_close = 0u64;
        for occurrence in &in_epoch {
            let seal = self
                .seals
                .get(occurrence)
                .ok_or(SealRefusal::OccurrenceStillOpen)?;
            serials.extend(seal.serials.iter().cloned());
            ordinals.extend(seal.layer_trail_ordinals.iter().copied());
            vad_close = vad_close.max(seal.vad_close_sample);
        }
        let first = in_epoch.first().expect("non-empty epoch");
        let last = in_epoch.last().expect("non-empty epoch");
        let coverage =
            OccurrenceIdentity::new(session, capture_epoch, first.sample_start, last.sample_end);
        // The terminal frontier schedules nobody: closure here is a restatement
        // of the constituent occurrence frontiers, which were each checked in
        // `seal` above. It is not independent evidence and must not be read as
        // any.
        let frontier = ObservationFrontier::scheduled(coverage.clone(), Vec::new());
        let receipt = LedgerSealReceipt {
            word_finality: in_epoch
                .iter()
                .filter_map(|owner| self.seals.get(owner))
                .flat_map(|seal| seal.word_finality.clone())
                .collect(),
            receipt_id: Self::seal_id(LedgerSealScope::Terminal, &coverage),
            scope: LedgerSealScope::Terminal,
            coverage,
            sealed_occurrences: in_epoch,
            serials,
            vad_close_sample: vad_close,
            frontier,
            layer_trail_ordinals: ordinals,
        };
        if !self.terminal_seals.contains(&receipt) {
            self.terminal_seals.push(receipt.clone());
            if self.transcript_seal_timestamp_ms.is_none() {
                self.transcript_seal_timestamp_ms = unix_epoch_millis();
            }
        }
        Ok(receipt)
    }

    /// Compare the entire carrier with ledger-owned finality, never just its ID.
    pub fn authenticates_seal(&self, receipt: &LedgerSealReceipt) -> bool {
        match receipt.scope {
            LedgerSealScope::Occurrence => self.seal_of(&receipt.coverage) == Some(receipt),
            LedgerSealScope::Terminal => self.terminal_seals.contains(receipt),
        }
    }

    /// Read-only lookup of an already minted scope-bearing receipt.
    pub fn seal_receipt(&self, id: &str) -> Option<&LedgerSealReceipt> {
        self.seals
            .values()
            .chain(self.terminal_seals.iter())
            .find(|seal| seal.receipt_id == id)
    }

    /// A projected finality reference must cover this exact occurrence.
    pub fn authenticates_seal_reference(&self, occurrence: &OccurrenceIdentity, id: &str) -> bool {
        self.seal_of(occurrence)
            .is_some_and(|seal| seal.receipt_id == id)
            || self
                .terminal_seals
                .iter()
                .any(|seal| seal.receipt_id == id && seal.sealed_occurrences.contains(occurrence))
    }

    /// The seal held for one occurrence, if it is sealed.
    pub fn seal_of(&self, occurrence: &OccurrenceIdentity) -> Option<&LedgerSealReceipt> {
        self.seals.get(occurrence)
    }

    /// Whether the occurrence is final.
    pub fn is_sealed(&self, occurrence: &OccurrenceIdentity) -> bool {
        self.seals.contains_key(occurrence)
    }

    /// Deterministic seal identifier for a coverage.
    pub(super) fn seal_id(scope: LedgerSealScope, coverage: &OccurrenceIdentity) -> String {
        format!(
            "seal-{}-{}-{}-{}-{}",
            scope.as_str(),
            coverage.session,
            coverage.capture_epoch,
            coverage.sample_start,
            coverage.sample_end
        )
    }
}
