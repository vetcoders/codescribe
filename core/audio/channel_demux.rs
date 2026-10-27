//! Sample-clock participant demuxer for the shared agent-channel capture.
//!
//! Every open agent channel listens to ONE recognizer, ONE acoustic ledger and
//! ONE archive. What differs per channel is only *when* it listened: each
//! participant owns half-open intervals `[start_sample, end_sample)` on the
//! shared capture clock, which is the count of PCM samples the shared session
//! actually received (blocks dropped to backpressure never advance it, so it
//! is the same clock the ledger stamps on every occurrence).
//!
//! Attribution is deterministic and text-free: an occurrence belongs to a
//! participant when the occurrence's sample midpoint lies inside one of that
//! participant's intervals. A word split by a join or a leave therefore goes to
//! exactly one side, while the decoder itself keeps its full acoustic context.
//! Speech that ended before a join is never backfilled; speech that starts
//! after a leave never reaches the participant that left, even if the same
//! owner joins again later with a new interval.
//!
//! The demux is the shared session's only [`EventSink`]. It forwards each
//! engine event to the participant sinks it belongs to and never rewrites a
//! receipt: the ledger stays the one authority every participant reducer
//! authenticates against.

use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use tokio::sync::Notify;
use tracing::{debug, info};

use super::streaming_recorder::CaptureSubscriberId;
use crate::pipeline::contracts::{EngineEvent, EventSink};

/// How long a detached participant waits for the shared decoder to pass its
/// detach sample before its own intervals are frozen for Send. The shared
/// capture ending releases it at once; so does the first committed
/// occurrence that starts at or after the detach sample.
pub const CHANNEL_DETACH_SETTLE_BUDGET: Duration = Duration::from_secs(10);

/// Exact logical owner of one participation. Two seats of the same logical
/// session share `provider` and `provider_session_id`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChannelParticipantOwner {
    pub provider: String,
    pub provider_session_id: String,
    pub lease_id: String,
    pub channel: String,
}

impl ChannelParticipantOwner {
    /// Delivery identity: one logical session receives one delivery execution,
    /// however many seats (hosts, digits) listen for it. Never the text.
    pub fn delivery_identity(&self) -> (&str, &str) {
        (&self.provider, &self.provider_session_id)
    }
}

/// Whether a participant's rows execute delivery or only project history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliverySeat {
    /// First seat of its logical session in this capture: executes delivery.
    Primary,
    /// Another seat of a logical session that already has a primary seat.
    Projection,
}

impl DeliverySeat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Projection => "projection",
        }
    }
}

/// One closed or open participation on the shared capture clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParticipationInterval {
    pub participant: CaptureSubscriberId,
    pub owner: Option<ChannelParticipantOwner>,
    pub start_sample: u64,
    /// `None` while the participant is still attached.
    pub end_sample: Option<u64>,
    pub seat: DeliverySeat,
}

/// Why a detached participant's settlement wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetachSettlement {
    /// The decoder committed an occurrence starting at or after the detach.
    DecoderPassed,
    /// The shared capture ended; every event it will ever emit was delivered.
    CaptureEnded,
    /// The budget elapsed first; the participant freezes what it holds.
    BudgetElapsed,
    /// The participant was unknown or already settled.
    Unknown,
}

/// Deterministic, text-free attribution of `[sample_start, sample_end)` to
/// `[interval_start, interval_end)`: the occurrence's midpoint decides.
pub fn occurrence_in_interval(
    sample_start: u64,
    sample_end: u64,
    interval_start: u64,
    interval_end: Option<u64>,
) -> bool {
    let anchor = sample_start + sample_end.saturating_sub(sample_start) / 2;
    anchor >= interval_start && interval_end.is_none_or(|end| anchor < end)
}

struct Participant {
    id: CaptureSubscriberId,
    sink: Arc<dyn EventSink>,
    start_sample: u64,
    /// Set at detach; events are then routed only by range.
    end_sample: Option<u64>,
    fence_passed: bool,
}

impl Participant {
    fn owns(&self, sample_start: u64, sample_end: u64) -> bool {
        occurrence_in_interval(sample_start, sample_end, self.start_sample, self.end_sample)
    }
}

#[derive(Default)]
struct DemuxState {
    opened: Option<(String, u64)>,
    participants: Vec<Participant>,
    history: Vec<ParticipationInterval>,
    /// Largest `sample_start` of a committed occurrence the decoder emitted.
    decoder_frontier: Option<u64>,
    capture_ended: bool,
}

/// How an engine event is routed to participants.
enum Route {
    /// Owned by the participants whose interval holds this range.
    Range(u64, u64),
    /// Ledger finality and capture-end receipts: every unsettled participant.
    /// Each reducer refuses a seal naming occurrences outside its document.
    Settlement,
    /// Live state of the shared capture: attached participants only.
    Attached,
}

fn route_of(event: &EngineEvent) -> Route {
    match event {
        EngineEvent::LedgerMutation { observation, .. }
        | EngineEvent::DirectTranscript { observation, .. } => Route::Range(
            observation.occurrence.sample_start,
            observation.occurrence.sample_end,
        ),
        EngineEvent::OccurrenceLabelProposal { proposal } => {
            Route::Range(proposal.sample_start, proposal.sample_end)
        }
        EngineEvent::Preview { pin, .. } => {
            Route::Range(pin.range.sample_start, pin.range.sample_end)
        }
        EngineEvent::SidebandEvidence { evidence } => {
            Route::Range(evidence.range.sample_start, evidence.range.sample_end)
        }
        EngineEvent::LedgerSeal { .. }
        | EngineEvent::SealCoverage { .. }
        | EngineEvent::DirectTranscriptEnd { .. }
        | EngineEvent::SessionFinalised { .. }
        | EngineEvent::Stats { .. } => Route::Settlement,
        _ => Route::Attached,
    }
}

/// The shared capture's one event sink and the owner of every participation.
pub struct ChannelDemux {
    capture_session_id: String,
    state: StdMutex<DemuxState>,
    settled: Notify,
}

impl std::fmt::Debug for ChannelDemux {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChannelDemux")
            .field("capture_session_id", &self.capture_session_id)
            .finish_non_exhaustive()
    }
}

impl ChannelDemux {
    pub fn new(capture_session_id: String) -> Arc<Self> {
        Arc::new(Self {
            capture_session_id,
            state: StdMutex::new(DemuxState::default()),
            settled: Notify::new(),
        })
    }

    /// Ledger namespace every participant reducer authenticates against.
    pub fn capture_session_id(&self) -> &str {
        &self.capture_session_id
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, DemuxState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// The seat a participation of `owner` would take now: the first
    /// attached seat of a logical session executes delivery, later ones
    /// only project history.
    pub fn seat_for(&self, owner: Option<&ChannelParticipantOwner>) -> DeliverySeat {
        Self::seat_in(&self.lock(), owner)
    }

    fn seat_in(state: &DemuxState, owner: Option<&ChannelParticipantOwner>) -> DeliverySeat {
        let Some(owner) = owner else {
            return DeliverySeat::Primary;
        };
        let held = state.history.iter().any(|interval| {
            interval.end_sample.is_none()
                && interval.seat == DeliverySeat::Primary
                && interval
                    .owner
                    .as_ref()
                    .is_some_and(|held| held.delivery_identity() == owner.delivery_identity())
        });
        if held {
            DeliverySeat::Projection
        } else {
            DeliverySeat::Primary
        }
    }

    /// Start one participation at `start_sample`. A participant joining an
    /// opened capture is bound to it before any later event reaches it.
    pub fn attach(
        &self,
        id: CaptureSubscriberId,
        owner: Option<ChannelParticipantOwner>,
        sink: Arc<dyn EventSink>,
        start_sample: u64,
    ) -> DeliverySeat {
        let mut state = self.lock();
        let seat = Self::seat_in(&state, owner.as_ref());
        if let Some((session, epoch)) = state.opened.clone() {
            sink.on_capture_opened(&session, epoch);
        }
        info!(
            ?id,
            start_sample,
            seat = seat.as_str(),
            capture = %self.capture_session_id,
            "channel participant attached to the shared capture"
        );
        state.history.push(ParticipationInterval {
            participant: id,
            owner,
            start_sample,
            end_sample: None,
            seat,
        });
        state.participants.push(Participant {
            id,
            sink,
            start_sample,
            end_sample: None,
            fence_passed: false,
        });
        seat
    }

    /// Close this participant's interval at `end_sample`. Later speech never
    /// reaches it; occurrences inside its interval still do until it settles.
    pub fn detach(&self, id: CaptureSubscriberId, end_sample: u64) {
        let mut state = self.lock();
        let frontier = state.decoder_frontier;
        let ended = state.capture_ended;
        if let Some(participant) = state
            .participants
            .iter_mut()
            .find(|participant| participant.id == id && participant.end_sample.is_none())
        {
            participant.end_sample = Some(end_sample.max(participant.start_sample));
            participant.fence_passed =
                ended || frontier.is_some_and(|frontier| frontier >= end_sample);
        }
        if let Some(interval) = state
            .history
            .iter_mut()
            .rev()
            .find(|interval| interval.participant == id && interval.end_sample.is_none())
        {
            interval.end_sample = Some(end_sample.max(interval.start_sample));
        }
        info!(
            ?id,
            end_sample, "channel participant detached from the shared capture"
        );
    }

    /// The shared session joined: every event it emitted has been delivered.
    pub fn finish_capture(&self) {
        let mut state = self.lock();
        state.capture_ended = true;
        for participant in &mut state.participants {
            participant.fence_passed = true;
        }
        drop(state);
        self.settled.notify_waiters();
    }

    /// Wait until this detached participant's interval is settled, then stop
    /// routing to it. Its reducer then holds exactly its own occurrences.
    pub async fn settle(&self, id: CaptureSubscriberId, budget: Duration) -> DetachSettlement {
        let deadline = tokio::time::Instant::now() + budget;
        let outcome = loop {
            let notified = self.settled.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let state = self.lock();
                let Some(participant) = state.participants.iter().find(|p| p.id == id) else {
                    return DetachSettlement::Unknown;
                };
                if participant.fence_passed {
                    break if state.capture_ended {
                        DetachSettlement::CaptureEnded
                    } else {
                        DetachSettlement::DecoderPassed
                    };
                }
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                break DetachSettlement::BudgetElapsed;
            }
        };
        self.lock()
            .participants
            .retain(|participant| participant.id != id);
        debug!(?id, ?outcome, "channel participant settled");
        outcome
    }

    /// Every participation this capture has seen, in attach order.
    pub fn intervals(&self) -> Vec<ParticipationInterval> {
        self.lock().history.clone()
    }

    /// Participants still attached (interval open).
    pub fn attached_count(&self) -> usize {
        self.lock()
            .participants
            .iter()
            .filter(|participant| participant.end_sample.is_none())
            .count()
    }

    fn deliver(state: &mut DemuxState, event: &EngineEvent) {
        match route_of(event) {
            Route::Range(start, end) => {
                if matches!(
                    event,
                    EngineEvent::LedgerMutation { .. } | EngineEvent::DirectTranscript { .. }
                ) {
                    state.decoder_frontier =
                        Some(state.decoder_frontier.map_or(start, |f| f.max(start)));
                }
                for participant in state.participants.iter().filter(|p| p.owns(start, end)) {
                    participant.sink.on_event(event);
                }
            }
            Route::Settlement => {
                for participant in &state.participants {
                    participant.sink.on_event(event);
                }
            }
            Route::Attached => {
                for participant in state.participants.iter().filter(|p| p.end_sample.is_none()) {
                    participant.sink.on_event(event);
                }
            }
        }
    }

    /// Apple's held-word mirror, cut to the words this participant owns.
    fn deliver_unadmitted(state: &DemuxState, event: &EngineEvent) {
        let EngineEvent::UnadmittedAppleWords {
            revision,
            words,
            closed_phrases,
        } = event
        else {
            return;
        };
        for participant in &state.participants {
            let owned: Vec<_> = words
                .iter()
                .filter(|word| participant.owns(word.sample_start, word.sample_end))
                .cloned()
                .collect();
            participant
                .sink
                .on_event(&EngineEvent::UnadmittedAppleWords {
                    revision: *revision,
                    words: owned,
                    closed_phrases: closed_phrases.clone(),
                });
        }
    }
}

impl EventSink for ChannelDemux {
    fn on_capture_opened(&self, session_id: &str, capture_epoch: u64) {
        let mut state = self.lock();
        if state.opened.is_some() {
            return;
        }
        state.opened = Some((session_id.to_owned(), capture_epoch));
        for participant in &state.participants {
            participant
                .sink
                .on_capture_opened(session_id, capture_epoch);
        }
    }

    fn on_formatter_pending(&self, pending: bool) {
        for participant in &self.lock().participants {
            participant.sink.on_formatter_pending(pending);
        }
    }

    fn on_event(&self, event: &EngineEvent) {
        let mut state = self.lock();
        if matches!(event, EngineEvent::UnadmittedAppleWords { .. }) {
            Self::deliver_unadmitted(&state, event);
            return;
        }
        let before = state.decoder_frontier;
        Self::deliver(&mut state, event);
        let frontier = state.decoder_frontier;
        if frontier == before {
            return;
        }
        let mut released = false;
        for participant in &mut state.participants {
            if !participant.fence_passed
                && participant
                    .end_sample
                    .is_some_and(|end| frontier.is_some_and(|frontier| frontier >= end))
            {
                participant.fence_passed = true;
                released = true;
            }
        }
        drop(state);
        if released {
            self.settled.notify_waiters();
        }
    }
}
