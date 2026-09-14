//! One retained owner per Max consultation. UI callers enqueue turns; they do
//! not own the future performing tool effects. Losing a UI receiver therefore
//! cannot cancel a turn and accidentally cause its tools to be replayed.
//!
//! The host still owns admission from acoustic/revision events, provider
//! construction, permissions and explicit cancellation. This module never
//! creates a recorder, changes delivery destination or invents PCM identity.

use std::sync::Arc;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, ensure};
use chrono::Utc;
use tokio::sync::{mpsc, oneshot};

use crate::config::FormattingPolicy;
use super::thread_store::consultation::ConsultationJournal;
use super::{
    AgentProvider, AgentSession, AgentUiEvent, ContentBlock, ImageAttachment,
    Role, StreamOptions, ThreadDeliveryGateway, ThreadDeliveryInput,
    ThreadDeliveryReceipt, ThreadDeliverySource, ThreadMessage,
};

/// Non-blocking host projection of tentative events. `Done` is withheld until
/// durable delivery succeeds; tool and text events do not authorize pasting.
pub type ConsultationEvents = Arc<dyn Fn(&str, &str, AgentUiEvent) + Send + Sync>;

/// Immutable reading of known sealed occurrences in one capture interval.
/// Requires measured speech coverage of that interval, but is not a semantic
/// turn verdict and does not certify lexical accuracy.
/// Grouping preserves PCM identity; generated words are never assigned back
/// to one arbitrary member. Construction neither mutates the ledger nor runs tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedConsultationInput {
    session_id: String,
    capture_epoch: u64,
    samples: std::ops::Range<u64>,
    members: Vec<(crate::pipeline::acoustic_ledger::OccurrenceIdentity, String)>,
    text: String,
}

impl SealedConsultationInput {
    /// Read under the caller's ledger lock. None means known speech is not yet
    /// fully labelled/sealed, or the interval contains no usable instruction.
    /// A crossing member is refused: clipping would invent an acoustic identity.
    pub fn from_ledger(
        ledger: &crate::pipeline::acoustic_ledger::AcousticLedger,
        session_id: &str,
        capture_epoch: u64,
        samples: std::ops::Range<u64>,
        speech: &crate::audio::capture_receipt::AcousticSpeechEvidence,
    ) -> Result<Option<Self>> {
        ensure!(!session_id.is_empty() && capture_epoch != 0 && samples.start < samples.end,
            "invalid consultation capture interval");
        // Ask the existing authority with the observer's entire actual extent.
        // Trimming evidence here could falsely certify an unobserved tail.
        let coverage = ledger.assess_seal_coverage(session_id, capture_epoch, speech, 0);
        if coverage.status.unavailable_reason().is_some()
            || !coverage.observed_samples.is_some_and(|end| end >= samples.end)
            || coverage.uncovered_speech_ranges.iter().any(|gap|
                gap.sample_start < samples.end && samples.start < gap.sample_end)
        {
            return Ok(None);
        }
        // Debt outside this candidate must not block earlier complete speech.
        // Member frontier and recovery checks below still apply inside it.
        let occurrences = ledger.qualified_occurrences().chain(ledger.occurrences())
            .filter(|occurrence| occurrence.session == session_id
                && occurrence.capture_epoch == capture_epoch
                && occurrence.sample_start < samples.end
                && samples.start < occurrence.sample_end)
            .collect::<std::collections::BTreeSet<_>>();
        if occurrences.is_empty() {
            return Ok(None);
        }
        let mut members = Vec::with_capacity(occurrences.len());
        let mut labels = Vec::with_capacity(occurrences.len());
        let mut previous_end = samples.start;
        for occurrence in occurrences {
            ensure!(occurrence.sample_start >= previous_end && occurrence.sample_end <= samples.end,
                "consultation interval crosses or overlaps an occurrence");
            previous_end = occurrence.sample_end;
            if !ledger.is_qualified(occurrence)
                || ledger.text_recovery_pending(occurrence)
                || !ledger.frontier_of(occurrence).is_some_and(|frontier| frontier.is_closed())
            {
                return Ok(None);
            }
            let Some(seal) = ledger.seal_of(occurrence) else { return Ok(None); };
            let Some(label) = ledger.text_of(occurrence).filter(|label| !label.trim().is_empty()) else {
                return Ok(None);
            };
            members.push((occurrence.clone(), seal.receipt_id.clone()));
            labels.push(label.to_string());
        }
        Ok(Some(Self {
            session_id: session_id.into(), capture_epoch, samples,
            members, text: labels.join(" "),
        }))
    }

    /// Exact capture session, never inferred from transcript text.
    pub fn session_id(&self) -> &str { &self.session_id }
    /// Recorder-owned clock epoch.
    pub fn capture_epoch(&self) -> u64 { self.capture_epoch }
    /// Candidate grouping interval, not a newly minted occurrence.
    pub fn samples(&self) -> std::ops::Range<u64> { self.samples.clone() }
    /// Every source occurrence paired with its existing seal receipt.
    pub fn members(&self) -> &[(crate::pipeline::acoustic_ledger::OccurrenceIdentity, String)] {
        &self.members
    }
    /// Ordered source labels, including repeated words from distinct PCM spans.
    pub fn text(&self) -> &str { &self.text }
}

/// Capture-local pending group boundaries. This owns transport order only;
/// the ledger still owns speech, labels and seals. A missing ready input is
/// retained for the next recovery tick, never consumed as an empty instruction.
/// The host must retain this owner until outstanding groups are resolved.
pub struct ConsultationInputQueue {
    session_id: String,
    capture_epoch: u64,
    accepted_end: u64,
    boundaries: std::collections::VecDeque<u64>,
}

impl ConsultationInputQueue {
    pub fn new(session_id: String, capture_epoch: u64) -> Result<Self> {
        ensure!(!session_id.trim().is_empty() && capture_epoch != 0,
            "consultation input requires capture identity");
        Ok(Self { session_id, capture_epoch, accepted_end: 0, boundaries: Default::default() })
    }

    /// Register a recorder-clock boundary. Duplicate ticks are harmless;
    /// backwards boundaries and queue pressure are explicit refusals. The
    /// caller must retain a refused boundary and retry, not discard the audio.
    pub fn append_boundary(&mut self, end: u64) -> Result<bool> {
        let last = self.boundaries.back().copied().unwrap_or(self.accepted_end);
        ensure!(end >= last, "consultation boundary moved backwards");
        if end == last { return Ok(false); }
        ensure!(self.boundaries.len() < 16, "consultation input queue is full");
        self.boundaries.push_back(end);
        Ok(true)
    }

    /// Re-read current ledger truth after each observer return. No cached
    /// label can hide a later recovery. Reading has no queue side effects.
    pub fn ready(
        &self,
        ledger: &crate::pipeline::acoustic_ledger::AcousticLedger,
        speech: &crate::audio::capture_receipt::AcousticSpeechEvidence,
    ) -> Result<Option<SealedConsultationInput>> {
        let Some(end) = self.boundaries.front().copied() else { return Ok(None); };
        SealedConsultationInput::from_ledger(
            ledger, &self.session_id, self.capture_epoch, self.accepted_end..end, speech,
        )
    }

    /// A semantic decision may join adjacent candidates, but cannot remove
    /// speech or merge their acoustic occurrences. Existing input snapshots
    /// cease to match the front and therefore cannot acknowledge this group.
    pub fn join_front(&mut self) -> Result<()> {
        ensure!(self.boundaries.len() >= 2, "no following consultation group yet");
        self.boundaries.pop_front();
        Ok(())
    }

    /// Only call after the retained executor has accepted this exact group.
    /// Provider failure is handled by its journal, never by replaying tools
    /// from this capture queue. A failed enqueue must not call this method.
    pub fn acknowledge(&mut self, input: &SealedConsultationInput) -> Result<()> {
        ensure!(input.session_id == self.session_id && input.capture_epoch == self.capture_epoch,
            "consultation acknowledgement names a different capture");
        ensure!(input.samples.start == self.accepted_end
            && self.boundaries.front().copied() == Some(input.samples.end),
            "consultation acknowledgement is stale or out of order");
        self.accepted_end = input.samples.end;
        self.boundaries.pop_front();
        Ok(())
    }

    pub fn pending_groups(&self) -> usize { self.boundaries.len() }
}

/// One admitted instruction, not an arbitrary partial ASR label.
pub struct ConsultationTurn {
    pub id: String,
    pub policy: FormattingPolicy,
    pub text: String,
    pub attachments: Vec<ImageAttachment>,
    pub options: StreamOptions,
    pub provider_name: String,
    /// Supply a newly admitted provider on settings changes only. It is
    /// installed inside the queue, never while an earlier turn is executing.
    pub replacement_provider: Option<Box<dyn AgentProvider>>,
}

/// Answer plus durable history receipt. Presentation must still admit this
/// against its own occurrence/revision identity before replacing visible text.
#[derive(Debug)]
pub struct ConsultationAnswer {
    pub turn_id: String,
    pub text: String,
    pub delivery: ThreadDeliveryReceipt,
}

struct QueuedTurn {
    turn: ConsultationTurn,
    reply: oneshot::Sender<Result<ConsultationAnswer>>,
    pending: PendingTurn,
}

enum OwnerCommand {
    Turn(QueuedTurn),
    Close(oneshot::Sender<()>),
}

#[derive(Default)]
struct Admission {
    closed: bool,
    pending: usize,
}

struct PendingTurn(Arc<std::sync::Mutex<Admission>>);

impl Drop for PendingTurn {
    fn drop(&mut self) {
        let mut admission = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        admission.pending -= 1;
    }
}

/// Cloneable admission handle, deliberately not a second history owner.
#[derive(Clone)]
pub struct ConsultationRuntime {
    id: String,
    tx: mpsc::Sender<OwnerCommand>,
    admission: Arc<std::sync::Mutex<Admission>>,
}

impl ConsultationRuntime {
    /// Start with a fresh session configured with tools/approvals. Load history
    /// under the exclusive consultation lease, never from a pre-lock snapshot.
    /// The bounded FIFO is the execution order, not just the rendering order.
    pub fn start(
        id: String,
        mut session: AgentSession,
        ui_rx: mpsc::Receiver<AgentUiEvent>,
        gateway: ThreadDeliveryGateway,
        events: ConsultationEvents,
        install_lease_path: PathBuf,
    ) -> Result<Self> {
        ensure!(!id.trim().is_empty(), "consultation identity is required");
        ensure!(session.messages().is_empty(), "consultation history must be loaded under its lease");
        let journal = gateway.open_consultation(&id)?;
        let history = gateway.restore_consultation(&id, journal.has_completed_turns())?;
        session.restore_messages(history);
        session.bind_execution_thread(id.clone());
        let (tx, rx) = mpsc::channel(16);
        tokio::spawn(run_owner(ConsultationOwner {
            id: id.clone(), session, ui_rx, gateway, journal, events, rx, install_lease_path,
        }));
        Ok(Self { id, tx, admission: Arc::new(std::sync::Mutex::new(Admission::default())) })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// Close only when no accepted work remains. The acknowledgement is sent
    /// after the owner drops its session and kernel journal lease. All cloned
    /// handles become closed under the same lock used for turn admission.
    pub async fn close_if_idle(&self) -> Result<()> {
        {
            let mut admission = self.admission.lock().map_err(|_| anyhow!("consultation admission lock poisoned"))?;
            ensure!(!admission.closed, "consultation already closing or closed");
            ensure!(admission.pending == 0, "consultation has pending work");
            admission.closed = true;
        }
        let (reply, closed) = oneshot::channel();
        self.tx.try_send(OwnerCommand::Close(reply)).map_err(|_| anyhow!("consultation owner unavailable during close"))?;
        closed.await.context("consultation owner stopped without close acknowledgement")
    }

    /// Enqueue without starting a competing provider future. A full queue is
    /// an explicit refusal, never silent loss. Dropping the returned receiver
    /// does not revoke an already accepted instruction.
    pub fn enqueue(&self, turn: ConsultationTurn) -> Result<oneshot::Receiver<Result<ConsultationAnswer>>> {
        ensure!(turn.policy == FormattingPolicy::Max, "consultation tools require Max");
        ensure!(!turn.id.trim().is_empty(), "turn identity is required");
        ensure!(!turn.text.trim().is_empty() || !turn.attachments.is_empty(), "empty consultation turn");
        {
            let mut admission = self.admission.lock().map_err(|_| anyhow!("consultation admission lock poisoned"))?;
            ensure!(!admission.closed, "consultation is closing or closed");
            admission.pending += 1;
        }
        let pending = PendingTurn(Arc::clone(&self.admission));
        let (reply, receipt) = oneshot::channel();
        self.tx.try_send(OwnerCommand::Turn(QueuedTurn { turn, reply, pending }))
            .map_err(|error| anyhow!("consultation admission failed: {error}"))?;
        Ok(receipt)
    }
}

struct ConsultationOwner {
    id: String,
    session: AgentSession,
    ui_rx: mpsc::Receiver<AgentUiEvent>,
    gateway: ThreadDeliveryGateway,
    journal: ConsultationJournal,
    events: ConsultationEvents,
    rx: mpsc::Receiver<OwnerCommand>,
    install_lease_path: PathBuf,
}

async fn run_owner(owner: ConsultationOwner) {
    let ConsultationOwner { id, mut session, mut ui_rx, gateway, mut journal,
        events, mut rx, install_lease_path } = owner;
    let mut unsettled: Option<String> = None;
    while let Some(command) = rx.recv().await {
        let QueuedTurn { turn, reply, pending } = match command {
            OwnerCommand::Turn(turn) => turn,
            OwnerCommand::Close(reply) => {
                drop(session);
                drop(journal);
                let _ = reply.send(());
                return;
            }
        };
        if let Some(reason) = &unsettled {
            drop(pending);
            let _ = reply.send(Err(anyhow!("consultation requires recovery: {reason}")));
            continue;
        }
        // Refuse before any provider or tool work if installation already owns
        // the file. Hold through history/journal settlement and the final reply.
        let _install_lease = match crate::config::acquire_agent_turn_lease_at(&install_lease_path) {
            Ok(lease) => lease,
            Err(error) => { drop(pending); let _ = reply.send(Err(error)); continue; }
        };
        if let Err(error) = journal.begin(&turn.id) {
            drop(pending);
            let _ = reply.send(Err(error));
            continue;
        }
        let turn_id = turn.id.clone();
        let result = run_turn(&id, &mut session, &mut ui_rx, &gateway, &events, turn).await
            .and_then(|answer| { journal.complete(&turn_id)?; Ok(answer) });
        if let Err(error) = &result {
            // Do not roll back successful tool effects or automatically retry a
            // partially executed instruction. The host must resolve this state.
            unsettled = Some(format!("turn {turn_id}: {error:#}"));
            events(&id, &turn_id, AgentUiEvent::Error(format!("{error:#}")));
        } else {
            events(&id, &turn_id, AgentUiEvent::Done);
        }
        drop(pending);
        let _ = reply.send(result);
    }
}

async fn run_turn(
    id: &str,
    session: &mut AgentSession,
    ui_rx: &mut mpsc::Receiver<AgentUiEvent>,
    gateway: &ThreadDeliveryGateway,
    events: &ConsultationEvents,
    turn: ConsultationTurn,
) -> Result<ConsultationAnswer> {
    if let Some(provider) = turn.replacement_provider {
        session.replace_provider(provider).await;
    }
    let history_start = session.messages().len();
    {
        let send = session.send(turn.text, turn.attachments, &turn.options);
        tokio::pin!(send);
        loop {
            tokio::select! {
                result = &mut send => { result?; break; }
                event = ui_rx.recv() => {
                    let event = event.context("consultation event channel closed")?;
                    if !matches!(event, AgentUiEvent::Done | AgentUiEvent::Error(_)) {
                        events(id, &turn.id, event);
                    }
                }
            }
        }
    }
    while let Ok(event) = ui_rx.try_recv() {
        if !matches!(event, AgentUiEvent::Done | AgentUiEvent::Error(_)) {
            events(id, &turn.id, event);
        }
    }
    // A pre-tool explanation is not the final answer. If the terminal round
    // has no assistant text, do not resurrect an earlier round's narration.
    let text = session.messages()[history_start..].last()
        .filter(|message| message.role == Role::Assistant && message.content.iter().any(|block| matches!(block, ContentBlock::Text(_))))
        .map(|message| message.content.iter().filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.as_str()),
            _ => None,
        }).collect::<Vec<_>>().join("\n"))
        .unwrap_or_default();
    let delivery = gateway.deliver(ThreadDeliveryInput {
        backend_id: id.to_string(),
        messages: session.messages().iter().map(ThreadMessage::from).collect(),
        provider: turn.provider_name,
        model: turn.options.model,
        source: ThreadDeliverySource::MaxConsultation,
        mode: "max".into(),
        tags: vec!["agent".into(), "max-consultation".into()],
        timestamp: Utc::now(),
    }).context("consultation answer not durably stored")?;
    Ok(ConsultationAnswer { turn_id: turn.id, text, delivery })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use async_trait::async_trait;
    use tokio::sync::Semaphore;
    use super::*;
    use crate::agent::{AgentEvent, Message, ThreadStore, ToolDefinition, ToolRegistry};

    fn consultation_speech_evidence(end: u64) -> crate::audio::capture_receipt::AcousticSpeechEvidence {
        use crate::audio::capture_receipt::{AcousticAvailability, AcousticSpeechEvidence, CaptureEvidenceIdentity};
        AcousticSpeechEvidence::measured(
            CaptureEvidenceIdentity::new("capture", 1), "synthetic-consultation",
            AcousticAvailability::Observed { observed_samples: end },
            vec![crate::stt::tail_provider::TailSampleRange {
                session: "capture".into(), capture_epoch: 1, sample_start: 0, sample_end: end,
            }],
        )
    }

    fn repeated_word_ledger(last_pending: bool) -> crate::pipeline::acoustic_ledger::AcousticLedger {
        use crate::pipeline::acoustic_ledger::{
            AcousticEvidence, AcousticLedger, EnergyCalibration, ObservationIdentity,
            ObservationProducer, OccurrenceIdentity,
        };
        let mut ledger = AcousticLedger::new();
        let calibration = EnergyCalibration::new("synthetic-consultation", 1.0, 1);
        for index in 0..5u64 {
            let occurrence = OccurrenceIdentity::new("capture", 1, index * 16_000, (index + 1) * 16_000);
            let evidence = AcousticEvidence {
                occurrence: occurrence.clone(), duration_ms: 1_000.0,
                energy_integral: 100.0, mean_rms_dbfs: -20.0, peak_dbfs: -10.0,
                vad_open_sample: Some(occurrence.sample_start),
                vad_close_sample: Some(occurrence.sample_end),
                evidence_calibration_version: calibration.version.clone(),
            };
            assert!(ledger.qualify(&evidence, &calibration).is_qualified());
            let pending = last_pending && index == 4;
            let mut producers = vec![ObservationProducer::Apple];
            if pending { producers.push(ObservationProducer::Whisper); }
            ledger.schedule_frontier(occurrence.clone(), producers);
            assert!(ledger.admit(
                &ObservationIdentity::new(ObservationProducer::Apple, index, 0, occurrence.clone()),
                "Iwo",
            ).grants_mutation());
            ledger.note_frontier_return(&occurrence, ObservationProducer::Apple);
            if !pending { ledger.seal(&occurrence).expect("synthetic closed member"); }
        }
        ledger
    }

    #[test]
    fn sealed_group_preserves_five_distinct_equal_words_and_original_receipts() {
        let ledger = repeated_word_ledger(false);
        let input = SealedConsultationInput::from_ledger(&ledger, "capture", 1, 0..80_000, &consultation_speech_evidence(80_000))
            .unwrap().expect("known members are sealed");
        assert_eq!(input.text(), "Iwo Iwo Iwo Iwo Iwo");
        assert_eq!(input.members().len(), 5);
        for (index, (occurrence, seal)) in input.members().iter().enumerate() {
            assert_eq!(occurrence.sample_start, index as u64 * 16_000);
            assert_eq!(seal, &ledger.seal_of(occurrence).unwrap().receipt_id);
        }
        assert_eq!(input.session_id(), "capture");
        assert_eq!(input.capture_epoch(), 1);
        assert_eq!(input.samples(), 0..80_000);
        assert_eq!(ledger.len(), 5, "reading must not merge ledger members");
        assert_eq!(
            SealedConsultationInput::from_ledger(&ledger, "capture", 1, 0..80_000, &consultation_speech_evidence(80_000)).unwrap(),
            Some(input),
        );
    }

    #[test]
    fn sealed_group_waits_for_last_observer_and_refuses_clipped_members() {
        let pending = repeated_word_ledger(true);
        assert!(SealedConsultationInput::from_ledger(&pending, "capture", 1, 0..80_000, &consultation_speech_evidence(80_000))
            .unwrap().is_none());
        let ledger = repeated_word_ledger(false);
        // A sealed prefix cannot certify speech beyond the last known word.
        let longer = consultation_speech_evidence(96_000);
        assert!(SealedConsultationInput::from_ledger(&ledger, "capture", 1, 0..96_000, &longer)
            .unwrap().is_none());
        // Later uncovered speech does not invalidate a fully covered prefix.
        assert!(SealedConsultationInput::from_ledger(&ledger, "capture", 1, 0..80_000, &longer)
            .unwrap().is_some());
        assert!(SealedConsultationInput::from_ledger(&ledger, "capture", 1, 0..80_000,
            &consultation_speech_evidence(79_999)).unwrap().is_none());
        let unavailable = crate::audio::capture_receipt::AcousticSpeechEvidence::unavailable(
            crate::audio::capture_receipt::CaptureEvidenceIdentity::new("capture", 1),
            "synthetic-consultation",
            crate::audio::capture_receipt::AcousticAvailability::NotObserved,
        );
        assert!(SealedConsultationInput::from_ledger(&ledger, "capture", 1, 0..80_000, &unavailable)
            .unwrap().is_none());
        assert!(SealedConsultationInput::from_ledger(&pending, "capture", 1, 0..64_000,
            &consultation_speech_evidence(80_000)).unwrap().is_some());
        assert!(SealedConsultationInput::from_ledger(&ledger, "capture", 1, 1..80_000, &consultation_speech_evidence(80_000)).is_err());
        assert!(SealedConsultationInput::from_ledger(&ledger, "capture", 1, 0..79_999, &consultation_speech_evidence(80_000)).is_err());
        assert!(SealedConsultationInput::from_ledger(&ledger, "capture", 2, 0..80_000, &consultation_speech_evidence(80_000))
            .unwrap().is_none());
        assert!(SealedConsultationInput::from_ledger(&ledger, "foreign", 1, 0..80_000, &consultation_speech_evidence(80_000))
            .unwrap().is_none());
        assert!(SealedConsultationInput::from_ledger(&ledger, "capture", 1, 10..10, &consultation_speech_evidence(80_000)).is_err());
    }

    #[test]
    fn input_queue_retains_unready_groups_and_rejects_stale_acknowledgements() {
        use crate::pipeline::acoustic_ledger::{ObservationProducer, OccurrenceIdentity};
        let mut ledger = repeated_word_ledger(true);
        let speech = consultation_speech_evidence(80_000);
        let mut queue = ConsultationInputQueue::new("capture".into(), 1).unwrap();
        assert!(queue.append_boundary(64_000).unwrap());
        assert!(!queue.append_boundary(64_000).unwrap());
        assert!(queue.append_boundary(80_000).unwrap());
        let first = queue.ready(&ledger, &speech).unwrap().unwrap();
        assert_eq!(first.text(), "Iwo Iwo Iwo Iwo");
        assert_eq!(queue.pending_groups(), 2);
        assert_eq!(queue.ready(&ledger, &speech).unwrap(), Some(first.clone()),
            "an unacknowledged read remains retryable after executor pressure");
        let mut foreign = ConsultationInputQueue::new("other".into(), 1).unwrap();
        foreign.append_boundary(64_000).unwrap();
        assert!(foreign.acknowledge(&first).is_err());
        assert_eq!(foreign.pending_groups(), 1);
        queue.acknowledge(&first).unwrap();
        assert!(queue.acknowledge(&first).is_err(), "old acknowledgement cannot consume the next group");
        for _ in 0..3 {
            assert!(queue.ready(&ledger, &speech).unwrap().is_none());
            assert_eq!(queue.pending_groups(), 1, "Whisper wait must retain the instruction");
        }
        let last = OccurrenceIdentity::new("capture", 1, 64_000, 80_000);
        ledger.note_frontier_return(&last, ObservationProducer::Whisper);
        ledger.seal(&last).unwrap();
        let recovered = queue.ready(&ledger, &speech).unwrap().unwrap();
        assert_eq!(recovered.text(), "Iwo");
        assert_eq!(recovered.members()[0].0, last);
        queue.acknowledge(&recovered).unwrap();
        assert_eq!(queue.pending_groups(), 0);
        assert!(queue.ready(&ledger, &speech).unwrap().is_none());
        assert!(!queue.append_boundary(80_000).unwrap());
        assert!(queue.append_boundary(79_999).is_err());

        let mut joined = ConsultationInputQueue::new("capture".into(), 1).unwrap();
        joined.append_boundary(64_000).unwrap();
        assert!(joined.join_front().is_err(), "waiting for a continuation must keep the first group");
        assert_eq!(joined.pending_groups(), 1);
        joined.append_boundary(80_000).unwrap();
        joined.join_front().unwrap();
        assert!(joined.acknowledge(&first).is_err(), "joining invalidates the narrower snapshot");
        let whole = joined.ready(&ledger, &speech).unwrap().unwrap();
        assert_eq!(whole.text(), "Iwo Iwo Iwo Iwo Iwo");
        assert_eq!(whole.members().len(), 5);
        joined.acknowledge(&whole).unwrap();

        let mut full = ConsultationInputQueue::new("capture".into(), 1).unwrap();
        for boundary in 1..=16 { full.append_boundary(boundary).unwrap(); }
        assert!(full.append_boundary(17).is_err());
        assert_eq!(full.pending_groups(), 16, "pressure cannot evict an earlier group");
        full.join_front().unwrap();
        assert!(full.append_boundary(17).unwrap(), "refused boundary remains retryable");
        assert_eq!(full.pending_groups(), 16);
    }

    struct ObservedProvider {
        requests: Arc<Mutex<Vec<Vec<Message>>>>,
        entered: Arc<Semaphore>,
        release: Arc<Semaphore>,
        fail: bool,
    }

    #[async_trait]
    impl AgentProvider for ObservedProvider {
        async fn stream(&self, messages: &[Message], _: &[ToolDefinition], _: &StreamOptions) -> Result<mpsc::Receiver<AgentEvent>> {
            let first = {
                let mut requests = self.requests.lock().expect("requests");
                requests.push(messages.to_vec());
                requests.len() == 1
            };
            self.entered.add_permits(1);
            if first {
                self.release.acquire().await.expect("release").forget();
            }
            let (tx, rx) = mpsc::channel(4);
            if self.fail {
                tx.send(AgentEvent::Error("provider interrupted".into())).await.expect("event");
            } else {
                tx.send(AgentEvent::TextDone("prepared command".into())).await.expect("text");
                tx.send(AgentEvent::ResponseDone { response_id: None, clean: true }).await.expect("done");
            }
            Ok(rx)
        }

        fn build_tool_result(&self, id: &str, content: Vec<ContentBlock>, is_error: bool) -> Message {
            Message::new(Role::User, vec![ContentBlock::ToolResult { tool_use_id: id.into(), content, is_error }])
        }

        fn build_image_block(&self, data: &[u8], media_type: &str) -> ContentBlock {
            ContentBlock::Image { data: data.to_vec(), media_type: media_type.into() }
        }

        fn name(&self) -> &str { "observed" }
    }

    fn turn(id: &str, text: &str) -> ConsultationTurn {
        ConsultationTurn {
            id: id.into(), policy: FormattingPolicy::Max, text: text.into(),
            attachments: Vec::new(), options: StreamOptions::default(),
            provider_name: "observed".into(), replacement_provider: None,
        }
    }

    #[tokio::test]
    async fn queue_preserves_history_and_completes_when_ui_receiver_is_dropped() {
        let dir = tempfile::tempdir().expect("temp directory");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let provider = ObservedProvider { requests: Arc::clone(&requests), entered: Arc::clone(&entered), release: Arc::clone(&release), fail: false };
        let (ui_tx, ui_rx) = mpsc::channel(2);
        let session = AgentSession::new(Box::new(provider), Arc::new(ToolRegistry::new()), ui_tx);
        let runtime = ConsultationRuntime::start("consultation-a".into(), session, ui_rx,
            ThreadDeliveryGateway::new_in(dir.path()).expect("gateway"), Arc::new(|_, _, _| {}), dir.path().join("agent-turn.lock")).expect("runtime");

        let first = runtime.enqueue(turn("one", "Prepare a command, do not execute it.")).expect("first accepted");
        entered.acquire().await.expect("first entered").forget();
        assert!(runtime.close_if_idle().await.is_err(), "active instruction cannot be detached");
        drop(first);
        let second = runtime.enqueue(turn("two", "Now change only the file name.")).expect("second accepted");
        assert_eq!(requests.lock().expect("requests").len(), 1);
        release.add_permits(1);
        let answer = second.await.expect("owner remains live").expect("second completed");
        assert_eq!(answer.delivery.message_count, 4);
        assert_eq!(answer.delivery.backend_id, "consultation-a");
        let observed = requests.lock().expect("requests");
        assert_eq!(observed.len(), 2);
        assert_eq!(observed[1].len(), 3);
        assert_eq!(observed[1][0].content, observed[0][0].content);
        assert_eq!(observed[1][1].role, Role::Assistant);
        drop(observed);
        let stored = ThreadStore::new_in(dir.path()).expect("store").load_thread("consultation-a").expect("durable history");
        assert_eq!(stored.messages.len(), 4);
        assert_eq!(stored.mode, "max");
        let duplicate = runtime.enqueue(turn("one", "Do not repeat tools.")).expect("queued");
        assert!(duplicate.await.expect("reply").is_err());
        assert_eq!(requests.lock().expect("requests").len(), 2);
        let stale_handle = runtime.clone();
        runtime.close_if_idle().await.expect("idle owner closes");
        assert!(stale_handle.enqueue(turn("three", "must not resurrect closed owner")).is_err());
        let gateway = ThreadDeliveryGateway::new_in(dir.path()).expect("gateway");
        assert!(gateway.open_consultation("consultation-a").is_ok(), "close receipt releases journal lease");
    }

    #[tokio::test]
    async fn failed_turn_stops_following_work_and_other_policies_cannot_enter() {
        let dir = tempfile::tempdir().expect("temp directory");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (ui_tx, ui_rx) = mpsc::channel(2);
        let session = AgentSession::new(Box::new(ObservedProvider {
            requests: Arc::clone(&requests), entered: Arc::new(Semaphore::new(0)),
            release: Arc::new(Semaphore::new(1)), fail: true,
        }), Arc::new(ToolRegistry::new()), ui_tx);
        let runtime = ConsultationRuntime::start("consultation-b".into(), session, ui_rx,
            ThreadDeliveryGateway::new_in(dir.path()).expect("gateway"), Arc::new(|_, _, _| {}), dir.path().join("agent-turn.lock")).expect("runtime");
        for policy in [FormattingPolicy::Off, FormattingPolicy::Correction, FormattingPolicy::Smart] {
            let mut request = turn("not-max", "use tools");
            request.policy = policy;
            assert!(runtime.enqueue(request).is_err());
        }
        let first = runtime.enqueue(turn("one", "first")).expect("accepted");
        let second = runtime.enqueue(turn("two", "must not execute")).expect("queued");
        assert!(first.await.expect("first reply").is_err());
        assert!(second.await.expect("second reply").expect_err("recovery required").to_string().contains("requires recovery"));
        assert_eq!(requests.lock().expect("requests").len(), 1);
        runtime.close_if_idle().await.expect("failed owner can close without replay");
        let gateway = ThreadDeliveryGateway::new_in(dir.path()).expect("gateway");
        assert!(gateway.open_consultation("consultation-b").is_err(), "closing must not erase unresolved turn");
    }

    #[tokio::test]
    async fn installer_ownership_refuses_turn_before_provider_execution() {
        use std::os::fd::AsRawFd;
        let dir = tempfile::tempdir().expect("temp directory");
        let lease_path = dir.path().join("agent-turn.lock");
        let installer = std::fs::OpenOptions::new().read(true).write(true)
            .create(true).truncate(false).open(&lease_path).expect("installer file");
        // SAFETY: installer retains this valid descriptor until explicit drop.
        assert_eq!(unsafe { libc::flock(installer.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }, 0);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (ui_tx, ui_rx) = mpsc::channel(2);
        let session = AgentSession::new(Box::new(ObservedProvider {
            requests: Arc::clone(&requests), entered: Arc::new(Semaphore::new(0)),
            release: Arc::new(Semaphore::new(1)), fail: false,
        }), Arc::new(ToolRegistry::new()), ui_tx);
        let runtime = ConsultationRuntime::start("installer-case".into(), session, ui_rx,
            ThreadDeliveryGateway::new_in(dir.path()).expect("gateway"),
            Arc::new(|_, _, _| {}), lease_path).expect("runtime");
        let refused = runtime.enqueue(turn("one", "prepare command")).expect("queued");
        assert!(refused.await.expect("reply").is_err());
        assert!(requests.lock().expect("requests").is_empty());
        drop(installer);
        // No effects were admitted, so explicitly resubmitting this instruction
        // after installation is permitted; the owner does not auto-retry it.
        let accepted = runtime.enqueue(turn("one", "prepare command")).expect("queued again");
        assert!(accepted.await.expect("reply").is_ok());
        assert_eq!(requests.lock().expect("requests").len(), 1);
    }
}
