//! Live formatter presentation proposals for an existing acoustic range.
//!
//! The transport retains occurrence coordinates and the exact source observation.
//! Formatter results are reducer-owned derived versions; they never grant ledger
//! word mutation authority. The producer returns its same scheduled frontier on
//! every terminal outcome. Missing or stale source evidence refuses publication.

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

/// Disposition of one derived presentation proposal. Returning this task does
/// not create a word observation or mutate an acoustic label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelProposalDisposition {
    /// Offer a derived presentation for an existing range.
    Propose,
    /// Refuse to alter the existing label (guard / fail-closed).
    Refuse,
    /// Keep the current authorized label unchanged.
    PreserveExisting,
}

/// Non-authoritative formatter output under an exact source observation.
/// PresentationEmitter owns admission to derived history and delivery selection.
/// This transport cannot create, change, merge, delete or seal acoustic words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OccurrenceLabelProposal {
    /// Exact source observation captured when the formatter job was scheduled.
    /// Missing provenance permits returning the task, never publishing its text.
    pub source_observation: Option<crate::pipeline::acoustic_ledger::ObservationIdentity>,
    pub source_text: String,
    pub policy: crate::config::FormattingPolicy,
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
    /// Chained Responses tip for a launched inline-format job.
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
            source_observation: None,
            source_text: String::new(),
            policy: crate::config::FormattingPolicy::Smart,
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

    pub fn with_source(
        mut self,
        observation: crate::pipeline::acoustic_ledger::ObservationIdentity,
        text: String,
        policy: crate::config::FormattingPolicy,
    ) -> Self {
        self.source_observation = Some(observation);
        self.source_text = text;
        self.policy = policy;
        self
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
