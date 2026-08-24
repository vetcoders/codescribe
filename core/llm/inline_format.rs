//! Automatic post-ASR label author — W1-D structural part set.
//!
//! Throne law for this module:
//! - Exactly one automatic label author exists: the chained Responses
//!   `inline_format` path represented by [`OccurrenceLabelProposal`].
//! - Proposals bind to already-grounded occurrence coordinates. Text is
//!   payload only and is never a key.
//! - This author cannot create, merge, or delete occurrence IDs.
//! - Light+ is not an author. Only explicitly retained lexicon constraints
//!   may accompany a proposal as non-authoritative input.
//! - Whole-session Final BAM and document assembly (`SessionStore`) do not
//!   return here.
//!
//! W1 places the part and leaves it unwired. W2 alone may connect proposals
//! into the reducer admission path. ASR, controller, clipboard, Agent, canvas,
//! and Swift consumers are intentionally absent from this file.

/// Non-authoritative lexicon constraint retained after Light+ authorship was
/// removed. Constraint input only — never mints occurrences and never selects
/// a delivery destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedLexiconConstraint {
    /// Observed surface form the lexicon may rewrite.
    pub variant: String,
    /// Canonical spelling the constraint prefers.
    pub canonical: String,
}

/// Disposition of one automatic label proposal for layer-decision history.
///
/// The ledger records the decision; this author only proposes. It does not
/// seal, reduce, or deliver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelProposalDisposition {
    /// Offer a new label for an existing occurrence.
    Propose,
    /// Refuse to alter the existing label (guard / fail-closed).
    Refuse,
    /// Keep the current authorized label unchanged.
    PreserveExisting,
}

/// Sole automatic post-ASR label proposal part.
///
/// # Inputs
/// - Occurrence coordinates already admitted by `AcousticLedger`
///   (`session`, `capture_epoch`, `sample_start`, `sample_end`).
/// - Current candidate label (payload, never identity).
/// - Optional [`RetainedLexiconConstraint`] values.
/// - Optional Responses `previous_response_id` chain tip.
///
/// # Outputs
/// - A proposed label plus [`LabelProposalDisposition`] for later ledger /
///   reducer admission (W2).
///
/// # Forbidden authority
/// - Creating or deleting occurrence IDs.
/// - Owning a transcript document / `SessionStore`.
/// - Selecting a delivery destination (that is `DeliveryRoute`).
/// - Whole-session Final BAM rewriting.
///
/// # Intended W2 consumers
/// - `app/presentation/emitter.rs` (reducer admission of proposals).
///
/// # Must not reach
/// - `app/controller/delivery_route.rs` (route cannot choose text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OccurrenceLabelProposal {
    /// Capture session owning the occurrence.
    pub session: String,
    /// Capture epoch; sample clocks restart across epochs.
    pub capture_epoch: u64,
    /// First sample of the already-grounded occurrence.
    pub sample_start: u64,
    /// One past the last sample of the already-grounded occurrence.
    pub sample_end: u64,
    /// Proposed textual label. Payload only — never an identity key.
    pub proposed_label: String,
    /// Retained lexicon constraints that informed the proposal, if any.
    pub lexicon_constraints: Vec<RetainedLexiconConstraint>,
    /// Chained Responses tip for the inline_format path; execution is W2+.
    pub previous_response_id: Option<String>,
    /// Typed proposal disposition for per-layer decision history.
    pub disposition: LabelProposalDisposition,
}

impl OccurrenceLabelProposal {
    /// Construct a proposal bound to existing occurrence coordinates.
    ///
    /// Coordinates are taken as already admitted. This constructor never
    /// allocates a new occurrence identity and never interprets text as a key.
    pub fn for_existing_occurrence(
        session: impl Into<String>,
        capture_epoch: u64,
        sample_start: u64,
        sample_end: u64,
        proposed_label: impl Into<String>,
        disposition: LabelProposalDisposition,
    ) -> Self {
        Self {
            session: session.into(),
            capture_epoch,
            sample_start,
            sample_end,
            proposed_label: proposed_label.into(),
            lexicon_constraints: Vec::new(),
            previous_response_id: None,
            disposition,
        }
    }

    /// Attach retained lexicon constraints without granting them authorship.
    pub fn with_lexicon_constraints(mut self, constraints: Vec<RetainedLexiconConstraint>) -> Self {
        self.lexicon_constraints = constraints;
        self
    }

    /// Attach the Responses chain tip used by the inline_format path.
    pub fn with_previous_response_id(mut self, previous_response_id: impl Into<String>) -> Self {
        self.previous_response_id = Some(previous_response_id.into());
        self
    }

    /// Sample length of the bound occurrence; saturating on reversed ranges.
    pub fn sample_len(&self) -> u64 {
        self.sample_end.saturating_sub(self.sample_start)
    }

    /// True when the proposal names a non-empty sample range.
    pub fn binds_real_samples(&self) -> bool {
        self.sample_end > self.sample_start
    }
}
