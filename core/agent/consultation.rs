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
