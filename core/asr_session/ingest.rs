//! Ordering and idempotence for a Layer 1 event stream.
//!
//! A live provider is not a well-behaved iterator. It reconnects and replays,
//! it re-sends a final it is not sure we received, and a slow frame can arrive
//! after the frame that supersedes it. Arrival order is therefore not ordering
//! — the provider's monotonic `sequence_id` is.
//!
//! [`SessionIngest`] is the one place that decision is made, so no downstream
//! consumer has to re-derive it and none of them can disagree. It holds no
//! audio, spawns nothing, and reads no clock: the same event sequence always
//! produces the same verdicts.
//!
//! ## The rules, in order
//!
//! 1. An event for another session is refused outright.
//! 2. A final identical to the one that already sealed its utterance is
//!    **idempotent** — accepted-in-effect, applied once. This is the reconnect
//!    resend, and it may legitimately arrive after newer events.
//! 3. A byte-identical repeat of the last accepted event is likewise idempotent.
//! 4. Anything else at or below the highest accepted sequence is out of order
//!    and refused. Late text must never overwrite newer text.
//! 5. A partial or final aimed at a sealed utterance is refused. A final is a
//!    commitment; reopening it is the replacement this product forbids.
//! 6. Otherwise the event is accepted, and a final seals its utterance.
//!
//! Errors and usage are diagnostics, not text, so they are not blocked by a
//! sealed utterance — only by ordering.

use std::collections::BTreeMap;

use super::events::{AsrSessionEvent, SessionId, TranscriptEvent};

/// What [`SessionIngest::ingest`] decided about one event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestVerdict {
    /// Applied; it advanced the stream.
    Accepted,
    /// Already applied. Re-delivery changed nothing, which is the point.
    DuplicateIdempotent,
    /// At or below the highest accepted sequence, and not a known duplicate.
    RejectedOutOfOrder,
    /// Aimed at an utterance a final already sealed.
    RejectedSealedUtterance,
    /// Belongs to a different session.
    RejectedForeignSession,
}

impl IngestVerdict {
    /// Whether the event joined the accepted stream.
    pub fn is_accepted(&self) -> bool {
        matches!(self, Self::Accepted)
    }

    /// Stable snake_case token for logs and telemetry.
    pub fn as_token(&self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::DuplicateIdempotent => "duplicate_idempotent",
            Self::RejectedOutOfOrder => "rejected_out_of_order",
            Self::RejectedSealedUtterance => "rejected_sealed_utterance",
            Self::RejectedForeignSession => "rejected_foreign_session",
        }
    }
}

/// Monotonic, idempotent ledger for one Layer 1 session.
#[derive(Debug)]
pub struct SessionIngest {
    /// The only session whose events this ledger accepts.
    session_id: SessionId,
    /// Highest accepted sequence, or `None` before the first accepted event.
    last_sequence: Option<u64>,
    /// The last accepted event, for same-sequence duplicate detection.
    last_accepted: Option<AsrSessionEvent>,
    /// The final that sealed each utterance.
    sealed: BTreeMap<u64, TranscriptEvent>,
    /// Accepted events, in accepted order.
    accepted: Vec<AsrSessionEvent>,
    /// How many re-deliveries were absorbed idempotently.
    duplicate_count: u64,
    /// How many events were refused as out of order.
    out_of_order_count: u64,
    /// How many transcript events were refused by a sealed utterance.
    sealed_rejection_count: u64,
    /// How many events were refused as belonging to another session.
    foreign_rejection_count: u64,
}

impl SessionIngest {
    /// Open a ledger bound to one session.
    pub fn new(session_id: SessionId) -> Self {
        Self {
            session_id,
            last_sequence: None,
            last_accepted: None,
            sealed: BTreeMap::new(),
            accepted: Vec::new(),
            duplicate_count: 0,
            out_of_order_count: 0,
            sealed_rejection_count: 0,
            foreign_rejection_count: 0,
        }
    }

    /// Apply one event and report what was decided.
    pub fn ingest(&mut self, event: AsrSessionEvent) -> IngestVerdict {
        let identity = event.identity();

        if identity.session_id() != &self.session_id {
            self.foreign_rejection_count += 1;
            return IngestVerdict::RejectedForeignSession;
        }

        let utterance_id = identity.utterance_id();
        let sequence_id = identity.sequence_id();
        let sealed_final = self.sealed.get(&utterance_id);

        // Rule 2: the reconnect resend. Position in the stream is irrelevant —
        // an identical final says exactly what we already committed.
        if let (AsrSessionEvent::Final(incoming), Some(existing)) = (&event, sealed_final)
            && incoming == existing
        {
            self.duplicate_count += 1;
            return IngestVerdict::DuplicateIdempotent;
        }

        match self.last_sequence {
            Some(last) if sequence_id == last && self.last_accepted.as_ref() == Some(&event) => {
                // Rule 3: same slot, same content — a retransmit, not a change.
                self.duplicate_count += 1;
                return IngestVerdict::DuplicateIdempotent;
            }
            Some(last) if sequence_id <= last => {
                // Rule 4: stale. Applying it would let older text win.
                self.out_of_order_count += 1;
                return IngestVerdict::RejectedOutOfOrder;
            }
            _ => {}
        }

        // Rule 5: a final is a commitment; nothing may reopen it.
        if event.is_transcript() && sealed_final.is_some() {
            self.sealed_rejection_count += 1;
            return IngestVerdict::RejectedSealedUtterance;
        }

        if let AsrSessionEvent::Final(transcript) = &event {
            self.sealed.insert(utterance_id, transcript.clone());
        }
        self.last_sequence = Some(sequence_id);
        self.last_accepted = Some(event.clone());
        self.accepted.push(event);
        IngestVerdict::Accepted
    }

    /// Accepted events, in accepted order.
    pub fn accepted(&self) -> &[AsrSessionEvent] {
        &self.accepted
    }

    /// The final that sealed `utterance_id`, if one has.
    pub fn sealed_final(&self, utterance_id: u64) -> Option<&TranscriptEvent> {
        self.sealed.get(&utterance_id)
    }

    /// Highest accepted sequence, or `None` before the first accepted event.
    pub fn last_sequence(&self) -> Option<u64> {
        self.last_sequence
    }

    /// Re-deliveries absorbed idempotently.
    pub fn duplicate_count(&self) -> u64 {
        self.duplicate_count
    }

    /// Events refused as out of order.
    pub fn out_of_order_count(&self) -> u64 {
        self.out_of_order_count
    }

    /// Transcript events refused by a sealed utterance.
    pub fn sealed_rejection_count(&self) -> u64 {
        self.sealed_rejection_count
    }

    /// Events refused as belonging to another session.
    pub fn foreign_rejection_count(&self) -> u64 {
        self.foreign_rejection_count
    }
}
