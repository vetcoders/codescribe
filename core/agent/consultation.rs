//! One retained owner per Max consultation. UI callers enqueue turns; they do
//! not own the future performing tool effects. Losing a UI receiver therefore
//! cannot cancel a turn and accidentally cause its tools to be replayed.
//!
//! The host still owns admission from acoustic/revision events, provider
//! construction, permissions and explicit cancellation. This module never
//! creates a recorder, changes delivery destination or invents PCM identity.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, ensure};
use chrono::Utc;
use tokio::sync::{mpsc, oneshot};

use super::thread_store::consultation::{ConsultationJournal, QueuedInstruction};
use super::{
    AgentProvider, AgentSession, AgentUiEvent, ContentBlock, ImageAttachment, Role, StreamOptions,
    ThreadDeliveryGateway, ThreadDeliveryInput, ThreadDeliveryReceipt, ThreadDeliverySource,
    ThreadMessage,
};
use crate::config::FormattingPolicy;

/// Non-blocking host projection of tentative events. `Done` is withheld until
/// durable delivery succeeds; tool and text events do not authorize pasting.
pub type ConsultationEvents = Arc<dyn Fn(&str, &str, AgentUiEvent) + Send + Sync>;

/// Semantic assessment of one immutable candidate, not execution permission.
/// Before enqueueing, the capture owner must match this input against fresh
/// ledger truth and reject it if speech resumed or the candidate changed.
pub enum ConsultationReadiness {
    Complete(SealedConsultationInput),
    Continue(SealedConsultationInput),
}

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
    members: Vec<crate::pipeline::acoustic_ledger::ConsultationPresentationMember>,
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
        ensure!(
            !session_id.is_empty() && capture_epoch != 0 && samples.start < samples.end,
            "invalid consultation capture interval"
        );
        // Ask the existing authority with the observer's entire actual extent.
        // Trimming evidence here could falsely certify an unobserved tail.
        let coverage = ledger.assess_seal_coverage(session_id, capture_epoch, speech, 0);
        if coverage.status.unavailable_reason().is_some()
            || coverage
                .observed_samples
                .is_none_or(|end| end < samples.end)
            || coverage
                .uncovered_speech_ranges
                .iter()
                .any(|gap| gap.sample_start < samples.end && samples.start < gap.sample_end)
        {
            return Ok(None);
        }
        // Debt outside this candidate must not block earlier complete speech.
        // Member frontier and recovery checks below still apply inside it.
        let occurrences = ledger
            .qualified_occurrences()
            .chain(ledger.occurrences())
            .filter(|occurrence| {
                occurrence.session == session_id
                    && occurrence.capture_epoch == capture_epoch
                    && occurrence.sample_start < samples.end
                    && samples.start < occurrence.sample_end
            })
            .collect::<std::collections::BTreeSet<_>>();
        if occurrences.is_empty() {
            return Ok(None);
        }
        let mut members = Vec::with_capacity(occurrences.len());
        let mut labels = Vec::with_capacity(occurrences.len());
        let mut previous_end = samples.start;
        for occurrence in occurrences {
            ensure!(
                occurrence.sample_start >= previous_end && occurrence.sample_end <= samples.end,
                "consultation interval crosses or overlaps an occurrence"
            );
            previous_end = occurrence.sample_end;
            if !ledger.is_qualified(occurrence)
                || ledger.text_recovery_pending(occurrence)
                || !ledger
                    .frontier_of(occurrence)
                    .is_some_and(|frontier| frontier.is_closed())
            {
                return Ok(None);
            }
            let Some(seal) = ledger.seal_of(occurrence) else {
                return Ok(None);
            };
            let Some(label) = ledger
                .text_of(occurrence)
                .filter(|label| !label.trim().is_empty())
            else {
                return Ok(None);
            };
            members.push(
                crate::pipeline::acoustic_ledger::ConsultationPresentationMember {
                    occurrence: occurrence.clone(),
                    source_label: label.to_string(),
                    seal_receipt: seal.receipt_id.clone(),
                },
            );
            labels.push(label.to_string());
        }
        Ok(Some(Self {
            session_id: session_id.into(),
            capture_epoch,
            samples,
            members,
            text: labels.join(" "),
        }))
    }

    /// Exact capture session, never inferred from transcript text.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    /// Recorder-owned clock epoch.
    pub fn capture_epoch(&self) -> u64 {
        self.capture_epoch
    }
    /// Candidate grouping interval, not a newly minted occurrence.
    pub fn samples(&self) -> std::ops::Range<u64> {
        self.samples.clone()
    }
    /// Every source occurrence paired with its existing seal receipt.
    pub fn members(&self) -> &[crate::pipeline::acoustic_ledger::ConsultationPresentationMember] {
        &self.members
    }
    /// Ordered source labels, including repeated words from distinct PCM spans.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Stable execution key within the selected consultation. Source labels
    /// and presentation revisions cannot mint another execution of this group.
    pub fn turn_id_for_group(&self) -> String {
        format!(
            "capture-group:{}:{}:{}:{}:{}",
            self.session_id.len(),
            self.session_id,
            self.capture_epoch,
            self.samples.start,
            self.samples.end
        )
    }
}

/// Capture-local pending group boundaries. This owns transport order only;
/// the ledger still owns speech, labels and seals. A missing ready input is
/// retained for the next recovery tick, never consumed as an empty instruction.
/// The host must retain this owner until outstanding groups are resolved.
pub struct ConsultationInputQueue {
    session_id: String,
    capture_epoch: u64,
    accepted_end: u64,
    last_boundary_end: u64,
    boundaries: std::collections::VecDeque<u64>,
}

impl ConsultationInputQueue {
    pub fn new(session_id: String, capture_epoch: u64) -> Result<Self> {
        ensure!(
            !session_id.trim().is_empty() && capture_epoch != 0,
            "consultation input requires capture identity"
        );
        Ok(Self {
            session_id,
            capture_epoch,
            accepted_end: 0,
            last_boundary_end: 0,
            boundaries: Default::default(),
        })
    }

    /// Register a recorder-clock boundary. Duplicate ticks are harmless;
    /// backwards boundaries and queue pressure are explicit refusals. The
    /// caller must retain a refused boundary and retry, not discard the audio.
    pub fn append_boundary(&mut self, end: u64) -> Result<bool> {
        let last = self.last_boundary_end;
        ensure!(end >= last, "consultation boundary moved backwards");
        if end == last {
            return Ok(false);
        }
        ensure!(
            self.boundaries.len() < 16,
            "consultation input queue is full"
        );
        self.boundaries.push_back(end);
        self.last_boundary_end = end;
        Ok(true)
    }

    /// Recorder-observed continuation invalidates all unaccepted candidates.
    /// Preserve the accepted prefix and clock high-water mark: the next new
    /// boundary includes all pending speech, without replaying accepted tools.
    /// This changes grouping only, never ledger occurrences or their seals.
    pub fn resume_speech(&mut self) {
        self.boundaries.clear();
    }

    /// Re-read current ledger truth after each observer return. No cached
    /// label can hide a later recovery. Reading has no queue side effects.
    pub fn ready(
        &self,
        ledger: &crate::pipeline::acoustic_ledger::AcousticLedger,
        speech: &crate::audio::capture_receipt::AcousticSpeechEvidence,
    ) -> Result<Option<SealedConsultationInput>> {
        let Some(end) = self.boundaries.front().copied() else {
            return Ok(None);
        };
        SealedConsultationInput::from_ledger(
            ledger,
            &self.session_id,
            self.capture_epoch,
            self.accepted_end..end,
            speech,
        )
    }

    /// Validate before authorizing a prepared candidate, not after tools start.
    /// The capture owner must hold its current ledger/observer state throughout
    /// this synchronous call and invalidate boundaries when speech resumes.
    /// Accepts only an already persisted handle, never a caller callback.
    /// The returned handle remains the caller's responsibility: acknowledge it
    /// and retain it for completion, including if acknowledgement refuses.
    /// Stale source and lost authorization transport never consume the front.
    pub fn authorize_prepared(
        &mut self,
        prepared: PreparedConsultationGroup,
        ledger: &crate::pipeline::acoustic_ledger::AcousticLedger,
        speech: &crate::audio::capture_receipt::AcousticSpeechEvidence,
    ) -> Result<PendingConsultationGroup> {
        ensure!(
            self.ready(ledger, speech)?.as_ref() == Some(prepared.input()),
            "consultation assessment no longer matches current source"
        );
        prepared.authorize()
    }

    /// Advance past measured silence only. Unknown audio, observed speech and
    /// ledger occurrences all retain the front, even when no label exists.
    /// This does not acknowledge an Agent turn or invent a seal.
    pub fn skip_measured_silence(
        &mut self,
        ledger: &crate::pipeline::acoustic_ledger::AcousticLedger,
        speech: &crate::audio::capture_receipt::AcousticSpeechEvidence,
    ) -> bool {
        let Some(end) = self.boundaries.front().copied() else {
            return false;
        };
        let coverage = ledger.assess_seal_coverage(&self.session_id, self.capture_epoch, speech, 0);
        if coverage.status.unavailable_reason().is_some()
            || coverage
                .observed_samples
                .is_none_or(|observed| observed < end)
            || speech
                .ranges()
                .iter()
                .any(|range| range.sample_start < end && self.accepted_end < range.sample_end)
            || ledger
                .qualified_occurrences()
                .chain(ledger.occurrences())
                .any(|occurrence| {
                    occurrence.session == self.session_id
                        && occurrence.capture_epoch == self.capture_epoch
                        && occurrence.sample_start < end
                        && self.accepted_end < occurrence.sample_end
                })
        {
            return false;
        }
        self.accepted_end = end;
        self.boundaries.pop_front();
        true
    }

    /// A semantic decision may join adjacent candidates, but cannot remove
    /// speech or merge their acoustic occurrences. Existing input snapshots
    /// cease to match the front and therefore cannot acknowledge this group.
    pub fn join_front(&mut self) -> Result<()> {
        ensure!(
            self.boundaries.len() >= 2,
            "no following consultation group yet"
        );
        self.boundaries.pop_front();
        Ok(())
    }

    /// Requires the retained executor's admission handle, not merely a ready
    /// ledger reading. The caller keeps the handle for completion/recovery even
    /// if this acknowledgement fails. Queue acceptance is not durable completion.
    pub fn acknowledge(&mut self, accepted: &PendingConsultationGroup) -> Result<()> {
        let input = accepted.input();
        ensure!(
            input.session_id == self.session_id && input.capture_epoch == self.capture_epoch,
            "consultation acknowledgement names a different capture"
        );
        ensure!(
            input.samples.start == self.accepted_end
                && self.boundaries.front().copied() == Some(input.samples.end),
            "consultation acknowledgement is stale or out of order"
        );
        self.accepted_end = input.samples.end;
        self.boundaries.pop_front();
        Ok(())
    }

    pub fn pending_groups(&self) -> usize {
        self.boundaries.len()
    }
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

/// Durably accepted input, not completed execution or presentation permission.
/// Keep this handle while capture advances. Dropping it does not cancel tools.
pub struct PendingConsultationGroup {
    consultation_id: String,
    input: SealedConsultationInput,
    reply: oneshot::Receiver<Result<ConsultationAnswer>>,
}

/// Persisted candidate with no execution authority. Preparation may block on
/// disk and must run outside capture and its ledger lock. Dropping this handle
/// rejects the candidate; only the owner removes its unstarted journal entry.
pub struct PreparedConsultationGroup {
    pending: PendingConsultationGroup,
    authorize: oneshot::Sender<()>,
}

impl PreparedConsultationGroup {
    pub fn input(&self) -> &SealedConsultationInput {
        self.pending.input()
    }

    /// Call only after capture revalidates this exact source. This operation
    /// performs no I/O and takes no journal or admission lock.
    pub fn authorize(self) -> Result<PendingConsultationGroup> {
        self.authorize
            .send(())
            .map_err(|_| anyhow!("consultation owner stopped before authorization"))?;
        Ok(self.pending)
    }
}

impl PendingConsultationGroup {
    pub fn input(&self) -> &SealedConsultationInput {
        &self.input
    }

    /// No provider retry is performed here, including after a lost reply.
    pub async fn finish(self) -> Result<ConsultationGroupAnswer> {
        let answer = self
            .reply
            .await
            .context("consultation owner stopped before group reply")??;
        ensure!(
            answer.turn_id == self.input.turn_id_for_group(),
            "consultation group turn mismatch"
        );
        ensure!(
            answer.delivery.backend_id == self.consultation_id,
            "consultation group history mismatch"
        );
        Ok(ConsultationGroupAnswer {
            input: self.input,
            answer,
        })
    }
}

/// Correlated source group and completed history. This is transport evidence;
/// the reducer must still validate the current destination and member receipts.
#[derive(Debug)]
pub struct ConsultationGroupAnswer {
    input: SealedConsultationInput,
    answer: ConsultationAnswer,
}

impl ConsultationGroupAnswer {
    pub fn input(&self) -> &SealedConsultationInput {
        &self.input
    }
    pub fn answer(&self) -> &ConsultationAnswer {
        &self.answer
    }
}

struct QueuedTurn {
    turn: ConsultationTurn,
    reply: oneshot::Sender<Result<ConsultationAnswer>>,
    pending: PendingTurn,
    authorization: Option<oneshot::Receiver<()>>,
}

enum OwnerCommand {
    Turn(Box<QueuedTurn>),
    Close(oneshot::Sender<()>),
}

#[derive(Default)]
struct Admission {
    closed: bool,
    pending: Arc<std::sync::atomic::AtomicUsize>,
}

struct PendingTurn(Arc<std::sync::atomic::AtomicUsize>);

impl Drop for PendingTurn {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Cloneable admission handle, deliberately not a second history owner.
#[derive(Clone)]
pub struct ConsultationRuntime {
    id: String,
    tx: mpsc::Sender<OwnerCommand>,
    admission: Arc<std::sync::Mutex<Admission>>,
    // Stale admission handles must not retain the owner's kernel lease.
    journal: std::sync::Weak<std::sync::Mutex<ConsultationJournal>>,
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
        ensure!(
            session.messages().is_empty(),
            "consultation history must be loaded under its lease"
        );
        let journal = gateway.open_consultation(&id)?;
        let history = gateway.restore_consultation(&id, journal.has_completed_turns())?;
        session.restore_messages(history);
        session.bind_execution_thread(id.clone());
        let journal = Arc::new(std::sync::Mutex::new(journal));
        let journal_handle = Arc::downgrade(&journal);
        let (tx, rx) = mpsc::channel(16);
        tokio::spawn(run_owner(ConsultationOwner {
            id: id.clone(),
            session,
            ui_rx,
            gateway,
            journal,
            events,
            rx,
            install_lease_path,
        }));
        Ok(Self {
            id,
            tx,
            admission: Arc::new(std::sync::Mutex::new(Admission::default())),
            journal: journal_handle,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// Persist a candidate off the capture thread. Bind sealed source to its
    /// queued instruction; no effects run until capture authorizes the handle.
    pub fn prepare_group(
        &self,
        input: SealedConsultationInput,
        turn: ConsultationTurn,
    ) -> Result<PreparedConsultationGroup> {
        ensure!(
            turn.id == input.turn_id_for_group(),
            "consultation group turn mismatch"
        );
        ensure!(
            turn.text == input.text(),
            "consultation group source mismatch"
        );
        let group = serde_json::json!({
            "session_id": input.session_id(), "capture_epoch": input.capture_epoch(),
            "sample_start": input.samples.start, "sample_end": input.samples.end,
            "members": input.members().iter().map(|member| serde_json::json!({
                "session_id": member.occurrence.session,
                "capture_epoch": member.occurrence.capture_epoch,
                "sample_start": member.occurrence.sample_start,
                "sample_end": member.occurrence.sample_end,
                "source_label": member.source_label, "seal_receipt": member.seal_receipt,
            })).collect::<Vec<_>>(),
        });
        let (authorize, authorization) = oneshot::channel();
        let reply = self.enqueue_recorded(turn, Some(group), Some(authorization))?;
        Ok(PreparedConsultationGroup {
            pending: PendingConsultationGroup {
                consultation_id: self.id.clone(),
                input,
                reply,
            },
            authorize,
        })
    }

    /// Close only when no accepted work remains. The acknowledgement is sent
    /// after the owner drops its session and kernel journal lease. All cloned
    /// handles become closed under the same lock used for turn admission.
    pub async fn close_if_idle(&self) -> Result<()> {
        {
            let mut admission = self
                .admission
                .lock()
                .map_err(|_| anyhow!("consultation admission lock poisoned"))?;
            ensure!(!admission.closed, "consultation already closing or closed");
            ensure!(
                admission.pending.load(std::sync::atomic::Ordering::SeqCst) == 0,
                "consultation has pending work"
            );
            admission.closed = true;
        }
        let (reply, closed) = oneshot::channel();
        self.tx
            .try_send(OwnerCommand::Close(reply))
            .map_err(|_| anyhow!("consultation owner unavailable during close"))?;
        closed
            .await
            .context("consultation owner stopped without close acknowledgement")
    }

    /// Reserve FIFO capacity and persist input before acknowledging acceptance.
    /// A full queue refuses before the journal write. Dropping the returned
    /// receiver does not revoke an already accepted instruction.
    pub fn enqueue(
        &self,
        turn: ConsultationTurn,
    ) -> Result<oneshot::Receiver<Result<ConsultationAnswer>>> {
        self.enqueue_recorded(turn, None, None)
    }

    fn enqueue_recorded(
        &self,
        turn: ConsultationTurn,
        group: Option<serde_json::Value>,
        authorization: Option<oneshot::Receiver<()>>,
    ) -> Result<oneshot::Receiver<Result<ConsultationAnswer>>> {
        ensure!(
            turn.policy == FormattingPolicy::Max,
            "consultation tools require Max"
        );
        ensure!(!turn.id.trim().is_empty(), "turn identity is required");
        ensure!(
            !turn.text.trim().is_empty() || !turn.attachments.is_empty(),
            "empty consultation turn"
        );
        let admission = self
            .admission
            .lock()
            .map_err(|_| anyhow!("consultation admission lock poisoned"))?;
        ensure!(!admission.closed, "consultation is closing or closed");
        // Reserve before writing; channel pressure must not create orphaned
        // acceptance. This lock preserves journal order and channel send order.
        let permit = self
            .tx
            .try_reserve()
            .map_err(|error| anyhow!("consultation admission failed: {error}"))?;
        let journal = self
            .journal
            .upgrade()
            .context("consultation owner unavailable")?;
        let mut content = vec![ContentBlock::Text(turn.text.clone())];
        content.extend(turn.attachments.iter().map(|image| ContentBlock::Image {
            data: image.data.clone(),
            media_type: image.media_type.clone(),
        }));
        journal.lock().map_err(|_| anyhow!("consultation journal lock poisoned"))?.accept_input(QueuedInstruction {
            turn_id: turn.id.clone(), input: super::Message::new(Role::User, content),
            provider_name: turn.provider_name.clone(), group,
            options: serde_json::json!({ "model": turn.options.model, "system_prompt": turn.options.system_prompt,
                "max_tokens": turn.options.max_tokens, "temperature": turn.options.temperature,
                "reset_chain": turn.options.reset_chain }),
        })?;
        admission
            .pending
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let pending = PendingTurn(Arc::clone(&admission.pending));
        let (reply, receipt) = oneshot::channel();
        permit.send(OwnerCommand::Turn(Box::new(QueuedTurn {
            turn,
            reply,
            pending,
            authorization,
        })));
        drop(journal);
        drop(admission);
        Ok(receipt)
    }
}

struct ConsultationOwner {
    id: String,
    session: AgentSession,
    ui_rx: mpsc::Receiver<AgentUiEvent>,
    gateway: ThreadDeliveryGateway,
    journal: Arc<std::sync::Mutex<ConsultationJournal>>,
    events: ConsultationEvents,
    rx: mpsc::Receiver<OwnerCommand>,
    install_lease_path: PathBuf,
}

async fn run_owner(owner: ConsultationOwner) {
    let ConsultationOwner {
        id,
        mut session,
        mut ui_rx,
        gateway,
        journal,
        events,
        mut rx,
        install_lease_path,
    } = owner;
    while let Some(command) = rx.recv().await {
        let QueuedTurn {
            turn,
            reply,
            pending,
            authorization,
        } = match command {
            OwnerCommand::Turn(turn) => *turn,
            OwnerCommand::Close(reply) => {
                drop(session);
                drop(journal);
                let _ = reply.send(());
                return;
            }
        };
        let recovery_reason = match journal.lock() {
            Ok(journal) => journal.recovery_reason().map(str::to_owned),
            Err(_) => Some("consultation journal lock poisoned".into()),
        };
        if let Some(reason) = recovery_reason {
            drop(pending);
            let _ = reply.send(Err(anyhow!("consultation requires recovery: {reason}")));
            continue;
        }
        // Persistence alone cannot authorize a spoken candidate. Capture may
        // have resumed or changed while preparation was syncing to disk.
        if let Some(authorization) = authorization
            && authorization.await.is_err()
        {
            let discarded = journal
                .lock()
                .map_err(|_| anyhow!("consultation journal lock poisoned"))
                .and_then(|mut journal| journal.discard_unstarted(&turn.id));
            if let Err(ref failure) = discarded
                && let Ok(mut journal) = journal.lock()
            {
                journal.require_recovery(format!("{failure:#}"));
            }
            drop(pending);
            let _ = reply.send(Err(discarded.err().unwrap_or_else(|| {
                anyhow!("consultation candidate rejected before execution")
            })));
            continue;
        }
        // Refuse before any provider or tool work if installation already owns
        // the file. Hold through history/journal settlement and the final reply.
        let _install_lease = match crate::config::acquire_agent_turn_lease_at(&install_lease_path) {
            Ok(lease) => lease,
            Err(error) => {
                let discarded = journal
                    .lock()
                    .map_err(|_| anyhow!("consultation journal lock poisoned"))
                    .and_then(|mut journal| journal.discard_unstarted(&turn.id));
                if let Err(ref failure) = discarded
                    && let Ok(mut journal) = journal.lock()
                {
                    journal.require_recovery(format!("{failure:#}"));
                }
                drop(pending);
                let _ = reply.send(Err(discarded.err().unwrap_or(error)));
                continue;
            }
        };
        let begun = journal
            .lock()
            .map_err(|_| anyhow!("consultation journal lock poisoned"))
            .and_then(|mut journal| journal.begin(&turn.id));
        if let Err(error) = begun {
            if let Ok(mut journal) = journal.lock() {
                journal.require_recovery(format!("{error:#}"));
            }
            drop(pending);
            let _ = reply.send(Err(error));
            continue;
        }
        let turn_id = turn.id.clone();
        let result = run_turn(&id, &mut session, &mut ui_rx, &gateway, &events, turn)
            .await
            .and_then(|answer| {
                journal
                    .lock()
                    .map_err(|_| anyhow!("consultation journal lock poisoned"))?
                    .complete(&turn_id)?;
                Ok(answer)
            });
        if let Err(error) = &result {
            // Do not roll back successful tool effects or automatically retry a
            // partially executed instruction. The host must resolve this state.
            if let Ok(mut journal) = journal.lock() {
                journal.require_recovery(format!("turn {turn_id}: {error:#}"));
            }
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
    let text = session.messages()[history_start..]
        .last()
        .filter(|message| {
            message.role == Role::Assistant
                && message
                    .content
                    .iter()
                    .any(|block| matches!(block, ContentBlock::Text(_)))
        })
        .map(|message| {
            message
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text(text) => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let delivery = gateway
        .deliver(ThreadDeliveryInput {
            backend_id: id.to_string(),
            messages: session.messages().iter().map(ThreadMessage::from).collect(),
            provider: turn.provider_name,
            model: turn.options.model,
            source: ThreadDeliverySource::MaxConsultation,
            mode: "max".into(),
            tags: vec!["agent".into(), "max-consultation".into()],
            timestamp: Utc::now(),
        })
        .context("consultation answer not durably stored")?;
    Ok(ConsultationAnswer {
        turn_id: turn.id,
        text,
        delivery,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentEvent, Message, ThreadStore, ToolDefinition, ToolRegistry};
    use async_trait::async_trait;
    use std::sync::Mutex;
    use tokio::sync::Semaphore;

    fn consultation_speech_evidence(
        end: u64,
    ) -> crate::audio::capture_receipt::AcousticSpeechEvidence {
        use crate::audio::capture_receipt::{
            AcousticAvailability, AcousticSpeechEvidence, CaptureEvidenceIdentity,
        };
        AcousticSpeechEvidence::measured(
            CaptureEvidenceIdentity::new("capture", 1),
            "synthetic-consultation",
            AcousticAvailability::Observed {
                observed_samples: end,
            },
            vec![crate::stt::tail_provider::TailSampleRange {
                session: "capture".into(),
                capture_epoch: 1,
                sample_start: 0,
                sample_end: end,
            }],
        )
    }

    fn repeated_word_ledger(
        last_pending: bool,
    ) -> crate::pipeline::acoustic_ledger::AcousticLedger {
        use crate::pipeline::acoustic_ledger::{
            AcousticEvidence, AcousticLedger, EnergyCalibration, ObservationIdentity,
            ObservationProducer, OccurrenceIdentity,
        };
        let mut ledger = AcousticLedger::new();
        let calibration = EnergyCalibration::new("synthetic-consultation", 1.0, 1);
        for index in 0..5u64 {
            let occurrence =
                OccurrenceIdentity::new("capture", 1, index * 16_000, (index + 1) * 16_000);
            let evidence = AcousticEvidence {
                occurrence: occurrence.clone(),
                duration_ms: 1_000.0,
                energy_integral: 100.0,
                mean_rms_dbfs: -20.0,
                peak_dbfs: -10.0,
                vad_open_sample: Some(occurrence.sample_start),
                vad_close_sample: Some(occurrence.sample_end),
                evidence_calibration_version: calibration.version.clone(),
            };
            assert!(ledger.qualify(&evidence, &calibration).is_qualified());
            let pending = last_pending && index == 4;
            let mut producers = vec![ObservationProducer::Apple];
            if pending {
                producers.push(ObservationProducer::Whisper);
            }
            ledger.schedule_frontier(occurrence.clone(), producers);
            assert!(
                ledger
                    .admit(
                        &ObservationIdentity::new(
                            ObservationProducer::Apple,
                            index,
                            0,
                            occurrence.clone()
                        ),
                        "Iwo",
                    )
                    .grants_mutation()
            );
            ledger.note_frontier_return(&occurrence, ObservationProducer::Apple);
            if !pending {
                ledger.seal(&occurrence).expect("synthetic closed member");
            }
        }
        ledger
    }

    #[test]
    fn sealed_group_preserves_five_distinct_equal_words_and_original_receipts() {
        let ledger = repeated_word_ledger(false);
        let input = SealedConsultationInput::from_ledger(
            &ledger,
            "capture",
            1,
            0..80_000,
            &consultation_speech_evidence(80_000),
        )
        .unwrap()
        .expect("known members are sealed");
        assert_eq!(input.text(), "Iwo Iwo Iwo Iwo Iwo");
        assert_eq!(input.members().len(), 5);
        for (index, member) in input.members().iter().enumerate() {
            assert_eq!(member.occurrence.sample_start, index as u64 * 16_000);
            assert_eq!(
                member.seal_receipt,
                ledger.seal_of(&member.occurrence).unwrap().receipt_id
            );
            assert_eq!(member.source_label, "Iwo");
        }
        assert_eq!(input.session_id(), "capture");
        assert_eq!(input.capture_epoch(), 1);
        assert_eq!(input.samples(), 0..80_000);
        assert_eq!(ledger.len(), 5, "reading must not merge ledger members");
        assert_eq!(
            SealedConsultationInput::from_ledger(
                &ledger,
                "capture",
                1,
                0..80_000,
                &consultation_speech_evidence(80_000)
            )
            .unwrap(),
            Some(input),
        );
    }

    #[test]
    fn sealed_group_waits_for_last_observer_and_refuses_clipped_members() {
        let pending = repeated_word_ledger(true);
        assert!(
            SealedConsultationInput::from_ledger(
                &pending,
                "capture",
                1,
                0..80_000,
                &consultation_speech_evidence(80_000)
            )
            .unwrap()
            .is_none()
        );
        let ledger = repeated_word_ledger(false);
        // A sealed prefix cannot certify speech beyond the last known word.
        let longer = consultation_speech_evidence(96_000);
        assert!(
            SealedConsultationInput::from_ledger(&ledger, "capture", 1, 0..96_000, &longer)
                .unwrap()
                .is_none()
        );
        // Later uncovered speech does not invalidate a fully covered prefix.
        assert!(
            SealedConsultationInput::from_ledger(&ledger, "capture", 1, 0..80_000, &longer)
                .unwrap()
                .is_some()
        );
        assert!(
            SealedConsultationInput::from_ledger(
                &ledger,
                "capture",
                1,
                0..80_000,
                &consultation_speech_evidence(79_999)
            )
            .unwrap()
            .is_none()
        );
        let unavailable = crate::audio::capture_receipt::AcousticSpeechEvidence::unavailable(
            crate::audio::capture_receipt::CaptureEvidenceIdentity::new("capture", 1),
            "synthetic-consultation",
            crate::audio::capture_receipt::AcousticAvailability::NotObserved,
        );
        assert!(
            SealedConsultationInput::from_ledger(&ledger, "capture", 1, 0..80_000, &unavailable)
                .unwrap()
                .is_none()
        );
        assert!(
            SealedConsultationInput::from_ledger(
                &pending,
                "capture",
                1,
                0..64_000,
                &consultation_speech_evidence(80_000)
            )
            .unwrap()
            .is_some()
        );
        assert!(
            SealedConsultationInput::from_ledger(
                &ledger,
                "capture",
                1,
                1..80_000,
                &consultation_speech_evidence(80_000)
            )
            .is_err()
        );
        assert!(
            SealedConsultationInput::from_ledger(
                &ledger,
                "capture",
                1,
                0..79_999,
                &consultation_speech_evidence(80_000)
            )
            .is_err()
        );
        assert!(
            SealedConsultationInput::from_ledger(
                &ledger,
                "capture",
                2,
                0..80_000,
                &consultation_speech_evidence(80_000)
            )
            .unwrap()
            .is_none()
        );
        assert!(
            SealedConsultationInput::from_ledger(
                &ledger,
                "foreign",
                1,
                0..80_000,
                &consultation_speech_evidence(80_000)
            )
            .unwrap()
            .is_none()
        );
        assert!(
            SealedConsultationInput::from_ledger(
                &ledger,
                "capture",
                1,
                10..10,
                &consultation_speech_evidence(80_000)
            )
            .is_err()
        );
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
        assert_eq!(
            queue.ready(&ledger, &speech).unwrap(),
            Some(first.clone()),
            "an unacknowledged read remains retryable after executor pressure"
        );
        // This unit test isolates capture identity/order validation. Runtime
        // admission is exercised by grouped_queue_preserves_source below.
        let acceptance = |input: &SealedConsultationInput| {
            let (_, reply) = oneshot::channel();
            PendingConsultationGroup {
                consultation_id: "fixture".into(),
                input: input.clone(),
                reply,
            }
        };
        let first_accepted = acceptance(&first);
        let mut foreign = ConsultationInputQueue::new("other".into(), 1).unwrap();
        foreign.append_boundary(64_000).unwrap();
        assert!(foreign.acknowledge(&first_accepted).is_err());
        assert_eq!(foreign.pending_groups(), 1);
        queue.acknowledge(&first_accepted).unwrap();
        assert!(
            queue.acknowledge(&first_accepted).is_err(),
            "old acknowledgement cannot consume the next group"
        );
        for _ in 0..3 {
            assert!(queue.ready(&ledger, &speech).unwrap().is_none());
            assert_eq!(
                queue.pending_groups(),
                1,
                "Whisper wait must retain the instruction"
            );
        }
        let last = OccurrenceIdentity::new("capture", 1, 64_000, 80_000);
        ledger.note_frontier_return(&last, ObservationProducer::Whisper);
        ledger.seal(&last).unwrap();
        let recovered = queue.ready(&ledger, &speech).unwrap().unwrap();
        assert_eq!(recovered.text(), "Iwo");
        assert_eq!(recovered.members()[0].occurrence, last);
        queue.acknowledge(&acceptance(&recovered)).unwrap();
        assert_eq!(queue.pending_groups(), 0);
        assert!(queue.ready(&ledger, &speech).unwrap().is_none());
        assert!(!queue.append_boundary(80_000).unwrap());
        assert!(queue.append_boundary(79_999).is_err());

        let mut joined = ConsultationInputQueue::new("capture".into(), 1).unwrap();
        joined.append_boundary(64_000).unwrap();
        assert!(
            joined.join_front().is_err(),
            "waiting for a continuation must keep the first group"
        );
        assert_eq!(joined.pending_groups(), 1);
        joined.append_boundary(80_000).unwrap();
        joined.join_front().unwrap();
        assert!(
            joined.acknowledge(&first_accepted).is_err(),
            "joining invalidates the narrower snapshot"
        );
        let whole = joined.ready(&ledger, &speech).unwrap().unwrap();
        assert_eq!(whole.text(), "Iwo Iwo Iwo Iwo Iwo");
        assert_eq!(whole.members().len(), 5);
        joined.acknowledge(&acceptance(&whole)).unwrap();

        let mut full = ConsultationInputQueue::new("capture".into(), 1).unwrap();
        for boundary in 1..=16 {
            full.append_boundary(boundary).unwrap();
        }
        assert!(full.append_boundary(17).is_err());
        assert_eq!(
            full.pending_groups(),
            16,
            "pressure cannot evict an earlier group"
        );
        full.join_front().unwrap();
        assert!(
            full.append_boundary(17).unwrap(),
            "refused boundary remains retryable"
        );
        assert_eq!(full.pending_groups(), 16);
    }

    #[test]
    fn assessed_admission_refuses_stale_source_and_preserves_continuation() {
        let ledger = repeated_word_ledger(false);
        let speech = consultation_speech_evidence(80_000);
        let mut queue = ConsultationInputQueue::new("capture".into(), 1).unwrap();
        queue.append_boundary(64_000).unwrap();
        let first = queue.ready(&ledger, &speech).unwrap().unwrap();
        // Synthetic transport only: observe the authorization signal itself,
        // not a callback whose behavior could hide premature execution.
        let candidate = |input| {
            let (authorize, observed) = oneshot::channel();
            let (_reply, receipt) = oneshot::channel();
            (
                PreparedConsultationGroup {
                    pending: PendingConsultationGroup {
                        consultation_id: "synthetic".into(),
                        input,
                        reply: receipt,
                    },
                    authorize,
                },
                observed,
            )
        };
        assert_eq!(queue.pending_groups(), 1);

        let mut changed = first.clone();
        changed.text = "different assessed source".into();
        let (prepared, mut observed) = candidate(changed);
        assert!(
            queue
                .authorize_prepared(prepared, &ledger, &speech)
                .is_err()
        );
        assert_eq!(
            observed.try_recv(),
            Err(oneshot::error::TryRecvError::Closed)
        );
        let (prepared, mut observed) = candidate(first.clone());
        assert!(
            queue
                .authorize_prepared(prepared, &ledger, &consultation_speech_evidence(63_999))
                .is_err()
        );
        assert_eq!(
            observed.try_recv(),
            Err(oneshot::error::TryRecvError::Closed)
        );
        let (prepared, observed) = candidate(first.clone());
        drop(observed);
        assert!(
            queue
                .authorize_prepared(prepared, &ledger, &speech)
                .is_err()
        );
        assert_eq!(queue.ready(&ledger, &speech).unwrap(), Some(first.clone()));

        queue.resume_speech();
        assert!(queue.ready(&ledger, &speech).unwrap().is_none());
        assert!(
            !queue.append_boundary(64_000).unwrap(),
            "old tick cannot revive an assessment"
        );
        assert!(queue.append_boundary(63_999).is_err());
        let (prepared, mut observed) = candidate(first.clone());
        assert!(
            queue
                .authorize_prepared(prepared, &ledger, &speech)
                .is_err()
        );
        assert_eq!(
            observed.try_recv(),
            Err(oneshot::error::TryRecvError::Closed)
        );
        queue.append_boundary(80_000).unwrap();
        let (prepared, mut observed) = candidate(first);
        assert!(
            queue
                .authorize_prepared(prepared, &ledger, &speech)
                .is_err()
        );
        assert_eq!(
            observed.try_recv(),
            Err(oneshot::error::TryRecvError::Closed)
        );
        let whole = queue.ready(&ledger, &speech).unwrap().unwrap();
        assert_eq!(whole.text(), "Iwo Iwo Iwo Iwo Iwo");
        assert_eq!(whole.members().len(), 5);
        assert_eq!(ledger.len(), 5);
    }

    #[test]
    fn input_queue_skips_only_measured_empty_intervals() {
        use crate::audio::capture_receipt::{
            AcousticAvailability, AcousticSpeechEvidence, CaptureEvidenceIdentity,
        };
        use crate::pipeline::acoustic_ledger::AcousticLedger;
        use crate::stt::tail_provider::TailSampleRange;
        let ledger = AcousticLedger::new();
        let mut queue = ConsultationInputQueue::new("capture".into(), 1).unwrap();
        queue.append_boundary(16_000).unwrap();
        queue.append_boundary(32_000).unwrap();
        for availability in [
            AcousticAvailability::NotObserved,
            AcousticAvailability::IdentityMismatch,
            AcousticAvailability::Discontinuous {
                observed_samples: 32_000,
            },
            AcousticAvailability::InvalidMeasurement {
                valid_samples: 32_000,
            },
        ] {
            let absent = AcousticSpeechEvidence::unavailable(
                CaptureEvidenceIdentity::new("capture", 1),
                "synthetic",
                availability,
            );
            assert!(!queue.skip_measured_silence(&ledger, &absent));
            assert_eq!(queue.pending_groups(), 2);
        }
        for (session, epoch, extent) in [
            ("foreign", 1, 32_000),
            ("capture", 2, 32_000),
            ("capture", 1, 15_999),
        ] {
            let invalid = AcousticSpeechEvidence::measured(
                CaptureEvidenceIdentity::new(session, epoch),
                "synthetic",
                AcousticAvailability::Observed {
                    observed_samples: extent,
                },
                vec![],
            );
            assert!(!queue.skip_measured_silence(&ledger, &invalid));
            assert_eq!(queue.pending_groups(), 2);
        }
        assert!(
            !queue.skip_measured_silence(&ledger, &consultation_speech_evidence(32_000)),
            "speech without ledger labels is not silence"
        );
        let measured = AcousticSpeechEvidence::measured(
            CaptureEvidenceIdentity::new("capture", 1),
            "synthetic",
            AcousticAvailability::Observed {
                observed_samples: 80_000,
            },
            vec![],
        );
        assert!(
            !queue.skip_measured_silence(&repeated_word_ledger(true), &measured),
            "ledger speech cannot be erased by an observer's empty range list"
        );
        let later_speech = AcousticSpeechEvidence::measured(
            CaptureEvidenceIdentity::new("capture", 1),
            "synthetic",
            AcousticAvailability::Observed {
                observed_samples: 32_000,
            },
            vec![TailSampleRange {
                session: "capture".into(),
                capture_epoch: 1,
                sample_start: 16_000,
                sample_end: 32_000,
            }],
        );
        assert!(queue.skip_measured_silence(&ledger, &later_speech));
        assert_eq!(queue.pending_groups(), 1);
        assert!(!queue.skip_measured_silence(&ledger, &later_speech));
        assert_eq!(
            queue.pending_groups(),
            1,
            "following speech must stay pending"
        );

        let mut silent = ConsultationInputQueue::new("capture".into(), 1).unwrap();
        silent.append_boundary(16_000).unwrap();
        assert!(silent.skip_measured_silence(&ledger, &measured));
        assert_eq!(silent.pending_groups(), 0);
        assert!(!silent.skip_measured_silence(&ledger, &measured));
        assert!(
            !silent.append_boundary(16_000).unwrap(),
            "a repeated boundary cannot reopen consumed silence"
        );
        assert_eq!(
            ledger.len(),
            0,
            "silence advancement must not fabricate a ledger occurrence"
        );
    }

    #[test]
    fn group_presentation_authenticates_members_without_rewriting_acoustic_labels() {
        use crate::pipeline::acoustic_ledger::ConsultationPresentationInput;
        let mut ledger = repeated_word_ledger(false);
        let input = SealedConsultationInput::from_ledger(
            &ledger,
            "capture",
            1,
            0..80_000,
            &consultation_speech_evidence(80_000),
        )
        .unwrap()
        .unwrap();
        let members = input.members();
        let mut bad_label = members.to_vec();
        bad_label[1].source_label = "different".into();
        let mut bad_seal = members.to_vec();
        bad_seal[1].seal_receipt = "forged".into();
        let mut foreign = members.to_vec();
        foreign[1].occurrence.session = "foreign".into();
        let mut epoch = members.to_vec();
        epoch[1].occurrence.capture_epoch = 2;
        let mut reversed = members.to_vec();
        reversed.swap(1, 2);
        let mut omitted = members.to_vec();
        omitted.remove(2);
        let mut duplicated = members.to_vec();
        duplicated.insert(2, members[1].clone());
        for invalid in [
            vec![],
            bad_label,
            bad_seal,
            foreign,
            epoch,
            reversed,
            omitted,
            duplicated,
        ] {
            assert!(
                ledger
                    .record_consultation_presentation(ConsultationPresentationInput {
                        consultation_id: "Max",
                        turn_id: "first",
                        source_revision: 4,
                        revision: 5,
                        members: &invalid,
                        rendered_text: "Prepared command",
                    })
                    .is_err()
            );
            assert!(ledger.consultation_presentations().is_empty());
        }
        for (consultation_id, turn_id, source_revision, revision, rendered_text) in [
            ("", "first", 4, 5, "answer"),
            ("Max", "", 4, 5, "answer"),
            ("Max", "first", 4, 6, "answer"),
            ("Max", "first", 4, 5, " "),
        ] {
            assert!(
                ledger
                    .record_consultation_presentation(ConsultationPresentationInput {
                        consultation_id,
                        turn_id,
                        source_revision,
                        revision,
                        members,
                        rendered_text,
                    })
                    .is_err()
            );
        }
        let mut pending = repeated_word_ledger(true);
        assert!(
            pending
                .record_consultation_presentation(ConsultationPresentationInput {
                    consultation_id: "Max",
                    turn_id: "first",
                    source_revision: 4,
                    revision: 5,
                    members,
                    rendered_text: "answer",
                })
                .is_err()
        );

        let first = ledger
            .record_consultation_presentation(ConsultationPresentationInput {
                consultation_id: "Max",
                turn_id: "first",
                source_revision: 4,
                revision: 5,
                members: &members[..4],
                rendered_text: "Prepared command",
            })
            .unwrap();
        assert_eq!(first.members, members[..4]);
        assert_eq!(first.rendered_text, "Prepared command");
        assert_eq!(ledger.len(), 5);
        for member in members {
            assert_eq!(ledger.text_of(&member.occurrence), Some("Iwo"));
            assert_eq!(
                ledger.seal_of(&member.occurrence).unwrap().receipt_id,
                member.seal_receipt
            );
        }
        for (turn_id, source) in [("first", &members[4..]), ("second", &members[3..])] {
            assert!(
                ledger
                    .record_consultation_presentation(ConsultationPresentationInput {
                        consultation_id: "Max",
                        turn_id,
                        source_revision: 8,
                        revision: 9,
                        members: source,
                        rendered_text: "another answer",
                    })
                    .is_err(),
                "repeated turns and overlapping groups cannot issue a new receipt"
            );
        }
        let second = ledger
            .record_consultation_presentation(ConsultationPresentationInput {
                consultation_id: "Max",
                turn_id: "second",
                source_revision: 8,
                revision: 9,
                members: &members[4..],
                rendered_text: "Second answer",
            })
            .unwrap();
        assert_ne!(first.receipt_id, second.receipt_id);
        assert_eq!(ledger.consultation_presentations(), &[first, second]);
        assert!(ledger.manual_document_revisions().is_empty());
        assert!(ledger.manual_edits().is_empty());
        assert!(ledger.incremental_shapings().is_empty());
    }

    struct ObservedProvider {
        requests: Arc<Mutex<Vec<Vec<Message>>>>,
        entered: Arc<Semaphore>,
        release: Arc<Semaphore>,
        fail: bool,
    }

    #[async_trait]
    impl AgentProvider for ObservedProvider {
        async fn stream(
            &self,
            messages: &[Message],
            _: &[ToolDefinition],
            _: &StreamOptions,
        ) -> Result<mpsc::Receiver<AgentEvent>> {
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
                tx.send(AgentEvent::Error("provider interrupted".into()))
                    .await
                    .expect("event");
            } else {
                tx.send(AgentEvent::TextDone("prepared command".into()))
                    .await
                    .expect("text");
                tx.send(AgentEvent::ResponseDone {
                    response_id: None,
                    clean: true,
                })
                .await
                .expect("done");
            }
            Ok(rx)
        }

        fn build_tool_result(
            &self,
            id: &str,
            content: Vec<ContentBlock>,
            is_error: bool,
        ) -> Message {
            Message::new(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: id.into(),
                    content,
                    is_error,
                }],
            )
        }

        fn build_image_block(&self, data: &[u8], media_type: &str) -> ContentBlock {
            ContentBlock::Image {
                data: data.to_vec(),
                media_type: media_type.into(),
            }
        }

        fn name(&self) -> &str {
            "observed"
        }
    }

    fn turn(id: &str, text: &str) -> ConsultationTurn {
        ConsultationTurn {
            id: id.into(),
            policy: FormattingPolicy::Max,
            text: text.into(),
            attachments: Vec::new(),
            options: StreamOptions::default(),
            provider_name: "observed".into(),
            replacement_provider: None,
        }
    }

    #[tokio::test]
    async fn prepared_group_rejection_never_calls_provider_and_allows_explicit_retry() {
        let dir = tempfile::tempdir().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(1));
        let provider = ObservedProvider {
            requests: Arc::clone(&requests),
            entered: Arc::clone(&entered),
            release,
            fail: false,
        };
        let (ui_tx, ui_rx) = mpsc::channel(2);
        let session = AgentSession::new(Box::new(provider), Arc::new(ToolRegistry::new()), ui_tx);
        let runtime = ConsultationRuntime::start(
            "prepared-history".into(),
            session,
            ui_rx,
            ThreadDeliveryGateway::new_in(dir.path()).unwrap(),
            Arc::new(|_, _, _| {}),
            dir.path().join("agent-turn.lock"),
        )
        .unwrap();
        let ledger = repeated_word_ledger(false);
        let speech = consultation_speech_evidence(80_000);
        let mut queue = ConsultationInputQueue::new("capture".into(), 1).unwrap();
        queue.append_boundary(64_000).unwrap();
        let input = queue.ready(&ledger, &speech).unwrap().unwrap();
        let prepared = runtime
            .prepare_group(
                input.clone(),
                turn(&input.turn_id_for_group(), input.text()),
            )
            .unwrap();
        tokio::task::yield_now().await;
        assert!(requests.lock().unwrap().is_empty());
        assert!(entered.try_acquire().is_err());
        assert!(
            runtime.close_if_idle().await.is_err(),
            "prepared work retains owner lifetime"
        );
        let PreparedConsultationGroup { pending, authorize } = prepared;
        drop(authorize);
        assert!(
            pending.finish().await.is_err(),
            "rejection waits for unstarted journal cleanup"
        );
        assert!(requests.lock().unwrap().is_empty());
        let retained: serde_json::Value = serde_json::from_slice(
            &std::fs::read(dir.path().join("consultations/prepared-history.json")).unwrap(),
        )
        .unwrap();
        assert!(retained["queued"].as_array().unwrap().is_empty());
        let retry = runtime
            .prepare_group(
                input.clone(),
                turn(&input.turn_id_for_group(), input.text()),
            )
            .unwrap();
        // Authorization cannot need either of these locks: capture may hold
        // its ledger while the journal is in use by an unrelated disk write.
        let journal = runtime.journal.upgrade().unwrap();
        let journal_guard = journal.lock().unwrap();
        let admission_guard = runtime.admission.lock().unwrap();
        let accepted = retry.authorize().unwrap();
        drop(admission_guard);
        drop(journal_guard);
        queue.acknowledge(&accepted).unwrap();
        accepted.finish().await.unwrap();
        assert_eq!(requests.lock().unwrap().len(), 1);
        runtime.close_if_idle().await.unwrap();
    }

    #[tokio::test]
    async fn grouped_queue_preserves_source_and_durable_answer_without_reexecution() {
        let dir = tempfile::tempdir().expect("temp directory");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let provider = ObservedProvider {
            requests: Arc::clone(&requests),
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
            fail: false,
        };
        let (ui_tx, ui_rx) = mpsc::channel(2);
        let session = AgentSession::new(Box::new(provider), Arc::new(ToolRegistry::new()), ui_tx);
        let runtime = ConsultationRuntime::start(
            "group-history".into(),
            session,
            ui_rx,
            ThreadDeliveryGateway::new_in(dir.path()).unwrap(),
            Arc::new(|_, _, _| {}),
            dir.path().join("agent-turn.lock"),
        )
        .unwrap();
        let ledger = repeated_word_ledger(false);
        let speech = consultation_speech_evidence(80_000);
        let mut input_queue = ConsultationInputQueue::new("capture".into(), 1).unwrap();
        input_queue.append_boundary(64_000).unwrap();
        input_queue.append_boundary(80_000).unwrap();
        let first = input_queue.ready(&ledger, &speech).unwrap().unwrap();
        assert!(
            runtime
                .prepare_group(first.clone(), turn("wrong", first.text()))
                .is_err()
        );
        assert!(
            runtime
                .prepare_group(
                    first.clone(),
                    turn(&first.turn_id_for_group(), "changed source")
                )
                .is_err()
        );
        assert!(requests.lock().unwrap().is_empty());
        assert_eq!(input_queue.pending_groups(), 2);
        assert_eq!(
            input_queue.ready(&ledger, &speech).unwrap(),
            Some(first.clone())
        );
        let prepared = runtime
            .prepare_group(
                first.clone(),
                turn(&first.turn_id_for_group(), first.text()),
            )
            .unwrap();
        tokio::task::yield_now().await;
        assert!(
            requests.lock().unwrap().is_empty(),
            "persisted candidate has no execution authority"
        );
        let pending = input_queue
            .authorize_prepared(prepared, &ledger, &speech)
            .unwrap();
        assert_eq!(pending.input(), &first);
        input_queue.acknowledge(&pending).unwrap();
        assert_eq!(input_queue.pending_groups(), 1);
        assert!(input_queue.acknowledge(&pending).is_err());
        let second = input_queue.ready(&ledger, &speech).unwrap().unwrap();
        assert_ne!(first.turn_id_for_group(), second.turn_id_for_group());
        entered.acquire().await.unwrap().forget();
        let prepared = runtime
            .prepare_group(
                second.clone(),
                turn(&second.turn_id_for_group(), second.text()),
            )
            .unwrap();
        let later = input_queue
            .authorize_prepared(prepared, &ledger, &speech)
            .unwrap();
        input_queue.acknowledge(&later).unwrap();
        assert_eq!(input_queue.pending_groups(), 0);
        assert_eq!(requests.lock().unwrap().len(), 1);
        let retained: serde_json::Value = serde_json::from_slice(
            &std::fs::read(dir.path().join("consultations/group-history.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            retained["queued"][0]["group"]["members"]
                .as_array()
                .unwrap()
                .len(),
            4
        );
        assert_eq!(
            retained["queued"][1]["group"]["members"][0]["sample_start"],
            64_000
        );
        assert_eq!(
            retained["queued"][1]["group"]["members"][0]["seal_receipt"],
            second.members()[0].seal_receipt
        );
        release.add_permits(1);
        let completed = pending.finish().await.unwrap();
        assert_eq!(completed.input(), &first);
        assert_eq!(completed.answer().turn_id, first.turn_id_for_group());
        assert_eq!(completed.answer().delivery.backend_id, "group-history");
        assert_eq!(completed.answer().delivery.message_count, 2);
        assert_eq!(completed.answer().text, "prepared command");
        // Exercise the sink topology using an actual retained-owner result,
        // not a publicly constructible text-shaped execution receipt.
        use crate::pipeline::contracts::{EngineEvent, EventSink};
        use crate::pipeline::sinks::{CollectorEventSink, FanoutEventSink};
        struct Destination {
            calls: Mutex<Vec<String>>,
            refuse: bool,
        }
        impl EventSink for Destination {
            fn consultation_destinations(&self) -> usize {
                1
            }
            fn on_consultation_completed(&self, completed: &ConsultationGroupAnswer) -> Result<()> {
                self.calls
                    .lock()
                    .unwrap()
                    .push(completed.answer().turn_id.clone());
                ensure!(!self.refuse, "synthetic destination refusal");
                Ok(())
            }
            fn on_event(&self, _: &EngineEvent) {}
        }
        let observer = Arc::new(CollectorEventSink::new());
        assert!(observer.on_consultation_completed(&completed).is_err());
        let zero = FanoutEventSink::new(vec![observer.clone()]);
        assert!(zero.on_consultation_completed(&completed).is_err());
        let destination = Arc::new(Destination {
            calls: Mutex::new(Vec::new()),
            refuse: false,
        });
        let duplicate = FanoutEventSink::pair(destination.clone(), destination.clone());
        assert_eq!(duplicate.consultation_destinations(), 2);
        assert!(duplicate.on_consultation_completed(&completed).is_err());
        let nested = FanoutEventSink::pair(duplicate, destination.clone());
        assert_eq!(nested.consultation_destinations(), 3);
        assert!(nested.on_consultation_completed(&completed).is_err());
        assert!(
            destination.calls.lock().unwrap().is_empty(),
            "validate before first publication"
        );
        let single = FanoutEventSink::pair(observer.clone(), destination.clone());
        let nested_single = FanoutEventSink::pair(observer.clone(), single);
        nested_single.on_consultation_completed(&completed).unwrap();
        assert_eq!(
            *destination.calls.lock().unwrap(),
            vec![first.turn_id_for_group()]
        );
        assert!(
            observer.events().is_empty(),
            "completed answers are not serializable engine events"
        );
        let refusing = Arc::new(Destination {
            calls: Mutex::new(Vec::new()),
            refuse: true,
        });
        assert!(
            FanoutEventSink::pair(observer, refusing.clone())
                .on_consultation_completed(&completed)
                .is_err()
        );
        assert_eq!(refusing.calls.lock().unwrap().len(), 1);
        // Presentation may discard this answer without discarding its history.
        drop(completed);
        let completed = later.finish().await.unwrap();
        assert_eq!(completed.input(), &second);
        assert_eq!(completed.answer().delivery.message_count, 4);
        assert!(
            runtime
                .prepare_group(
                    first.clone(),
                    turn(&first.turn_id_for_group(), first.text())
                )
                .is_err()
        );
        assert_eq!(requests.lock().unwrap().len(), 2);
        let stored = ThreadStore::new_in(dir.path())
            .unwrap()
            .load_thread("group-history")
            .unwrap();
        assert_eq!(stored.messages.len(), 4);
        assert_eq!(ledger.len(), 5);
        for member in first.members().iter().chain(second.members()) {
            assert_eq!(ledger.text_of(&member.occurrence), Some("Iwo"));
        }
        runtime.close_if_idle().await.unwrap();
    }

    #[tokio::test]
    async fn grouped_reply_refuses_foreign_turn_or_history() {
        let ledger = repeated_word_ledger(false);
        let input = SealedConsultationInput::from_ledger(
            &ledger,
            "capture",
            1,
            0..80_000,
            &consultation_speech_evidence(80_000),
        )
        .unwrap()
        .unwrap();
        for (turn_id, backend_id) in [
            ("wrong".to_string(), "selected"),
            (input.turn_id_for_group(), "foreign"),
        ] {
            let (tx, reply) = oneshot::channel();
            tx.send(Ok(ConsultationAnswer {
                turn_id,
                text: "answer".into(),
                delivery: ThreadDeliveryReceipt {
                    backend_id: backend_id.into(),
                    created: true,
                    message_count: 2,
                    updated_at: Utc::now(),
                    first_exchange: true,
                    title_eligible: true,
                },
            }))
            .unwrap();
            let pending = PendingConsultationGroup {
                consultation_id: "selected".into(),
                input: input.clone(),
                reply,
            };
            assert!(pending.finish().await.is_err());
        }
    }

    #[tokio::test]
    async fn queue_preserves_history_and_completes_when_ui_receiver_is_dropped() {
        let dir = tempfile::tempdir().expect("temp directory");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let provider = ObservedProvider {
            requests: Arc::clone(&requests),
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
            fail: false,
        };
        let (ui_tx, ui_rx) = mpsc::channel(2);
        let session = AgentSession::new(Box::new(provider), Arc::new(ToolRegistry::new()), ui_tx);
        let runtime = ConsultationRuntime::start(
            "consultation-a".into(),
            session,
            ui_rx,
            ThreadDeliveryGateway::new_in(dir.path()).expect("gateway"),
            Arc::new(|_, _, _| {}),
            dir.path().join("agent-turn.lock"),
        )
        .expect("runtime");

        let first = runtime
            .enqueue(turn("one", "Prepare a command, do not execute it."))
            .expect("first accepted");
        entered.acquire().await.expect("first entered").forget();
        assert!(
            runtime.close_if_idle().await.is_err(),
            "active instruction cannot be detached"
        );
        drop(first);
        let second = runtime
            .enqueue(turn("two", "Now change only the file name."))
            .expect("second accepted");
        assert_eq!(requests.lock().expect("requests").len(), 1);
        release.add_permits(1);
        let answer = second
            .await
            .expect("owner remains live")
            .expect("second completed");
        assert_eq!(answer.delivery.message_count, 4);
        assert_eq!(answer.delivery.backend_id, "consultation-a");
        {
            let observed = requests.lock().expect("requests");
            assert_eq!(observed.len(), 2);
            assert_eq!(observed[1].len(), 3);
            assert_eq!(observed[1][0].content, observed[0][0].content);
            assert_eq!(observed[1][1].role, Role::Assistant);
        }
        let stored = ThreadStore::new_in(dir.path())
            .expect("store")
            .load_thread("consultation-a")
            .expect("durable history");
        assert_eq!(stored.messages.len(), 4);
        assert_eq!(stored.mode, "max");
        assert!(
            runtime
                .enqueue(turn("one", "Do not repeat tools."))
                .is_err()
        );
        assert_eq!(requests.lock().expect("requests").len(), 2);
        let stale_handle = runtime.clone();
        runtime.close_if_idle().await.expect("idle owner closes");
        assert!(
            stale_handle
                .enqueue(turn("three", "must not resurrect closed owner"))
                .is_err()
        );
        let gateway = ThreadDeliveryGateway::new_in(dir.path()).expect("gateway");
        assert!(
            gateway.open_consultation("consultation-a").is_ok(),
            "close receipt releases journal lease"
        );
    }

    #[tokio::test]
    async fn accepted_waiting_instruction_is_durable_before_provider_execution() {
        let dir = tempfile::tempdir().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let provider = ObservedProvider {
            requests: Arc::clone(&requests),
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
            fail: false,
        };
        let (ui_tx, ui_rx) = mpsc::channel(2);
        let session = AgentSession::new(Box::new(provider), Arc::new(ToolRegistry::new()), ui_tx);
        let runtime = ConsultationRuntime::start(
            "durable-input".into(),
            session,
            ui_rx,
            ThreadDeliveryGateway::new_in(dir.path()).unwrap(),
            Arc::new(|_, _, _| {}),
            dir.path().join("agent-turn.lock"),
        )
        .unwrap();
        let first = runtime
            .enqueue(turn("first", "Prepare a command."))
            .unwrap();
        entered.acquire().await.unwrap().forget();
        let mut correction = turn("second", "Change only the file name to Monika.txt.");
        correction.attachments.push(ImageAttachment {
            data: vec![1, 2, 3],
            media_type: "image/png".into(),
        });
        correction.options.model = "selected-model".into();
        let second = runtime.enqueue(correction).unwrap();
        assert_eq!(
            requests.lock().unwrap().len(),
            1,
            "the second provider call has not started"
        );
        // Read while the first provider is blocked. No sleep, completion or
        // graceful shutdown may be needed to persist an acknowledged input.
        let bytes = std::fs::read(dir.path().join("consultations/durable-input.json")).unwrap();
        let journal: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let queued = journal["queued"]
            .as_array()
            .expect("acknowledged waiting inputs must be durable");
        let waiting = queued
            .iter()
            .find(|entry| entry["turn_id"] == "second")
            .expect("second instruction must survive loss of the in-memory channel");
        let input: Message = serde_json::from_value(waiting["input"].clone()).unwrap();
        assert_eq!(input.role, Role::User);
        assert_eq!(
            input.content,
            vec![
                ContentBlock::Text("Change only the file name to Monika.txt.".into()),
                ContentBlock::Image {
                    data: vec![1, 2, 3],
                    media_type: "image/png".into()
                },
            ]
        );
        assert_eq!(waiting["options"]["model"], "selected-model");
        assert_eq!(journal["pending"], "first");
        release.add_permits(1);
        first.await.unwrap().unwrap();
        second.await.unwrap().unwrap();
        runtime.close_if_idle().await.unwrap();
        let journal: serde_json::Value = serde_json::from_slice(
            &std::fs::read(dir.path().join("consultations/durable-input.json")).unwrap(),
        )
        .unwrap();
        assert!(
            journal["queued"].as_array().unwrap().is_empty(),
            "completed input belongs in canonical history only"
        );
        assert!(
            journal["completed"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("second"))
        );
    }

    #[tokio::test]
    async fn failed_turn_stops_following_work_and_other_policies_cannot_enter() {
        let dir = tempfile::tempdir().expect("temp directory");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (ui_tx, ui_rx) = mpsc::channel(2);
        let session = AgentSession::new(
            Box::new(ObservedProvider {
                requests: Arc::clone(&requests),
                entered: Arc::new(Semaphore::new(0)),
                release: Arc::new(Semaphore::new(1)),
                fail: true,
            }),
            Arc::new(ToolRegistry::new()),
            ui_tx,
        );
        let runtime = ConsultationRuntime::start(
            "consultation-b".into(),
            session,
            ui_rx,
            ThreadDeliveryGateway::new_in(dir.path()).expect("gateway"),
            Arc::new(|_, _, _| {}),
            dir.path().join("agent-turn.lock"),
        )
        .expect("runtime");
        for policy in [
            FormattingPolicy::Off,
            FormattingPolicy::Correction,
            FormattingPolicy::Smart,
        ] {
            let mut request = turn("not-max", "use tools");
            request.policy = policy;
            assert!(runtime.enqueue(request).is_err());
        }
        let first = runtime.enqueue(turn("one", "first")).expect("accepted");
        let second = runtime
            .enqueue(turn("two", "must not execute"))
            .expect("queued");
        assert!(first.await.expect("first reply").is_err());
        assert!(
            second
                .await
                .expect("second reply")
                .expect_err("recovery required")
                .to_string()
                .contains("requires recovery")
        );
        assert_eq!(requests.lock().expect("requests").len(), 1);
        let journal_path = dir.path().join("consultations/consultation-b.json");
        let before = std::fs::read(&journal_path).unwrap();
        let retained: serde_json::Value = serde_json::from_slice(&before).unwrap();
        assert_eq!(retained["queued"].as_array().unwrap().len(), 2);
        assert_eq!(retained["pending"], "one");
        assert!(
            runtime
                .enqueue(turn("three", "must refuse before acknowledgement"))
                .is_err()
        );
        assert_eq!(std::fs::read(&journal_path).unwrap(), before);
        runtime
            .close_if_idle()
            .await
            .expect("failed owner can close without replay");
        let gateway = ThreadDeliveryGateway::new_in(dir.path()).expect("gateway");
        assert!(
            gateway.open_consultation("consultation-b").is_err(),
            "closing must not erase unresolved turn"
        );
    }

    #[tokio::test]
    async fn installer_ownership_refuses_turn_before_provider_execution() {
        use std::os::fd::AsRawFd;
        let dir = tempfile::tempdir().expect("temp directory");
        let lease_path = dir.path().join("agent-turn.lock");
        let installer = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lease_path)
            .expect("installer file");
        // SAFETY: installer retains this valid descriptor until explicit drop.
        assert_eq!(
            unsafe { libc::flock(installer.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (ui_tx, ui_rx) = mpsc::channel(2);
        let session = AgentSession::new(
            Box::new(ObservedProvider {
                requests: Arc::clone(&requests),
                entered: Arc::new(Semaphore::new(0)),
                release: Arc::new(Semaphore::new(1)),
                fail: false,
            }),
            Arc::new(ToolRegistry::new()),
            ui_tx,
        );
        let runtime = ConsultationRuntime::start(
            "installer-case".into(),
            session,
            ui_rx,
            ThreadDeliveryGateway::new_in(dir.path()).expect("gateway"),
            Arc::new(|_, _, _| {}),
            lease_path,
        )
        .expect("runtime");
        let refused = runtime
            .enqueue(turn("one", "prepare command"))
            .expect("queued");
        assert!(refused.await.expect("reply").is_err());
        assert!(requests.lock().expect("requests").is_empty());
        // dup(2) stands in for the fork-to-exec reference. LOCK_UN releases it;
        // close() alone does not.
        let duplicated = unsafe { libc::dup(installer.as_raw_fd()) };
        assert!(duplicated >= 0, "dup: {}", std::io::Error::last_os_error());
        assert_eq!(
            unsafe { libc::flock(installer.as_raw_fd(), libc::LOCK_UN) },
            0,
            "unlock installer: {}",
            std::io::Error::last_os_error()
        );
        drop(installer);
        // No effects were admitted, so explicitly resubmitting this instruction
        // after installation is permitted; the owner does not auto-retry it.
        let accepted = runtime
            .enqueue(turn("one", "prepare command"))
            .expect("queued again");
        assert!(accepted.await.expect("reply").is_ok());
        assert_eq!(requests.lock().expect("requests").len(), 1);
        unsafe { libc::close(duplicated) };
    }
}
