//! Recorder-side Layer 1 orchestration: the lane a live session drives.
//!
//! This is the C1 seam between audio capture and a Layer 1 refiner. The
//! recorder/session pipeline owns capture and the Apple canvas; this lane owns
//! everything a Layer 1 provider is *allowed* to do while a recording runs:
//!
//! - **Injected authority.** The lane never constructs a provider. It receives
//!   a [`Layer1Decision`] — an already-authorized, typed decision made by the
//!   consent/settings owner. The decision distinguishes Apple-only, local
//!   exact-span Whisper, and a generic injected provider.
//! - **Bounded, non-blocking fan-out.** [`RecorderLayer1Lane::offer_pcm`]
//!   returns immediately on every call. A refiner that cannot keep up costs
//!   refinement frames, never capture: sustained overflow degrades the lane to
//!   canvas + lexicon instead of ever exerting backpressure on audio.
//! - **Partials are volatile draft.** They live in the lane, are replaced
//!   freely, and die with the lane. Nothing here can commit a partial to the
//!   canvas.
//! - **Finals go through the doctrine seam.** Every final is vetted by
//!   [`SessionIngest`] (ordering, idempotence, sealed utterances) and the
//!   session outcome routes through [`crate::quality::merge_live_layer1`]. A
//!   generic full-session candidate remains evidence until it has exact span
//!   identity; the Apple path owns the bounded rewrite fence.
//! - **Every failure lands on Apple + lexicon.** Overflow, disconnect,
//!   sleep/wake, and an incomplete stop-drain all degrade to
//!   [`RefinerMode::Off`]. Nothing in this module can reach local Whisper —
//!   there is no import edge to `crate::stt`, and the fleet witness measures
//!   the init counters to keep it that way.
//!
//! ## Degrade drops, stop closes
//!
//! [`AsrSessionProvider::close`] is bounded but may block briefly (the cloud
//! session drains its socket tail). Degradation happens on the live session
//! loop, where even a bounded stall would hold up canvas event drainage — so
//! a degrading lane *drops* its provider (the cloud transport aborts its actor
//! on drop) and only the deliberate stop path pays for a graceful close and
//! trailing-event drain.

use std::collections::BTreeMap;
use std::fmt;

use tracing::{info, warn};

use super::events::{AsrErrorKind, AsrSessionEvent, TranscriptEvent};
use super::ingest::{IngestVerdict, SessionIngest};
use super::provider::{AsrSessionProvider, RefinerMode, SessionInput};
use crate::quality::{Layer1MergedDelivery, merge_live_layer1};

/// Consecutive overflowed frames tolerated before the lane degrades.
///
/// A single full queue is a hiccup and costs one refinement frame. A run of
/// them means the provider is not consuming; continuing to offer audio would
/// only burn CPU converting frames nobody reads. At the expected 200 ms frame
/// cadence this limit degrades after roughly 1.6 s of sustained overflow.
pub const OVERFLOW_DEGRADE_LIMIT: u32 = 8;

/// Maximum post-close drain iterations before the stop path stops waiting.
///
/// Each iteration consumes one non-empty [`AsrSessionProvider::drain`] batch.
/// The bound is iterations, not wall time, so tests need no clocks and a
/// misbehaving provider cannot hold the stop path hostage.
pub const STOP_DRAIN_MAX_POLLS: u32 = 32;

/// `EngineEvent::Warning` code emitted when the live Layer 1 lane degrades.
///
/// The message carries only the typed reason token — never transcript, audio,
/// or provider payload content.
pub const LAYER1_DEGRADED_WARNING_CODE: &str = "layer1_lane_degraded";

/// Host lifecycle boundary delivered to the active recording session.
///
/// This channel is deliberately per recording. A sleep/wake notification must
/// never create a recorder, retry a provider, or affect a later session that
/// did not cross the boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecorderLifecycleEvent {
    /// The host is about to sleep or has just resumed.
    SleepWake,
}

/// O(1) sender retained by the recording owner while one session is active.
#[derive(Debug, Clone)]
pub struct RecorderLifecycleHandle {
    sender: tokio::sync::mpsc::UnboundedSender<RecorderLifecycleEvent>,
}

impl RecorderLifecycleHandle {
    /// Notify the active session of a sleep/wake boundary.
    ///
    /// Returns false only when the session has already gone away. Sending does
    /// no model, disk, network, formatting, or transcript work.
    pub fn note_sleep_wake(&self) -> bool {
        self.sender.send(RecorderLifecycleEvent::SleepWake).is_ok()
    }
}

/// Receive side owned exclusively by the live transcription task.
#[derive(Debug)]
pub struct RecorderLifecycleEvents {
    receiver: tokio::sync::mpsc::UnboundedReceiver<RecorderLifecycleEvent>,
}

impl RecorderLifecycleEvents {
    /// Wait for the next host lifecycle boundary.
    pub async fn recv(&mut self) -> Option<RecorderLifecycleEvent> {
        self.receiver.recv().await
    }
}

/// Create the per-recording lifecycle adapter shared by recorder and session.
pub fn recorder_lifecycle_channel() -> (RecorderLifecycleHandle, RecorderLifecycleEvents) {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    (
        RecorderLifecycleHandle { sender },
        RecorderLifecycleEvents { receiver },
    )
}

/// The injected, already-authorized Layer 1 decision a recording starts with.
///
/// Construction and consent are deliberately *not* this module's business: the
/// settings/consent owner builds the provider and hands the finished decision
/// in. A recording that receives [`Self::Disarmed`] is the normal product —
/// not an error, and never a trigger for loading anything heavier.
pub enum Layer1Decision {
    /// No Layer 1 refiner for this recording. Canvas plus lexicon, complete.
    Disarmed,
    /// Local Whisper owns bounded, PCM-identified tail patches for this
    /// recording. This is deliberately a recording-start decision, not a
    /// second environment read inside the Apple session.
    LocalTailPatch(LocalTailPatchDisposition),
    /// An already-authorized provider, ready to open.
    Armed(Box<dyn AsrSessionProvider + Send>),
}

impl Layer1Decision {
    /// Whether this decision carries a provider.
    pub fn is_armed(&self) -> bool {
        matches!(
            self,
            Self::Armed(_)
                | Self::LocalTailPatch(
                    LocalTailPatchDisposition::ArmedDefault
                        | LocalTailPatchDisposition::ArmedPhase(_)
                )
        )
    }

    /// Whether the generic provider fan-out lane (Cloud) is armed.
    pub fn is_provider_armed(&self) -> bool {
        matches!(self, Self::Armed(_))
    }

    /// Recording-start local tail-patch disposition, when local power was the
    /// selected product mode.
    pub fn local_tail_patch_disposition(&self) -> Option<LocalTailPatchDisposition> {
        match self {
            Self::LocalTailPatch(disposition) => Some(*disposition),
            Self::Disarmed | Self::Armed(_) => None,
        }
    }
}

/// Why the local Whisper tail-patch lane is armed or degraded for one take.
///
/// This is intentionally distinct from the generic provider lane: Cloud owns
/// provider fan-out, while Local power owns exact-span Whisper jobs. Both are
/// resolved once at recording start and carried through [`Layer1Decision`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalTailPatchDisposition {
    /// The product mode does not request local Whisper.
    NotApplicable,
    /// Local power's product default: Apple live plus local Whisper patches.
    ArmedDefault,
    /// Explicit `phase1`..`phase4` compatibility token armed the same lane.
    ArmedPhase(u8),
    /// Local power was selected but an explicit hard-off token disabled its
    /// required patcher. This is degraded, never a healthy Apple-only state.
    DegradedExplicitOff,
    /// Local power was selected but the override token was not understood.
    DegradedInvalidOverride,
}

impl LocalTailPatchDisposition {
    /// Whether the Apple session must construct the local tail-patch lane.
    pub fn is_armed(self) -> bool {
        matches!(self, Self::ArmedDefault | Self::ArmedPhase(_))
    }

    /// Stable content-free token for logs and receipts.
    pub fn as_token(self) -> &'static str {
        match self {
            Self::NotApplicable => "not_applicable",
            Self::ArmedDefault => "armed_default",
            Self::ArmedPhase(_) => "armed_phase",
            Self::DegradedExplicitOff => "degraded_explicit_off",
            Self::DegradedInvalidOverride => "degraded_invalid_override",
        }
    }
}

impl Default for Layer1Decision {
    /// Safe fallback decision: no Layer 1.
    fn default() -> Self {
        Self::Disarmed
    }
}

impl fmt::Debug for Layer1Decision {
    /// Counts-only debug: the provider itself is never printed.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disarmed => f.write_str("Layer1Decision::Disarmed"),
            Self::LocalTailPatch(disposition) => f
                .debug_struct("Layer1Decision::LocalTailPatch")
                .field("disposition", disposition)
                .finish(),
            Self::Armed(provider) => f
                .debug_struct("Layer1Decision::Armed")
                .field("mode", &provider.mode().as_token())
                .finish(),
        }
    }
}

/// Why the lane fell back to canvas + lexicon. Typed, content-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer1DegradeReason {
    /// The provider refused to open.
    OpenFailed(AsrErrorKind),
    /// Sustained fan-out overflow — the provider stopped consuming.
    Overflow,
    /// The provider reported or exhibited a session-fatal fault.
    Disconnect(AsrErrorKind),
    /// The host slept mid-recording; the session is presumed stale.
    SleepWake,
    /// Stop-drain hit its iteration bound before the provider went quiet.
    StopDrainIncomplete,
}

impl Layer1DegradeReason {
    /// Stable snake_case token for logs and telemetry.
    pub fn as_token(&self) -> &'static str {
        match self {
            Self::OpenFailed(_) => "open_failed",
            Self::Overflow => "overflow",
            Self::Disconnect(_) => "disconnect",
            Self::SleepWake => "sleep_wake",
            Self::StopDrainIncomplete => "stop_drain_incomplete",
        }
    }
}

/// Where the lane is in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer1LaneState {
    /// Opened with [`Layer1Decision::Disarmed`] — normal Apple + lexicon.
    Unarmed,
    /// Provider session open and consuming fan-out.
    Live,
    /// Layer 1 is gone for this recording; canvas + lexicon carry it.
    Degraded(Layer1DegradeReason),
    /// The recording stopped and the lane completed its bounded drain.
    Stopped,
}

/// What one fan-out offer did. Informational — capture never branches on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FanOutVerdict {
    /// The frame reached the provider.
    Forwarded,
    /// The provider's queue was full; the frame was dropped, capture continues.
    DroppedOverflow,
    /// The lane is not live; the frame was ignored.
    Inactive,
}

/// Content-free lane counters, reported at session end.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Layer1LaneTelemetry {
    /// Frames capture offered to the lane (including while not live).
    pub frames_offered: u64,
    /// Frames actually forwarded to the provider.
    pub frames_forwarded: u64,
    /// Frames dropped because the provider queue was full.
    pub overflow_frame_drops: u64,
    /// Partials applied to the volatile draft.
    pub partials_applied: u64,
    /// Finals accepted by the ingest ledger.
    pub finals_accepted: u64,
    /// Events the ingest ledger refused (out of order, sealed, foreign).
    pub events_rejected: u64,
    /// Typed provider error events observed.
    pub provider_errors: u64,
}

/// Everything the lane knows once the recording is over.
#[derive(Debug)]
pub struct Layer1SessionOutcome {
    /// Doctrine-vetted finals, in accepted order.
    finals: Vec<TranscriptEvent>,
    /// Content-free counters for the session log.
    telemetry: Layer1LaneTelemetry,
    /// Why the lane degraded, when it did.
    degrade: Option<Layer1DegradeReason>,
}

impl Layer1SessionOutcome {
    /// Doctrine-vetted finals, in accepted order.
    pub fn finals(&self) -> &[TranscriptEvent] {
        &self.finals
    }

    /// Content-free counters for the session log.
    pub fn telemetry(&self) -> Layer1LaneTelemetry {
        self.telemetry
    }

    /// Why the lane degraded, when it did.
    pub fn degrade_reason(&self) -> Option<Layer1DegradeReason> {
        self.degrade
    }

    /// The refiner's transcript candidate: sealed finals joined in order.
    ///
    /// `None` when the session produced no accepted finals — the caller keeps
    /// the canvas untouched rather than merging against an empty candidate.
    pub fn refined_transcript(&self) -> Option<String> {
        if self.finals.is_empty() {
            return None;
        }
        Some(
            self.finals
                .iter()
                .map(|event| event.text.trim())
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join(" "),
        )
    }

    /// Route the outcome through the integrated doctrine-safe truth seam.
    ///
    /// This is [`merge_live_layer1`]: the committed live floor is immutable,
    /// Layer 1 text may fill aligned gaps and extend the tail, and a
    /// substitution always keeps the live token. Callers deliver
    /// [`Layer1MergedDelivery::text`]; they never deliver the raw candidate.
    pub fn adjudicate_against_live_floor(&self, live_floor: &str) -> Layer1MergedDelivery {
        let candidate = self.refined_transcript();
        merge_live_layer1(live_floor, candidate.as_deref().unwrap_or(""))
    }
}

/// The per-recording Layer 1 lane: open at start, fan out, drain at stop.
///
/// Owned by the live session loop. Every method is non-blocking except
/// [`Self::stop`], whose blocking is bounded by the provider's own close
/// contract plus [`STOP_DRAIN_MAX_POLLS`] drain iterations.
pub struct RecorderLayer1Lane {
    /// Lifecycle position.
    state: Layer1LaneState,
    /// The open provider while [`Layer1LaneState::Live`].
    provider: Option<Box<dyn AsrSessionProvider + Send>>,
    /// The doctrine ledger every provider event passes through.
    ingest: SessionIngest,
    /// Volatile partial text per open utterance. Never canvas; dies on degrade.
    draft: BTreeMap<u64, String>,
    /// Accepted finals in accepted order.
    finals: Vec<TranscriptEvent>,
    /// Content-free counters.
    telemetry: Layer1LaneTelemetry,
    /// Current run of consecutive overflowed frames.
    consecutive_overflows: u32,
    /// Sticky first degrade reason for the outcome record.
    degrade: Option<Layer1DegradeReason>,
    /// One-shot notice so the session can emit a single degrade warning event.
    degrade_notice: Option<Layer1DegradeReason>,
}

impl fmt::Debug for RecorderLayer1Lane {
    /// Counts-only debug shape; the provider is summarized by mode token.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecorderLayer1Lane")
            .field("state", &self.state)
            .field(
                "provider_mode",
                &self.provider.as_ref().map(|p| p.mode().as_token()),
            )
            .field("draft_utterances", &self.draft.len())
            .field("finals", &self.finals.len())
            .field("telemetry", &self.telemetry)
            .finish()
    }
}

impl RecorderLayer1Lane {
    /// Open the lane at recording start. Never fails.
    ///
    /// A provider whose `open` fails is dropped on the spot and the lane
    /// starts degraded — the recording proceeds on canvas + lexicon exactly as
    /// if no provider had been injected.
    pub fn open(decision: Layer1Decision, input: &SessionInput) -> Self {
        let mut lane = Self {
            state: Layer1LaneState::Unarmed,
            provider: None,
            ingest: SessionIngest::new(input.session_id.clone()),
            draft: BTreeMap::new(),
            finals: Vec::new(),
            telemetry: Layer1LaneTelemetry::default(),
            consecutive_overflows: 0,
            degrade: None,
            degrade_notice: None,
        };
        match decision {
            Layer1Decision::Disarmed | Layer1Decision::LocalTailPatch(_) => lane,
            Layer1Decision::Armed(mut provider) => {
                match provider.open(input) {
                    Ok(()) => {
                        info!(
                            refiner = provider.mode().as_token(),
                            sample_rate = input.sample_rate,
                            "Layer 1 lane opened at recording start"
                        );
                        lane.state = Layer1LaneState::Live;
                        lane.provider = Some(provider);
                    }
                    Err(kind) => {
                        // The provider is dropped here; a failed open must not
                        // hold a half-connected session for the whole hold.
                        lane.degrade_dropping_provider(Layer1DegradeReason::OpenFailed(kind));
                    }
                }
                lane
            }
        }
    }

    /// Lifecycle position.
    pub fn state(&self) -> Layer1LaneState {
        self.state
    }

    /// Whether a provider session is currently consuming fan-out.
    pub fn is_live(&self) -> bool {
        matches!(self.state, Layer1LaneState::Live)
    }

    /// The refiner mode currently in effect.
    ///
    /// Anything other than [`Layer1LaneState::Live`] is [`RefinerMode::Off`]:
    /// canvas plus lexicon, the complete shipping product.
    pub fn refiner_mode(&self) -> RefinerMode {
        match (&self.state, self.provider.as_ref()) {
            (Layer1LaneState::Live, Some(provider)) => provider.mode(),
            _ => RefinerMode::Off,
        }
    }

    /// Content-free counters so far.
    pub fn telemetry(&self) -> Layer1LaneTelemetry {
        self.telemetry
    }

    /// Latest volatile partial for `utterance_id`, when one is open.
    pub fn draft_text(&self, utterance_id: u64) -> Option<&str> {
        self.draft.get(&utterance_id).map(String::as_str)
    }

    /// Number of utterances with an open volatile draft.
    pub fn draft_len(&self) -> usize {
        self.draft.len()
    }

    /// Finals accepted so far, in accepted order.
    pub fn finals(&self) -> &[TranscriptEvent] {
        &self.finals
    }

    /// Take the one-shot degrade notice, if a degrade happened since the last
    /// call. The session loop uses this to emit exactly one warning event.
    pub fn take_degrade_notice(&mut self) -> Option<Layer1DegradeReason> {
        self.degrade_notice.take()
    }

    /// Offer one captured PCM frame. Returns immediately, always.
    ///
    /// Capture never branches on the verdict — it is informational. A frame
    /// offered while the lane is not live is silently ignored, which is what
    /// makes a degraded or disarmed lane indistinguishable from no lane at all
    /// on the capture path.
    pub fn offer_pcm(&mut self, samples: &[f32]) -> FanOutVerdict {
        self.telemetry.frames_offered += 1;
        if !self.is_live() || samples.is_empty() {
            return FanOutVerdict::Inactive;
        }
        let Some(provider) = self.provider.as_mut() else {
            return FanOutVerdict::Inactive;
        };
        match provider.push_audio(samples) {
            Ok(()) => {
                self.consecutive_overflows = 0;
                self.telemetry.frames_forwarded += 1;
                FanOutVerdict::Forwarded
            }
            Err(AsrErrorKind::Overflow) => {
                self.telemetry.overflow_frame_drops += 1;
                self.consecutive_overflows += 1;
                if self.consecutive_overflows >= OVERFLOW_DEGRADE_LIMIT {
                    self.degrade_dropping_provider(Layer1DegradeReason::Overflow);
                }
                FanOutVerdict::DroppedOverflow
            }
            Err(kind) => {
                self.telemetry.provider_errors += 1;
                self.degrade_dropping_provider(Layer1DegradeReason::Disconnect(kind));
                FanOutVerdict::Inactive
            }
        }
    }

    /// Drain ready provider events and route them. Non-blocking.
    pub fn poll(&mut self) {
        if !self.is_live() {
            return;
        }
        let Some(provider) = self.provider.as_mut() else {
            return;
        };
        let events = provider.drain();
        for event in events {
            self.route_event(event);
            if !self.is_live() {
                // A fatal error event degraded the lane; anything still queued
                // belonged to the session that just ended.
                break;
            }
        }
    }

    /// The host slept mid-recording: the provider session is presumed stale.
    ///
    /// Wired by the platform sleep observer when one is present; the state
    /// transition is the contract either way.
    pub fn note_sleep_wake(&mut self) {
        if self.is_live() {
            self.degrade_dropping_provider(Layer1DegradeReason::SleepWake);
        }
    }

    /// Stop the lane at recording end: graceful close, bounded trailing drain.
    ///
    /// This is the only lane call with bounded blocking (the provider's own
    /// close contract). Whatever happens inside it, the method returns an
    /// outcome and the lane ends [`Layer1LaneState::Stopped`] — the stop path
    /// never propagates a Layer 1 failure.
    pub fn stop(&mut self) -> Layer1SessionOutcome {
        if let Some(mut provider) = self.provider.take() {
            // Route anything already decoded before asking for the tail.
            for event in provider.drain() {
                self.route_event(event);
            }
            match provider.close() {
                Ok(()) => {
                    let mut polls = 0u32;
                    loop {
                        let events = provider.drain();
                        if events.is_empty() {
                            break;
                        }
                        for event in events {
                            self.route_event(event);
                        }
                        polls += 1;
                        if polls >= STOP_DRAIN_MAX_POLLS {
                            self.note_degrade(Layer1DegradeReason::StopDrainIncomplete);
                            break;
                        }
                    }
                }
                Err(kind) => {
                    self.telemetry.provider_errors += 1;
                    self.note_degrade(Layer1DegradeReason::Disconnect(kind));
                }
            }
            // The provider drops here; a cloud transport aborts on drop.
        }
        self.draft.clear();
        self.state = Layer1LaneState::Stopped;
        Layer1SessionOutcome {
            finals: std::mem::take(&mut self.finals),
            telemetry: self.telemetry,
            degrade: self.degrade,
        }
    }

    /// Pass one provider event through the doctrine ledger and apply it.
    fn route_event(&mut self, event: AsrSessionEvent) {
        let verdict = self.ingest.ingest(event.clone());
        match verdict {
            IngestVerdict::Accepted => match event {
                AsrSessionEvent::Partial(transcript) => {
                    self.telemetry.partials_applied += 1;
                    self.draft
                        .insert(transcript.identity.utterance_id(), transcript.text);
                }
                AsrSessionEvent::Final(transcript) => {
                    self.telemetry.finals_accepted += 1;
                    self.draft.remove(&transcript.identity.utterance_id());
                    self.finals.push(transcript);
                }
                AsrSessionEvent::Error(error) => {
                    self.telemetry.provider_errors += 1;
                    if session_fatal(error.kind) && self.is_live() {
                        self.degrade_dropping_provider(Layer1DegradeReason::Disconnect(error.kind));
                    }
                }
                AsrSessionEvent::Usage(_) => {
                    // Accounting only; nothing to apply.
                }
            },
            IngestVerdict::DuplicateIdempotent => {
                // Re-delivery changed nothing, which is the point.
            }
            IngestVerdict::RejectedOutOfOrder
            | IngestVerdict::RejectedSealedUtterance
            | IngestVerdict::RejectedForeignSession => {
                self.telemetry.events_rejected += 1;
            }
        }
    }

    /// Record the sticky degrade reason and the one-shot notice.
    fn note_degrade(&mut self, reason: Layer1DegradeReason) {
        if self.degrade.is_none() {
            self.degrade = Some(reason);
            self.degrade_notice = Some(reason);
            warn!(
                reason = reason.as_token(),
                "Layer 1 lane degraded — canvas + lexicon carry the session"
            );
        }
    }

    /// Degrade on the live path: drop the provider without a graceful close.
    ///
    /// Dropping (rather than closing) is deliberate — see the module docs.
    /// The volatile draft dies with the lane; accepted finals stay, because
    /// they already passed the doctrine seam and remain gap-fill candidates.
    fn degrade_dropping_provider(&mut self, reason: Layer1DegradeReason) {
        self.provider = None;
        self.draft.clear();
        self.state = Layer1LaneState::Degraded(reason);
        self.note_degrade(reason);
    }
}

/// Apply one host lifecycle boundary to the active Layer 1 lane.
///
/// Kept as the single adapter used by the production session loop and its
/// deterministic channel-level regression. The transition itself remains
/// owned by [`RecorderLayer1Lane::note_sleep_wake`].
pub fn apply_recorder_lifecycle_event(
    lane: &mut RecorderLayer1Lane,
    event: RecorderLifecycleEvent,
) {
    match event {
        RecorderLifecycleEvent::SleepWake => lane.note_sleep_wake(),
    }
}

/// Whether one typed error kind ends the session for this recording.
///
/// `RateLimited` and `Overflow` describe pressure that the bounded fan-out
/// already absorbs frame by frame; everything else means the provider cannot
/// serve this session and the lane lands on canvas + lexicon.
fn session_fatal(kind: AsrErrorKind) -> bool {
    !matches!(kind, AsrErrorKind::RateLimited | AsrErrorKind::Overflow)
}

#[cfg(test)]
mod tests {
    use super::super::events::{
        ErrorEvent, EventIdentity, SessionId, TranscriptEvent as Transcript,
    };
    use super::super::fake::FakeAsrSessionProvider;
    use super::*;
    use crate::quality::Layer1MergeMode;

    /// Session identity every fixture in this module records under.
    fn session_id() -> SessionId {
        SessionId::new("recording-1").expect("non-blank session id")
    }

    /// The thin open parameters the recorder hands the lane.
    fn input() -> SessionInput {
        SessionInput {
            session_id: session_id(),
            locale: Some("pl-PL".to_string()),
            sample_rate: 16_000,
        }
    }

    /// Identity triple within the fixture session.
    fn identity(utterance_id: u64, sequence_id: u64) -> EventIdentity {
        EventIdentity::new(session_id(), utterance_id, sequence_id)
    }

    /// Partial event fixture.
    fn partial(utterance_id: u64, sequence_id: u64, text: &str) -> AsrSessionEvent {
        AsrSessionEvent::Partial(Transcript {
            identity: identity(utterance_id, sequence_id),
            text: text.to_string(),
            range: None,
        })
    }

    /// Final event fixture.
    fn final_event(utterance_id: u64, sequence_id: u64, text: &str) -> AsrSessionEvent {
        AsrSessionEvent::Final(Transcript {
            identity: identity(utterance_id, sequence_id),
            text: text.to_string(),
            range: None,
        })
    }

    /// Typed error event fixture.
    fn error_event(sequence_id: u64, kind: AsrErrorKind) -> AsrSessionEvent {
        AsrSessionEvent::Error(ErrorEvent {
            identity: identity(0, sequence_id),
            kind,
        })
    }

    /// An armed decision over a scripted fake provider.
    fn armed(script: Vec<AsrSessionEvent>) -> Layer1Decision {
        Layer1Decision::Armed(Box::new(FakeAsrSessionProvider::with_script(
            RefinerMode::CloudSession,
            script,
        )))
    }

    /// A missing provider is normal operation, not an error: the lane runs
    /// unarmed, ignores fan-out, and stops with an empty outcome.
    #[test]
    fn disarmed_lane_is_normal_apple_plus_lexicon_operation() {
        let mut lane = RecorderLayer1Lane::open(Layer1Decision::Disarmed, &input());
        assert_eq!(lane.state(), Layer1LaneState::Unarmed);
        assert_eq!(lane.refiner_mode(), RefinerMode::Off);

        assert_eq!(lane.offer_pcm(&[0.1; 320]), FanOutVerdict::Inactive);
        lane.poll();
        assert!(lane.take_degrade_notice().is_none(), "no degrade to report");

        let outcome = lane.stop();
        assert_eq!(lane.state(), Layer1LaneState::Stopped);
        assert!(outcome.finals().is_empty());
        assert!(outcome.degrade_reason().is_none());
        assert!(outcome.refined_transcript().is_none());
    }

    /// A provider whose open fails is dropped and the recording proceeds
    /// degraded — never an error surfaced to capture.
    #[test]
    fn open_failure_degrades_instead_of_erroring() {
        // Pre-open the fake so the lane's open hits a Protocol fault.
        let mut provider = FakeAsrSessionProvider::new(RefinerMode::CloudSession);
        provider.open(&input()).expect("first open succeeds");
        let mut lane =
            RecorderLayer1Lane::open(Layer1Decision::Armed(Box::new(provider)), &input());

        assert_eq!(
            lane.state(),
            Layer1LaneState::Degraded(Layer1DegradeReason::OpenFailed(AsrErrorKind::Protocol))
        );
        assert_eq!(lane.refiner_mode(), RefinerMode::Off);
        assert_eq!(
            lane.take_degrade_notice(),
            Some(Layer1DegradeReason::OpenFailed(AsrErrorKind::Protocol))
        );
        assert_eq!(lane.offer_pcm(&[0.1; 320]), FanOutVerdict::Inactive);
    }

    /// Partials are volatile draft: replaced freely, cleared by their final,
    /// and never part of the outcome's committed candidate.
    #[test]
    fn partials_stay_volatile_draft_until_the_final_seals() {
        let mut lane = RecorderLayer1Lane::open(
            armed(vec![
                partial(1, 1, "pacjent"),
                partial(1, 2, "pacjent ma"),
                final_event(1, 3, "pacjent ma goraczke"),
            ]),
            &input(),
        );

        lane.offer_pcm(&[0.1; 320]);
        lane.poll();
        assert_eq!(lane.draft_text(1), Some("pacjent"));

        lane.offer_pcm(&[0.1; 320]);
        lane.poll();
        assert_eq!(lane.draft_text(1), Some("pacjent ma"), "draft is replaced");

        lane.offer_pcm(&[0.1; 320]);
        lane.poll();
        assert_eq!(lane.draft_text(1), None, "the final clears its draft");
        assert_eq!(lane.finals().len(), 1);
        assert_eq!(lane.finals()[0].text, "pacjent ma goraczke");
    }

    /// The ingest doctrine holds inside the lane: duplicates are idempotent,
    /// stale finals cannot rewrite a sealed utterance.
    #[test]
    fn finals_route_through_the_ingest_doctrine() {
        let mut lane = RecorderLayer1Lane::open(
            armed(vec![
                final_event(1, 2, "pacjent ma goraczke"),
                final_event(1, 2, "pacjent ma goraczke"), // reconnect resend
                final_event(1, 1, "pacjent"),             // stale rewrite attempt
            ]),
            &input(),
        );

        for _ in 0..3 {
            lane.offer_pcm(&[0.1; 320]);
            lane.poll();
        }

        assert_eq!(lane.finals().len(), 1, "one sealed final");
        assert_eq!(lane.finals()[0].text, "pacjent ma goraczke");
        let telemetry = lane.telemetry();
        assert_eq!(telemetry.finals_accepted, 1);
        assert_eq!(
            telemetry.events_rejected, 1,
            "the stale rewrite was refused, not applied"
        );
        assert!(lane.is_live(), "doctrine refusals do not degrade the lane");
    }

    /// Bounded overflow: frames are dropped and counted while the run is
    /// short, and the lane stays live.
    #[test]
    fn overflow_below_the_budget_drops_frames_without_degrading() {
        let provider = FakeAsrSessionProvider::new(RefinerMode::CloudSession)
            .failing_pushes(AsrErrorKind::Overflow);
        let mut lane =
            RecorderLayer1Lane::open(Layer1Decision::Armed(Box::new(provider)), &input());

        for _ in 0..(OVERFLOW_DEGRADE_LIMIT - 1) {
            assert_eq!(lane.offer_pcm(&[0.1; 320]), FanOutVerdict::DroppedOverflow);
        }
        assert!(lane.is_live(), "a short overflow run is absorbed");
        assert_eq!(
            lane.telemetry().overflow_frame_drops,
            u64::from(OVERFLOW_DEGRADE_LIMIT - 1)
        );
    }

    /// Sustained overflow degrades to canvas + lexicon; capture keeps offering
    /// and the lane keeps returning instantly.
    #[test]
    fn sustained_overflow_degrades_and_capture_continues() {
        let provider = FakeAsrSessionProvider::new(RefinerMode::CloudSession)
            .failing_pushes(AsrErrorKind::Overflow);
        let mut lane =
            RecorderLayer1Lane::open(Layer1Decision::Armed(Box::new(provider)), &input());

        for _ in 0..OVERFLOW_DEGRADE_LIMIT {
            lane.offer_pcm(&[0.1; 320]);
        }
        assert_eq!(
            lane.state(),
            Layer1LaneState::Degraded(Layer1DegradeReason::Overflow)
        );
        assert_eq!(lane.refiner_mode(), RefinerMode::Off);
        assert_eq!(
            lane.take_degrade_notice(),
            Some(Layer1DegradeReason::Overflow)
        );
        // Capture is oblivious: further offers are ignored, never errors.
        assert_eq!(lane.offer_pcm(&[0.1; 320]), FanOutVerdict::Inactive);
    }

    /// A successful push resets the consecutive-overflow run, so scattered
    /// hiccups never accumulate into a degrade.
    #[test]
    fn interleaved_success_resets_the_overflow_run() {
        // Script one event so the first push succeeds, then force overflows.
        let mut lane = RecorderLayer1Lane::open(armed(vec![partial(1, 1, "a")]), &input());
        for _ in 0..(OVERFLOW_DEGRADE_LIMIT - 1) {
            // The fake accepts pushes (no failure armed): every offer forwards
            // and the overflow run stays at zero.
            assert_eq!(lane.offer_pcm(&[0.1; 320]), FanOutVerdict::Forwarded);
        }
        assert!(lane.is_live());
        assert_eq!(lane.telemetry().overflow_frame_drops, 0);
    }

    /// A transport-fatal push failure degrades as a disconnect.
    #[test]
    fn transport_push_failure_degrades_as_disconnect() {
        let provider = FakeAsrSessionProvider::new(RefinerMode::CloudSession)
            .failing_pushes(AsrErrorKind::Transport);
        let mut lane =
            RecorderLayer1Lane::open(Layer1Decision::Armed(Box::new(provider)), &input());

        assert_eq!(lane.offer_pcm(&[0.1; 320]), FanOutVerdict::Inactive);
        assert_eq!(
            lane.state(),
            Layer1LaneState::Degraded(Layer1DegradeReason::Disconnect(AsrErrorKind::Transport))
        );
        assert_eq!(lane.refiner_mode(), RefinerMode::Off);
    }

    /// A session-fatal error *event* degrades the lane; queued events behind
    /// it are abandoned with the session.
    #[test]
    fn fatal_error_event_degrades_the_lane() {
        let mut lane =
            RecorderLayer1Lane::open(armed(vec![error_event(1, AsrErrorKind::Auth)]), &input());
        lane.offer_pcm(&[0.1; 320]);
        lane.poll();
        assert_eq!(
            lane.state(),
            Layer1LaneState::Degraded(Layer1DegradeReason::Disconnect(AsrErrorKind::Auth))
        );
    }

    /// Rate limiting is pressure, not death: the lane counts it and stays live.
    #[test]
    fn rate_limit_error_event_is_absorbed_without_degrading() {
        let mut lane = RecorderLayer1Lane::open(
            armed(vec![
                error_event(1, AsrErrorKind::RateLimited),
                final_event(1, 2, "pacjent ma goraczke"),
            ]),
            &input(),
        );
        lane.offer_pcm(&[0.1; 320]);
        lane.poll();
        assert!(lane.is_live(), "rate limiting must not end the session");

        lane.offer_pcm(&[0.1; 320]);
        lane.poll();
        assert_eq!(lane.finals().len(), 1, "the session keeps producing");
    }

    /// Sleep/wake presumes the session stale and degrades immediately.
    #[test]
    fn sleep_wake_degrades_and_clears_the_draft() {
        let mut lane = RecorderLayer1Lane::open(armed(vec![partial(1, 1, "pacjent")]), &input());
        lane.offer_pcm(&[0.1; 320]);
        lane.poll();
        assert_eq!(lane.draft_len(), 1);

        lane.note_sleep_wake();
        assert_eq!(
            lane.state(),
            Layer1LaneState::Degraded(Layer1DegradeReason::SleepWake)
        );
        assert_eq!(lane.draft_len(), 0, "volatile draft dies with the lane");
    }

    /// The production lifecycle adapter, not a direct lane call, reaches the
    /// active transition and preserves the fail-closed semantics.
    #[tokio::test]
    async fn recorder_lifecycle_adapter_reaches_active_lane_transition() {
        let (handle, mut events) = recorder_lifecycle_channel();
        let mut lane = RecorderLayer1Lane::open(armed(vec![partial(1, 1, "pacjent")]), &input());
        lane.offer_pcm(&[0.1; 320]);
        lane.poll();
        assert_eq!(lane.draft_len(), 1);

        assert!(handle.note_sleep_wake(), "active adapter accepts boundary");
        let event = events
            .recv()
            .await
            .expect("active session receives boundary");
        apply_recorder_lifecycle_event(&mut lane, event);

        assert_eq!(
            lane.state(),
            Layer1LaneState::Degraded(Layer1DegradeReason::SleepWake)
        );
        assert_eq!(lane.draft_len(), 0, "adapter clears volatile draft");
        assert_eq!(
            lane.take_degrade_notice(),
            Some(Layer1DegradeReason::SleepWake),
            "the session will emit one content-free degrade warning"
        );
    }

    /// Stop drains the provider's tail (the fake flushes its remaining script
    /// on close) and the outcome carries the doctrine-vetted finals.
    #[test]
    fn stop_drain_collects_trailing_finals_bounded() {
        let mut lane = RecorderLayer1Lane::open(
            armed(vec![
                final_event(1, 1, "pacjent ma goraczke"),
                final_event(2, 2, "podano plyny"),
            ]),
            &input(),
        );
        // No pushes: the whole script is still queued when stop closes.
        let outcome = lane.stop();

        assert_eq!(lane.state(), Layer1LaneState::Stopped);
        assert_eq!(outcome.finals().len(), 2);
        assert_eq!(
            outcome.refined_transcript().as_deref(),
            Some("pacjent ma goraczke podano plyny")
        );
        assert!(
            outcome.degrade_reason().is_none(),
            "a clean close is not a degrade"
        );
    }

    /// Degrading mid-session keeps already-accepted finals: they passed the
    /// doctrine seam and remain bounded gap-fill candidates.
    #[test]
    fn degrade_keeps_doctrine_vetted_finals_for_the_outcome() {
        let provider = FakeAsrSessionProvider::with_script(
            RefinerMode::CloudSession,
            vec![
                final_event(1, 1, "pacjent ma goraczke"),
                error_event(2, AsrErrorKind::Transport),
            ],
        );
        let mut lane =
            RecorderLayer1Lane::open(Layer1Decision::Armed(Box::new(provider)), &input());
        lane.offer_pcm(&[0.1; 320]);
        lane.offer_pcm(&[0.1; 320]);
        lane.poll();
        assert!(matches!(lane.state(), Layer1LaneState::Degraded(_)));

        let outcome = lane.stop();
        assert_eq!(outcome.finals().len(), 1);
        assert_eq!(
            outcome.degrade_reason(),
            Some(Layer1DegradeReason::Disconnect(AsrErrorKind::Transport))
        );
    }

    /// The outcome routes through the T0 truth seam: the committed live floor
    /// is immutable and Layer 1 text only fills the tail/gaps.
    #[test]
    fn outcome_adjudication_preserves_the_live_floor() {
        let mut lane = RecorderLayer1Lane::open(
            armed(vec![final_event(
                1,
                1,
                "pacjent ma goraczke i wymioty od wczoraj",
            )]),
            &input(),
        );
        let outcome = lane.stop();

        let live_floor = "pacjent ma goraczke";
        let merged = outcome.adjudicate_against_live_floor(live_floor);
        assert_eq!(merged.mode, Layer1MergeMode::LiveFloorGapFill);
        assert!(
            merged.text.starts_with(live_floor),
            "committed live text must survive adjudication verbatim"
        );
        assert!(
            merged.text.contains("wymioty"),
            "the provider tail may extend the floor"
        );
    }

    /// With no finals the outcome refuses to fabricate a candidate, and the
    /// seam reports the live floor untouched.
    #[test]
    fn empty_outcome_leaves_the_live_floor_alone() {
        let mut lane = RecorderLayer1Lane::open(Layer1Decision::Disarmed, &input());
        let outcome = lane.stop();
        let merged = outcome.adjudicate_against_live_floor("pacjent ma goraczke");
        assert_eq!(merged.mode, Layer1MergeMode::LiveOnly);
        assert_eq!(merged.text, "pacjent ma goraczke");
    }

    /// Stopping a degraded or unarmed lane is a quiet no-op path — the stop
    /// path never propagates Layer 1 trouble.
    #[test]
    fn stop_after_degrade_is_quiet_and_final() {
        let provider = FakeAsrSessionProvider::new(RefinerMode::CloudSession)
            .failing_pushes(AsrErrorKind::Transport);
        let mut lane =
            RecorderLayer1Lane::open(Layer1Decision::Armed(Box::new(provider)), &input());
        lane.offer_pcm(&[0.1; 320]);
        assert!(matches!(lane.state(), Layer1LaneState::Degraded(_)));

        let outcome = lane.stop();
        assert_eq!(lane.state(), Layer1LaneState::Stopped);
        assert_eq!(
            outcome.degrade_reason(),
            Some(Layer1DegradeReason::Disconnect(AsrErrorKind::Transport))
        );
        assert!(outcome.finals().is_empty());
    }
}
