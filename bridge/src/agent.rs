//! Agent streaming surface — thin UniFFI wrapper over the live codescribe
//! `AgentSession` (token/reasoning/tool-call streaming). Moved out of `lib.rs`
//! in W3 cut #0 so each bridge slice owns a disjoint file.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use codescribe_core::agent::{
    AgentSession, AgentUiEvent, ImageAttachment, Message, StreamOptions, ThreadDeliveryGateway,
    ThreadDeliveryInput, ThreadDeliverySource, ThreadMessage, ThreadStore, ToolApprovalHandler,
    ToolApprovalRequest, ToolOrigin, ToolRegistry,
};
use codescribe_core::attachment::{MAX_VISION_IMAGE_BYTES, load_image_for_vision};
use codescribe_core::llm::lane_truth::assistive_identity;
use codescribe_core::llm::provider::provider_supports_vision;
use tokio::task::AbortHandle;

use crate::{CsError, application_runtime};

/// Maximum number of image attachments the composer may forward in one message.
/// Matches the live app controller's `MAX_AGENT_VISION_IMAGES` so both send paths
/// behave alike; exceeding it is surfaced as a readable error rather than a silent
/// truncation.
const MAX_COMPOSER_VISION_IMAGES: usize = 16;

/// One outgoing composer attachment. Path-based on purpose: the bridge reads and
/// validates the file on the Rust side (via `load_image_for_vision`), which is
/// cheaper than marshalling raw image bytes across FFI and reuses core's single
/// vision-loading path. Swift persists clipboard images to disk before handing a
/// path here, so every attachment reduces to a filesystem path.
#[derive(uniffi::Record)]
pub struct CsAttachment {
    /// Absolute filesystem path to the attached image.
    pub path: String,
}

/// Assistive-lane availability for the Swift chat surface: `available` gates
/// the send, `detail` is the honest reason shown in the thread when the lane
/// cannot reach a model (empty when ready).
#[derive(uniffi::Record)]
pub struct CsAgentAvailability {
    /// Whether the assistive lane can reach a model right now.
    pub available: bool,
    /// Actionable reason the lane is unreachable; empty when `available`.
    pub detail: String,
}

/// Approval payload forwarded as one typed FFI record, preserving the exact
/// call/session/thread identity used by the Rust execution gateway.
#[derive(uniffi::Record)]
pub struct CsToolApprovalRequest {
    /// Identifies the specific tool call awaiting a decision. Must be echoed
    /// back verbatim to [`CodescribeAgent::resolve_tool_approval`].
    pub call_id: String,
    /// Agent session the call belongs to; part of the exact-match resolve key.
    pub session_id: String,
    /// Conversation thread the call belongs to; part of the same key.
    pub thread_id: String,
    /// Tool name as the model addressed it.
    pub tool: String,
    /// Originating MCP server, or `"native"` for a built-in tool.
    pub server: String,
    /// Risk tier the permission gate assigned (read-only, mutating, …).
    pub risk: String,
    /// Human-readable description of what the call will do.
    pub summary: String,
    /// Shell command the call would run, when it runs one.
    pub command: Option<String>,
    /// Working directory the call would use, when it declares one.
    pub cwd: Option<String>,
    /// Filesystem paths the call declares it will touch.
    pub paths: Vec<String>,
}

/// Foreign callback trait — agent streaming events forwarded to Swift.
/// Mirrors `AgentUiEvent`; the Swift side must hop these onto the main actor.
#[uniffi::export(with_foreign)]
pub trait CsAgentListener: Send + Sync {
    /// Incremental assistant text as it streams in.
    fn on_text_delta(&self, delta: String);
    /// Final assembled assistant text for the turn.
    fn on_text_done(&self, text: String);
    /// Incremental reasoning text, when the model emits it.
    fn on_reasoning_delta(&self, delta: String);
    /// A tool call started executing.
    fn on_tool_executing(&self, name: String, id: String);
    /// A tool call is blocked awaiting the user's decision. The turn stays
    /// suspended until [`CodescribeAgent::resolve_tool_approval`] answers.
    fn on_tool_approval_requested(&self, request: CsToolApprovalRequest);
    /// A tool call finished; `is_error` marks a failed one.
    fn on_tool_result(&self, name: String, id: String, summary: String, is_error: bool);
    /// The turn completed successfully. Not emitted for a cancelled turn.
    fn on_done(&self);
    /// A non-fatal error worth surfacing. Provider failures and cancellation
    /// arrive through the call's `Err` instead, never doubled here.
    fn on_error(&self, message: String);
}

/// Thin handle to the codescribe agent engine.
#[derive(uniffi::Object, Default, Clone)]
pub struct CodescribeAgent {
    /// In-flight turns keyed by thread id, so `cancel_turn` can abort them.
    /// Shared (`Arc`) because each turn's RAII guard must be able to deregister
    /// itself even while the FFI object stays borrowed by other calls.
    turns: Arc<TurnRegistry>,
    /// Tool calls suspended awaiting a user decision, keyed by
    /// session + thread + call.
    approvals: Arc<ApprovalBroker>,
}

#[uniffi::export]
impl CodescribeAgent {
    /// Construct the FFI handle. Only initialises logging — provider, tools and
    /// config are resolved lazily per send, so building the Swift app model
    /// never triggers a Keychain prompt.
    #[uniffi::constructor]
    pub fn new() -> Self {
        codescribe::logging::init_logging();
        Self::default()
    }

    /// True when the assistive lane can currently reach a provider. Resolved
    /// fresh on every call (settings → env → Keychain via lane_truth), so a
    /// Settings save flips this on the very next send — no restart, no stale
    /// bootstrap env. A key-optional local endpoint counts as available.
    pub fn is_available(&self) -> bool {
        // Warm settings + Keychain only when the agent surface is actually used.
        // Constructing the Swift app model must not trigger a keychain prompt.
        let _ = codescribe_core::config::Config::load();
        codescribe::agent::assistive_unavailable_reason().is_none()
    }

    /// Availability of the assistive lane as one record: `available` mirrors
    /// [`Self::is_available`]; `detail` carries the actionable, user-facing
    /// reason when the lane cannot reach a model (which lane, endpoint or key
    /// is missing — never a generic "add an API key"). Empty when ready.
    pub fn availability(&self) -> CsAgentAvailability {
        let _ = codescribe_core::config::Config::load();
        match codescribe::agent::assistive_unavailable_reason() {
            None => CsAgentAvailability {
                available: true,
                detail: String::new(),
            },
            Some(detail) => CsAgentAvailability {
                available: false,
                detail,
            },
        }
    }

    /// Generate an isolated one-shot title from raw first-turn text using the
    /// formatting lane. The core call has its own 8-second timeout and never
    /// participates in the assistive or formatting response chains.
    pub async fn generate_thread_title(&self, text: String) -> Result<Option<String>, CsError> {
        application_runtime::run(async move {
            Ok(codescribe_core::llm::ai_formatting::generate_thread_title(&text).await?)
        })
        .await?
    }

    /// Stream one agent reply for `text` on the conversation identified by
    /// `thread_id`, forwarding token/reasoning/tool events to `listener` as they
    /// arrive. Returns the final assembled assistant text.
    ///
    /// Memory + persistence: prior turns stored under `thread_id` are restored
    /// into the session before sending (so the model sees the conversation
    /// history), and the updated thread is written back after a successful reply
    /// so the SwiftUI app's conversations survive restart. Persistence is
    /// best-effort: a load/save failure never fails the reply the user already
    /// saw.
    ///
    /// Full native tool set + MCP are registered, so the agent can actually act
    /// (clipboard, selection, screenshot, filesystem, typing, github, search,
    /// transcribe). Tools execute on demand when the model calls them.
    pub async fn stream_reply(
        &self,
        text: String,
        thread_id: String,
        listener: Arc<dyn CsAgentListener>,
    ) -> Result<String, CsError> {
        let agent = self.clone();
        application_runtime::run(async move {
            agent
                .run_stream(text, thread_id, Vec::new(), listener)
                .await
        })
        .await?
    }

    /// Stream one agent reply for `text` with `attachments` forwarded as real
    /// vision input (the composer 📎 path). Attachments are path-based; the bridge
    /// loads + validates each one via core's single `load_image_for_vision` path
    /// (PNG/JPEG/GIF/WebP/BMP/TIFF, ≤ 8 MB each) so the send never routes raw
    /// bytes through FFI and never produces a second attachment pipeline.
    ///
    /// Degradation is explicit, never a silent drop:
    /// - the selected model is not vision-capable ⇒ readable error, nothing sent;
    /// - any attachment is missing / unsupported / too large / empty ⇒ readable
    ///   error naming the offending file(s), nothing sent;
    /// - more than 16 images ⇒ readable error.
    pub async fn stream_reply_with_attachments(
        &self,
        text: String,
        thread_id: String,
        attachments: Vec<CsAttachment>,
        listener: Arc<dyn CsAgentListener>,
    ) -> Result<String, CsError> {
        let agent = self.clone();
        application_runtime::run(async move {
            let images = validate_composer_attachments(&attachments)?;
            agent.run_stream(text, thread_id, images, listener).await
        })
        .await?
    }

    /// Abort the in-flight turn(s) for `thread_id`. Returns `true` when an
    /// active turn was found and aborted, `false` when the thread was idle
    /// (the call is a safe no-op then).
    ///
    /// This explicit call is the ONLY working cancel path from Swift: the
    /// generated UniFFI Swift bindings poll a Rust future to completion and
    /// never propagate Swift `Task` cancellation (`uniffiRustCallAsync` has no
    /// `rust_future_cancel` wiring), so cancelling the Swift task alone leaves
    /// the turn — and its tool side effects — running.
    ///
    /// The abort lands on the turn task's next `.await` point (tokio abort
    /// semantics): an in-flight tool future is dropped there, so side effects
    /// scheduled after that point never run; a synchronous section already
    /// executing finishes its current poll segment first. The aborted turn is
    /// NOT persisted — the thread on disk keeps its last completed-turn state,
    /// so the next turn on the same thread restores clean history.
    pub fn cancel_turn(&self, thread_id: String) -> bool {
        self.approvals.cancel_thread(&thread_id);
        self.turns.cancel(&thread_id)
    }

    /// Answer a pending tool-approval request, resuming the suspended call.
    /// Returns `false` when no call matches — the identity must match on all
    /// three of session, thread and call id, so a stale card cannot resume a
    /// different call.
    ///
    /// `remember` persists an always-allow grant for the tool before the call
    /// resumes; a failed write downgrades to allow-once rather than to a deny.
    pub fn resolve_tool_approval(
        &self,
        session_id: String,
        thread_id: String,
        call_id: String,
        approved: bool,
        remember: bool,
    ) -> bool {
        self.approvals
            .resolve(&session_id, &thread_id, &call_id, approved, remember)
    }
}

impl CodescribeAgent {
    /// Shared streaming core behind [`stream_reply`] and
    /// [`stream_reply_with_attachments`]. `attachments` are already loaded +
    /// validated `ImageAttachment`s (empty for the text-only path).
    async fn run_stream(
        &self,
        text: String,
        thread_id: String,
        attachments: Vec<ImageAttachment>,
        listener: Arc<dyn CsAgentListener>,
    ) -> Result<String, CsError> {
        // Keep provider construction behavior identical to the old eager
        // constructor path, but delay it until the user sends a message.
        let config = codescribe_core::config::Config::load();
        let provider = codescribe::agent::create_default_provider()?;
        let mut registry = ToolRegistry::new();
        codescribe::agent::tools::register_all_tools(&mut registry);
        // settings.json agent.permissions + legacy tool_grants (always-allow).
        registry.set_policy(
            codescribe_core::agent::permissions::AgentPermissions::load()
                .with_legacy_grants(codescribe_core::agent::tool_grants::load_granted()),
        );
        registry.enable_policy_hot_reload();
        let (ui_tx, ui_rx) = tokio::sync::mpsc::channel::<AgentUiEvent>(64);
        let approvals = Arc::clone(&self.approvals);
        let approval_handler: ToolApprovalHandler =
            Arc::new(move |request| approvals.begin(request));
        let mut session = AgentSession::new(provider, Arc::new(registry), ui_tx)
            .with_tool_approval(thread_id.clone(), approval_handler);

        // Restore prior turns for cross-turn memory. ThreadStore does blocking
        // fs I/O, so the load runs on a blocking pool thread and is awaited
        // before the agent loop starts. A missing/corrupt thread yields an empty
        // history (best-effort: a first turn simply has nothing to restore).
        let thread_id_for_load = thread_id.clone();
        let restored: Vec<Message> = tokio::task::spawn_blocking(move || {
            ThreadStore::new()
                .ok()
                .and_then(|store| store.load_thread(&thread_id_for_load).ok())
                .map(|thread| {
                    thread
                        .messages
                        .iter()
                        .map(ThreadMessage::to_message)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
        .await
        .unwrap_or_default();
        if !restored.is_empty() {
            // Seeds the conversation history; resets the provider chain id to
            // None (the persistence id is `thread_id`, separate from the
            // provider's response-chain id).
            session.restore_messages(restored);
        }

        // Honor the same assistive system prompt + token cap the in-app
        // controller path uses (build_agent_stream_options), so a Swift chat send
        // is not stripped of the WORKSPACE-augmented assistive prompt and the
        // configured `ai_assistive_max_tokens`.
        let options = build_bridge_stream_options(config.ai_assistive_max_tokens);

        let turn = PreparedTurn {
            session,
            text,
            attachments,
            options,
            ui_rx,
        };
        let (final_text, messages) =
            drive_turn(turn, listener, Arc::clone(&self.turns), thread_id.clone()).await?;

        // Persist the updated thread (best-effort). The reply already streamed
        // to the user, so a save failure is logged-and-ignored rather than
        // surfaced as an error. A cancelled turn never reaches this point on
        // purpose: its partial messages are discarded, so the thread on disk
        // keeps the last completed-turn state (today's only cancel trigger is
        // thread deletion, where persisting would resurrect the thread).
        deliver_completed_thread(thread_id, messages).await;
        Ok(final_text)
    }
}

/// Everything a spawned agent turn needs, bundled so [`drive_turn`] stays a
/// single testable unit (tests build one from a scripted provider + mock tools).
struct PreparedTurn {
    /// Agent session with history already restored and tools registered.
    session: AgentSession,
    /// The user's message for this turn.
    text: String,
    /// Vision attachments, already loaded and validated.
    attachments: Vec<ImageAttachment>,
    /// Resolved stream options (system prompt, token cap).
    options: StreamOptions,
    /// Receiving end of the session's UI event channel; its closure is what
    /// terminates the forwarding loop in [`drive_turn`].
    ui_rx: tokio::sync::mpsc::Receiver<AgentUiEvent>,
}

/// Spawn the agent loop for one turn, forward its UI events to `listener`, and
/// join the task for the final message log.
///
/// Cancellation contract (2.15):
/// - The spawned task is tied to this future through a [`TurnGuard`]: if this
///   future is dropped mid-turn, the task is aborted at its next `.await` point
///   instead of running detached to completion (the pre-fix bug: tools kept
///   typing/pasting after a "cancelled" turn).
/// - The guard also registers the task in `turns`, so an explicit
///   [`CodescribeAgent::cancel_turn`] can abort it by thread id.
/// - An aborted turn surfaces as a readable `Err` and hands back no messages,
///   so the caller never persists a half-finished turn.
/// - A turn that already completed cannot be broken retroactively: aborting a
///   finished tokio task is a documented no-op and the join below still yields
///   its result.
async fn drive_turn(
    turn: PreparedTurn,
    listener: Arc<dyn CsAgentListener>,
    turns: Arc<TurnRegistry>,
    thread_id: String,
) -> Result<(String, Vec<Message>), CsError> {
    let PreparedTurn {
        session,
        text,
        attachments,
        options,
        mut ui_rx,
    } = turn;

    // Drive the agent loop on a task so the channel closes when it finishes,
    // letting the drain loop below terminate cleanly. The task hands back the
    // session's final message log so the caller can persist the thread.
    let send_handle = tokio::spawn(async move {
        let mut session = session;
        let attachments = attachments;
        session.send(text, attachments, &options).await?;
        Ok::<Vec<Message>, anyhow::Error>(session.messages().to_vec())
    });
    let _turn_guard = turns.register(&thread_id, send_handle.abort_handle());

    let mut final_text = String::new();
    while let Some(event) = ui_rx.recv().await {
        match event {
            AgentUiEvent::TextDelta(delta) => listener.on_text_delta(delta),
            AgentUiEvent::TextDone(t) => {
                final_text = t.clone();
                listener.on_text_done(t);
            }
            AgentUiEvent::ReasoningDelta(delta) => listener.on_reasoning_delta(delta),
            AgentUiEvent::ToolExecuting { name, id } => listener.on_tool_executing(name, id),
            AgentUiEvent::ToolApprovalRequested(request) => {
                let server = match &request.origin {
                    ToolOrigin::Native => "native".to_string(),
                    ToolOrigin::Mcp { server, .. } => server.clone(),
                };
                listener.on_tool_approval_requested(CsToolApprovalRequest {
                    call_id: request.call_id,
                    session_id: request.session_id,
                    thread_id: request.thread_id,
                    tool: request.tool,
                    server,
                    risk: request.risk.as_str().to_string(),
                    summary: request.summary,
                    command: request.command,
                    cwd: request.cwd,
                    paths: request.paths,
                });
            }
            AgentUiEvent::ToolResult {
                name,
                id,
                summary,
                is_error,
            } => listener.on_tool_result(name, id, summary, is_error),
            AgentUiEvent::Done => listener.on_done(),
            AgentUiEvent::Error(message) => listener.on_error(message),
        }
    }

    match send_handle.await {
        Ok(Ok(messages)) => Ok((final_text, messages)),
        Ok(Err(error)) => Err(CsError::Agent {
            msg: error.to_string(),
        }),
        Err(join_error) if join_error.is_cancelled() => Err(CsError::Agent {
            msg: "Turn cancelled".to_string(),
        }),
        Err(join_error) => Err(CsError::Agent {
            msg: format!("agent task join error: {join_error}"),
        }),
    }
}

/// Exact identity of one suspended tool call. All three components participate
/// in equality: a decision must not resume a same-named call on another thread
/// or from an earlier session.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ApprovalKey {
    session_id: String,
    thread_id: String,
    call_id: String,
}

/// A tool call parked awaiting the user's decision.
struct PendingApproval {
    /// Resumes the suspended call with the verdict. Dropping this sender
    /// instead resolves the call to `false` — the fail-closed path used when a
    /// thread is cancelled.
    tx: tokio::sync::oneshot::Sender<bool>,
    /// Where an "always allow" for this call is persisted.
    grant_target: GrantTarget,
}

/// Durable target for the approval card's "remember" checkbox. Native tools now
/// reach the gate too (review P1-06), so they need a persist path of their own —
/// otherwise "always allow" would silently do nothing and re-ask every turn.
enum GrantTarget {
    /// MCP: writes `agent.permissions.tools[server:tool]` and dual-writes
    /// `tool_grants.json` so the existing revoke UI stays truthful.
    Mcp {
        server: String,
        upstream_tool: String,
    },
    /// Native: writes `agent.permissions.tools[native:<name>]` only — there is
    /// no upstream server to grant against.
    Native { identity: String },
}

impl GrantTarget {
    /// Write the always-allow grant to its durable home.
    fn persist(&self) -> anyhow::Result<()> {
        use codescribe_core::agent::permissions::{AgentPermissions, PermissionLevel};
        match self {
            Self::Mcp {
                server,
                upstream_tool,
            } => AgentPermissions::remember_allow(server, upstream_tool),
            Self::Native { identity } => {
                AgentPermissions::set_tool_level(identity, PermissionLevel::Allow)
            }
        }
    }

    /// Log-friendly identifier for the grant target (`server:tool`, or the
    /// native tool identity).
    fn label(&self) -> String {
        match self {
            Self::Mcp {
                server,
                upstream_tool,
            } => format!("{server}:{upstream_tool}"),
            Self::Native { identity } => identity.clone(),
        }
    }
}

/// Suspension point between the Rust tool gateway and the Swift approval card.
///
/// Holds every call awaiting a decision. Shared behind an `Arc` because a
/// pending call's own guard must be able to evict its entry after the FFI
/// object has moved on.
#[derive(Default)]
struct ApprovalBroker {
    pending: Mutex<HashMap<ApprovalKey, PendingApproval>>,
}

/// Recover a poisoned lock instead of unwinding across the FFI boundary. A
/// panicking `expect` here aborts the whole SwiftUI host — for bookkeeping maps
/// whose worst-case damage is a stale entry, that trade is wrong (review
/// P3-11). Recovery is also fail-closed for safety: a lost pending approval
/// resolves to "not approved", never to an implicit allow.
fn recover<'a, T>(
    result: Result<MutexGuard<'a, T>, PoisonError<MutexGuard<'a, T>>>,
) -> MutexGuard<'a, T> {
    result.unwrap_or_else(PoisonError::into_inner)
}

impl ApprovalBroker {
    /// Park a tool call and hand back the future its execution awaits.
    ///
    /// The future resolves to the user's verdict, or to `false` if the sender is
    /// dropped — an unanswered call is never an implicit allow. A guard inside
    /// the future evicts the entry however it completes, so an abandoned call
    /// leaves no stale row behind.
    fn begin(
        self: &Arc<Self>,
        request: ToolApprovalRequest,
    ) -> codescribe_core::agent::ToolApprovalFuture {
        let grant_target = match &request.origin {
            ToolOrigin::Mcp {
                server,
                upstream_tool,
            } => GrantTarget::Mcp {
                server: server.clone(),
                upstream_tool: upstream_tool.clone(),
            },
            ToolOrigin::Native => GrantTarget::Native {
                identity: codescribe_core::agent::permissions::tool_identity(
                    &request.origin,
                    &request.tool,
                ),
            },
        };
        let key = ApprovalKey {
            session_id: request.session_id,
            thread_id: request.thread_id,
            call_id: request.call_id,
        };
        let (tx, rx) = tokio::sync::oneshot::channel();
        recover(self.pending.lock()).insert(key.clone(), PendingApproval { tx, grant_target });
        let broker = Arc::clone(self);
        Box::pin(async move {
            let _guard = PendingApprovalGuard {
                broker: Arc::clone(&broker),
                key: key.clone(),
            };
            rx.await.unwrap_or(false)
        })
    }

    /// Deliver a verdict to the exactly-matching pending call, returning `false`
    /// when none matches. Persisting a remembered grant happens before the call
    /// resumes, so the tool's next invocation cannot race its own grant write.
    fn resolve(
        &self,
        session_id: &str,
        thread_id: &str,
        call_id: &str,
        approved: bool,
        remember: bool,
    ) -> bool {
        let key = ApprovalKey {
            session_id: session_id.to_string(),
            thread_id: thread_id.to_string(),
            call_id: call_id.to_string(),
        };
        let Some(entry) = recover(self.pending.lock()).remove(&key) else {
            return false;
        };
        // Persist BEFORE resuming the call so a granted tool never races its
        // own next invocation against the write. Grant failure downgrades to
        // allow-once (the approval itself was explicit), never to a deny.
        // Persist BEFORE resuming so a remembered grant never races the next
        // identical call. Writes settings.json agent.permissions.tools[key]
        // (product source of truth) and dual-writes tool_grants.json.
        if approved
            && remember
            && let Err(error) = entry.grant_target.persist()
        {
            tracing::warn!(
                %error,
                target = entry.grant_target.label(),
                "tool grant persist failed; allowing once"
            );
        }
        entry.tx.send(approved).is_ok()
    }

    /// Drop every approval pending on `thread_id`. Each dropped sender resolves
    /// its call to "not approved", so cancelling a thread can never leave a tool
    /// waiting on a card the user will never see again.
    fn cancel_thread(&self, thread_id: &str) {
        let mut pending = recover(self.pending.lock());
        let keys = pending
            .keys()
            .filter(|key| key.thread_id == thread_id)
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            pending.remove(&key);
        }
    }
}

/// Evicts a pending approval from the broker when its awaiting future finishes
/// — answered, cancelled, or dropped. Without it, an abandoned call would keep
/// its row forever and a later card could resolve a call nobody is awaiting.
struct PendingApprovalGuard {
    broker: Arc<ApprovalBroker>,
    key: ApprovalKey,
}

impl Drop for PendingApprovalGuard {
    /// Remove this call's pending row so a finished awaiter cannot be resolved
    /// again by a stale Swift approval card.
    fn drop(&mut self) {
        recover(self.broker.pending.lock()).remove(&self.key);
    }
}

/// In-flight turn bookkeeping behind [`CodescribeAgent::cancel_turn`].
///
/// One thread id can briefly hold several entries (the composer allows firing a
/// new send while a previous one is draining), so entries carry a unique token:
/// `cancel` aborts every turn on the thread, while each turn's guard removes
/// only its own entry on completion.
#[derive(Default)]
struct TurnRegistry {
    /// In-flight turns per thread id.
    turns: Mutex<HashMap<String, Vec<TurnEntry>>>,
    /// Monotonic source of the per-entry token.
    next_token: AtomicU64,
}

/// One in-flight turn: a unique token plus the handle that aborts it.
struct TurnEntry {
    /// Distinguishes concurrent turns on the same thread, so a completing turn
    /// removes only its own entry.
    token: u64,
    /// Aborts the spawned turn task at its next `.await`.
    abort: AbortHandle,
}

impl TurnRegistry {
    /// Track a spawned turn task and return the RAII guard that owns both the
    /// abort-on-drop semantics and the registry entry's lifetime.
    fn register(self: &Arc<Self>, thread_id: &str, abort: AbortHandle) -> TurnGuard {
        let token = self.next_token.fetch_add(1, Ordering::Relaxed);
        recover(self.turns.lock())
            .entry(thread_id.to_string())
            .or_default()
            .push(TurnEntry {
                token,
                abort: abort.clone(),
            });
        TurnGuard {
            registry: Arc::clone(self),
            thread_id: thread_id.to_string(),
            token,
            abort,
        }
    }

    /// Remove one turn's entry by token, dropping the thread's row once it holds
    /// no more turns. Called from [`TurnGuard::drop`], never directly.
    fn deregister(&self, thread_id: &str, token: u64) {
        let mut turns = recover(self.turns.lock());
        if let Some(entries) = turns.get_mut(thread_id) {
            entries.retain(|entry| entry.token != token);
            if entries.is_empty() {
                turns.remove(thread_id);
            }
        }
    }

    /// Abort every in-flight turn on `thread_id`; `false` when idle. Entries are
    /// left in place — each aborted turn's guard deregisters it as the turn's
    /// `drive_turn` future unwinds (aborting an already-finished task is a
    /// no-op, so a turn that completed just before this call is unaffected).
    fn cancel(&self, thread_id: &str) -> bool {
        let turns = recover(self.turns.lock());
        let Some(entries) = turns.get(thread_id) else {
            return false;
        };
        for entry in entries {
            entry.abort.abort();
        }
        !entries.is_empty()
    }
}

/// RAII guard tying a spawned turn task to the [`drive_turn`] future that owns
/// it. Dropping the guard — on normal completion, on error, or because the
/// UniFFI-held future was dropped — aborts the task (no-op once it finished)
/// and removes its registry entry, so cancelled and completed turns never leak
/// stale abort handles.
struct TurnGuard {
    /// Registry this guard removes its entry from on drop.
    registry: Arc<TurnRegistry>,
    /// Thread the guarded turn belongs to.
    thread_id: String,
    /// Token identifying this turn's entry.
    token: u64,
    /// Abort handle fired on drop; a no-op once the task has finished.
    abort: AbortHandle,
}

impl Drop for TurnGuard {
    /// Abort the spawned turn task (no-op if finished) and drop its registry
    /// entry so cancelled and completed turns never leak abort handles.
    fn drop(&mut self) {
        self.abort.abort();
        self.registry.deregister(&self.thread_id, self.token);
    }
}

/// Build the assistive stream options for a bridge chat send, honoring the same
/// assistive system prompt and token cap the in-app controller path uses
/// (`app/controller/helpers.rs::build_agent_stream_options`). Model is left empty
/// so the provider resolves it from `LLM_ASSISTIVE_MODEL` (identical default to
/// the controller), keeping both send paths behaviorally aligned.
fn build_bridge_stream_options(ai_assistive_max_tokens: i32) -> StreamOptions {
    let max_tokens = u32::try_from(ai_assistive_max_tokens)
        .ok()
        .filter(|tokens| *tokens > 0);
    StreamOptions {
        model: String::new(),
        system_prompt: Some(compose_agent_system_prompt()),
        max_tokens,
        temperature: None,
        reset_chain: false,
    }
}

/// Compose the agent system prompt exactly like the controller path
/// (`app/controller/helpers.rs::compose_agent_system_prompt`): the base assistive
/// prompt, the WORKSPACE section (6238ca1) that pins project roots and tells the
/// model to resolve names via `list_projects` instead of guessing paths, the
/// review-tool + connector doctrine for long-running MCP review calls and
/// GitHub-connector fallback, and the measured Responses/streaming API ground
/// truth with the answer-first rule (operator incident 2026-08-14: a spoken
/// engine question got a clarification questionnaire instead of an answer).
fn compose_agent_system_prompt() -> String {
    let base = codescribe_core::config::prompts::get_assistive_prompt();
    let workspace = codescribe::agent::tools::workspace::workspace_prompt_section();
    let doctrine = codescribe::agent::tools::doctrine::review_doctrine_prompt_section();
    let api_truth = codescribe::agent::tools::api_truth::responses_api_prompt_section();
    format!("{base}\n\n{workspace}\n\n{doctrine}\n\n{api_truth}")
}

/// Load + validate composer attachments into vision `ImageAttachment`s.
///
/// All-or-nothing on purpose: a partial success would silently drop images the
/// user chose to attach. Any failure returns a readable [`CsError`] naming the
/// offending files so the composer surfaces it instead of sending a quietly
/// degraded message. Also gates on the selected model's vision capability.
fn validate_composer_attachments(
    attachments: &[CsAttachment],
) -> Result<Vec<ImageAttachment>, CsError> {
    if attachments.is_empty() {
        return Ok(Vec::new());
    }

    if attachments.len() > MAX_COMPOSER_VISION_IMAGES {
        return Err(CsError::Agent {
            msg: format!(
                "Too many images ({}). Attach at most {} per message.",
                attachments.len(),
                MAX_COMPOSER_VISION_IMAGES
            ),
        });
    }

    // Vision gate: refuse (readable error) rather than silently drop the images
    // when the configured assistive model cannot read them. Lane identity comes
    // from lane_truth (fresh settings), not the frozen bootstrap env.
    let config = codescribe_core::config::Config::load();
    let (provider, model) = assistive_identity(&config);
    if !provider_supports_vision(provider, &model) {
        return Err(CsError::Agent {
            msg: "The selected model can't read images. Switch to a vision-capable \
                  model in Settings, or remove the attachment before sending."
                .to_string(),
        });
    }

    let mut images = Vec::with_capacity(attachments.len());
    let mut failed: Vec<String> = Vec::new();
    for attachment in attachments {
        let path = std::path::Path::new(&attachment.path);
        match load_image_for_vision(path, MAX_VISION_IMAGE_BYTES) {
            Some((data, media_type)) => images.push(ImageAttachment { data, media_type }),
            None => failed.push(attachment_label(&attachment.path)),
        }
    }

    if !failed.is_empty() {
        return Err(CsError::Agent {
            msg: format!(
                "Couldn't attach {}: image must be PNG, JPEG, GIF, WebP, BMP, or \
                 TIFF and 8 MB or smaller.",
                failed.join(", ")
            ),
        });
    }

    Ok(images)
}

/// Short, user-facing label (file name, path fallback) for an attachment path.
fn attachment_label(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// Deliver the completed composer turn through core's single durable gateway.
/// Blocking filesystem work stays off the async executor and remains
/// best-effort because the reply has already reached the user.
async fn deliver_completed_thread(thread_id: String, messages: Vec<Message>) {
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<_>> {
        // `now` is sourced from the freshest message timestamp the session
        // stamped (`Some(Utc::now())` per turn), avoiding a direct `chrono`
        // dependency in the bridge crate. With nothing to anchor the thread to,
        // skip the write.
        let Some(now) = messages.iter().rev().find_map(|message| message.timestamp) else {
            return Ok(None);
        };

        let config = codescribe_core::config::Config::load();
        let (provider, model) = assistive_identity(&config);
        let persisted_messages = messages
            .iter()
            .map(|message| {
                let mut persisted = ThreadMessage::from(message);
                if message.timestamp.is_none() {
                    persisted.timestamp = now;
                }
                persisted
            })
            .collect::<Vec<_>>();

        let receipt = ThreadDeliveryGateway::new()?.deliver(ThreadDeliveryInput {
            backend_id: thread_id,
            messages: persisted_messages,
            provider: provider.as_str().to_string(),
            model,
            source: ThreadDeliverySource::Composer,
            mode: "assistive".to_string(),
            tags: vec!["agent".to_string(), "overlay".to_string()],
            timestamp: now,
        })?;
        Ok(Some(receipt))
    })
    .await;

    match result {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => {
            eprintln!("Failed to deliver agent thread (best-effort): {error}");
        }
        Err(error) => {
            eprintln!("Agent thread delivery task failed (best-effort): {error}");
        }
    }
}

/// Unit coverage for composer attachment validation, turn cancellation, and
/// the approval broker — exercises the real `drive_turn` path with a scripted
/// provider so no live model is required.
#[cfg(test)]
mod tests {
    use super::*;

    /// Per-process scratch directory for attachment fixtures, namespaced by pid
    /// and `tag` so concurrent test binaries do not collide.
    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cs_bridge_attach_{}_{tag}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    /// Wrap a path as the composer attachment record.
    fn cs(path: &std::path::Path) -> CsAttachment {
        CsAttachment {
            path: path.to_string_lossy().into_owned(),
        }
    }

    /// An empty attachment list must yield zero vision payloads, not invent one.
    #[test]
    fn empty_attachments_yield_no_images() {
        let images = validate_composer_attachments(&[]).unwrap();
        assert!(images.is_empty());
    }

    /// A readable PNG path loads through core vision loading into one attachment.
    #[test]
    fn valid_image_loads_as_vision_attachment() {
        let dir = tmp_dir("valid");
        let png = dir.join("shot.png");
        std::fs::write(&png, b"\x89PNG\r\n\x1a\nfake").unwrap();

        let images = validate_composer_attachments(&[cs(&png)]).unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].media_type, "image/png");
        assert!(!images[0].data.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Bad paths surface named, readable errors — never a silent skip of the file.
    #[test]
    fn unreadable_or_nonimage_is_a_readable_error_not_a_silent_drop() {
        let dir = tmp_dir("bad");
        let txt = dir.join("note.txt");
        std::fs::write(&txt, b"hello").unwrap();
        let missing = dir.join("gone.png");

        let err = validate_composer_attachments(&[cs(&txt), cs(&missing)]).unwrap_err();
        let CsError::Agent { msg } = err else {
            panic!("expected a readable agent error");
        };
        assert!(
            msg.contains("note.txt"),
            "names the unsupported file: {msg}"
        );
        assert!(msg.contains("gone.png"), "names the missing file: {msg}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Exceeding `MAX_COMPOSER_VISION_IMAGES` is rejected before any image loads.
    #[test]
    fn too_many_images_is_rejected() {
        let attachments: Vec<CsAttachment> = (0..=MAX_COMPOSER_VISION_IMAGES)
            .map(|i| CsAttachment {
                path: format!("/tmp/x{i}.png"),
            })
            .collect();
        let err = validate_composer_attachments(&attachments).unwrap_err();
        let CsError::Agent { msg } = err else {
            panic!("expected a readable agent error");
        };
        assert!(msg.contains("Too many"), "explains the cap: {msg}");
    }

    // ── Turn cancellation (2.15) ─────────────────────────────────────────

    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::time::Duration;

    use codescribe_core::agent::{
        AgentEvent, AgentProvider, ContentBlock, Role, ToolDefinition, ToolResultContent,
    };

    /// Provider that replays one scripted event batch per `stream` call —
    /// the same shape core's session tests use, local to the bridge so these
    /// tests exercise the real `drive_turn` unit without a live provider.
    struct ScriptedProvider {
        scripts: Mutex<VecDeque<Vec<AgentEvent>>>,
    }

    impl ScriptedProvider {
        /// Queue one event batch per expected `stream` call, in order. A call
        /// past the end of the script yields an empty batch.
        fn new(scripts: Vec<Vec<AgentEvent>>) -> Self {
            Self {
                scripts: Mutex::new(scripts.into()),
            }
        }
    }

    #[async_trait::async_trait]
    impl AgentProvider for ScriptedProvider {
        /// Pop one scripted event batch and feed it through a channel, matching
        /// the real provider stream shape without hitting the network.
        async fn stream(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &StreamOptions,
        ) -> anyhow::Result<tokio::sync::mpsc::Receiver<AgentEvent>> {
            let events = self
                .scripts
                .lock()
                .expect("script lock should not be poisoned")
                .pop_front()
                .unwrap_or_default();
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            for event in events {
                tx.send(event)
                    .await
                    .expect("test stream channel should accept scripted event");
            }
            Ok(rx)
        }

        /// Build a user tool-result message for a completed call, as live
        /// providers do when feeding the next model turn.
        fn build_tool_result(
            &self,
            call_id: &str,
            content: Vec<ContentBlock>,
            is_error: bool,
        ) -> Message {
            Message::new(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: call_id.to_string(),
                    content,
                    is_error,
                }],
            )
        }

        /// Wrap raw image bytes as a content block for vision-capable turns.
        fn build_image_block(&self, data: &[u8], media_type: &str) -> ContentBlock {
            ContentBlock::Image {
                data: data.to_vec(),
                media_type: media_type.to_string(),
            }
        }

        /// Stable provider label used in diagnostics and test logs.
        fn name(&self) -> &str {
            "scripted-provider"
        }
    }

    /// Listener that only records that a tool started executing — the signal
    /// the cancellation tests key their cancel timing on.
    #[derive(Default)]
    struct RecordingListener {
        /// Set once a tool call begins — the cancellation tests wait on this
        /// before cancelling, so the cancel lands mid-tool rather than by luck.
        tool_started: AtomicBool,
        /// Successful final texts seen; must stay 0 for a cancelled turn.
        text_done_count: AtomicUsize,
        /// `Done` terminals seen; must stay 0 for a cancelled turn.
        done_count: AtomicUsize,
        /// Errors delivered through the listener. Provider failures and
        /// cancellation arrive via the call's `Err`, so this staying 0 is what
        /// proves they are not double-signalled.
        error_count: AtomicUsize,
    }

    impl CsAgentListener for RecordingListener {
        /// Streaming text deltas are ignored; cancellation tests key on tools.
        fn on_text_delta(&self, _delta: String) {}
        /// Count successful final text emissions (must stay 0 after cancel).
        fn on_text_done(&self, _text: String) {
            self.text_done_count.fetch_add(1, Ordering::SeqCst);
        }
        /// Reasoning deltas are unused by these cancellation fixtures.
        fn on_reasoning_delta(&self, _delta: String) {}
        /// Arm `tool_started` so cancel tests wait until the tool is mid-flight.
        fn on_tool_executing(&self, _name: String, _id: String) {
            self.tool_started.store(true, Ordering::SeqCst);
        }
        /// Approval requests are not asserted by the cancellation suite.
        fn on_tool_approval_requested(&self, _request: CsToolApprovalRequest) {}
        /// Tool result payloads are not asserted by the cancellation suite.
        fn on_tool_result(&self, _name: String, _id: String, _summary: String, _is_error: bool) {}
        /// Count successful Done terminals (must stay 0 after cancel).
        fn on_done(&self) {
            self.done_count.fetch_add(1, Ordering::SeqCst);
        }
        /// Count listener errors; provider failures must use throw-only instead.
        fn on_error(&self, _message: String) {
            self.error_count.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Registry with one tool whose observable side effect fires only AFTER
    /// `delay` — the stand-in for typing/clipboard/fs effects that a cancelled
    /// turn must never execute.
    fn slow_tool_registry(side_effect: Arc<AtomicBool>, delay: Duration) -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry
            .register_native(
                ToolDefinition {
                    name: "slow_side_effect".to_string(),
                    description: "test tool with a delayed side effect".to_string(),
                    input_schema: serde_json::json!({"type": "object", "properties": {}}),
                },
                Box::new(move |_input| {
                    let side_effect = Arc::clone(&side_effect);
                    Box::pin(async move {
                        tokio::time::sleep(delay).await;
                        side_effect.store(true, Ordering::SeqCst);
                        vec![ToolResultContent::Text("side effect done".to_string())]
                    })
                }),
                // Read-only keeps the permission gate out of the way: these
                // tests prove cancel drops an in-flight tool future, and the
                // prepared session has no approval handler attached.
                codescribe_core::agent::ToolRisk::ReadOnly,
            )
            .expect("registering the test tool must succeed");
        registry
    }

    /// Script driving one slow tool call; the second batch is only consumed
    /// when the turn survives to iteration 2 (i.e. was NOT cancelled).
    fn tool_turn_script() -> Vec<Vec<AgentEvent>> {
        vec![
            vec![
                AgentEvent::ToolCallReady {
                    id: "call_1".to_string(),
                    name: "slow_side_effect".to_string(),
                    arguments: serde_json::json!({}),
                },
                AgentEvent::ResponseDone {
                    response_id: Some("resp_1".to_string()),
                    clean: true,
                },
            ],
            vec![
                AgentEvent::TextDone("late full run".to_string()),
                AgentEvent::ResponseDone {
                    response_id: Some("resp_2".to_string()),
                    clean: true,
                },
            ],
        ]
    }

    /// Single-batch script for a plain text turn with no tool calls.
    fn text_turn_script(reply: &str) -> Vec<Vec<AgentEvent>> {
        vec![vec![
            AgentEvent::TextDone(reply.to_string()),
            AgentEvent::ResponseDone {
                response_id: Some("resp_text".to_string()),
                clean: true,
            },
        ]]
    }

    /// Neutral stream options: no system prompt, no caps — these tests assert
    /// turn lifecycle, not prompt composition.
    fn test_options() -> StreamOptions {
        StreamOptions {
            model: String::new(),
            system_prompt: None,
            max_tokens: None,
            temperature: None,
            reset_chain: false,
        }
    }

    /// Assemble a [`PreparedTurn`] over a scripted provider, so [`drive_turn`]
    /// is exercised as the real unit without a live provider.
    fn scripted_turn(
        scripts: Vec<Vec<AgentEvent>>,
        registry: ToolRegistry,
        text: &str,
    ) -> PreparedTurn {
        let (ui_tx, ui_rx) = tokio::sync::mpsc::channel(64);
        let session = AgentSession::new(
            Box::new(ScriptedProvider::new(scripts)),
            Arc::new(registry),
            ui_tx,
        );
        PreparedTurn {
            session,
            text: text.to_string(),
            attachments: Vec::new(),
            options: test_options(),
            ui_rx,
        }
    }

    /// Poll `flag` until set or `timeout` elapses, returning its final value.
    /// Lets a test synchronise on the tool actually running instead of sleeping
    /// a guessed interval.
    async fn wait_until_set(flag: &AtomicBool, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        while tokio::time::Instant::now() < deadline {
            if flag.load(Ordering::SeqCst) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        flag.load(Ordering::SeqCst)
    }

    /// Provider stream errors throw once as `CsError::Agent` and must not also
    /// land on `on_error` (no double-signalling to Swift).
    #[tokio::test]
    async fn provider_error_is_reported_once_via_throw_only() {
        let listener = Arc::new(RecordingListener::default());
        let turns = Arc::new(TurnRegistry::default());
        let turn = scripted_turn(
            vec![vec![AgentEvent::Error("upstream exploded".to_string())]],
            ToolRegistry::new(),
            "fail once",
        );

        let result = drive_turn(
            turn,
            Arc::clone(&listener) as Arc<dyn CsAgentListener>,
            Arc::clone(&turns),
            "thread-provider-error".to_string(),
        )
        .await;

        let result_error_count = usize::from(result.is_err());
        let CsError::Agent { msg } = result.expect_err("provider error must throw") else {
            panic!("provider error must surface as CsError::Agent");
        };
        assert!(
            msg.contains("Provider stream error: upstream exploded"),
            "provider failure reason should survive the thrown error: {msg}"
        );
        assert_eq!(
            listener.error_count.load(Ordering::SeqCst) + result_error_count,
            1,
            "provider errors must not double-signal through on_error and throw"
        );
        assert!(
            !turns.cancel("thread-provider-error"),
            "errored turn should deregister itself"
        );
    }

    /// Cancelling mid-tool aborts before the delayed side effect, surfaces a
    /// readable cancel error, and leaves the thread ready for a next turn.
    #[tokio::test]
    async fn cancel_turn_aborts_in_flight_tool_before_its_side_effect() {
        let side_effect = Arc::new(AtomicBool::new(false));
        let listener = Arc::new(RecordingListener::default());
        let turns = Arc::new(TurnRegistry::default());

        let turn = scripted_turn(
            tool_turn_script(),
            slow_tool_registry(Arc::clone(&side_effect), Duration::from_millis(500)),
            "cancel me",
        );
        let driven = tokio::spawn(drive_turn(
            turn,
            Arc::clone(&listener) as Arc<dyn CsAgentListener>,
            Arc::clone(&turns),
            "thread-cancel".to_string(),
        ));

        assert!(
            wait_until_set(&listener.tool_started, Duration::from_secs(5)).await,
            "tool should start executing before we cancel"
        );
        assert!(
            turns.cancel("thread-cancel"),
            "an active turn should be cancellable"
        );

        let result = driven.await.expect("driving task must not panic");
        let CsError::Agent { msg } = result.expect_err("a cancelled turn must not report success")
        else {
            panic!("cancellation must surface as an agent error");
        };
        assert!(
            msg.contains("cancelled"),
            "cancel surfaces as a readable cancellation: {msg}"
        );
        assert_eq!(
            listener.text_done_count.load(Ordering::SeqCst),
            0,
            "a cancelled turn must not emit a successful final text"
        );
        assert_eq!(
            listener.done_count.load(Ordering::SeqCst),
            0,
            "a cancelled turn must not emit the successful Done terminal"
        );
        assert_eq!(
            listener.error_count.load(Ordering::SeqCst),
            0,
            "cancellation is returned once through the async result, not double-signalled"
        );

        // Wait well past the tool's own delay: the side effect must never fire
        // because the tool future was dropped at the abort point.
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert!(
            !side_effect.load(Ordering::SeqCst),
            "cancelled tool must not run its side effect"
        );
        assert!(
            !turns.cancel("thread-cancel"),
            "the aborted turn must deregister itself"
        );

        // The same thread accepts the next turn after an abort: a fresh session
        // (as run_stream builds per send) completes normally.
        let next = scripted_turn(text_turn_script("recovered"), ToolRegistry::new(), "again");
        let (final_text, messages) = drive_turn(
            next,
            Arc::clone(&listener) as Arc<dyn CsAgentListener>,
            Arc::clone(&turns),
            "thread-cancel".to_string(),
        )
        .await
        .expect("the thread must keep working after a cancelled turn");
        assert_eq!(final_text, "recovered");
        assert!(messages.iter().any(|m| m.role == Role::Assistant));
    }

    /// Dropping the `drive_turn` future (UniFFI cancel path) must abort the
    /// inner task via `TurnGuard`, not leave a tool running detached.
    #[tokio::test]
    async fn dropping_the_turn_future_aborts_the_spawned_task() {
        let side_effect = Arc::new(AtomicBool::new(false));
        let listener = Arc::new(RecordingListener::default());
        let turns = Arc::new(TurnRegistry::default());

        let turn = scripted_turn(
            tool_turn_script(),
            slow_tool_registry(Arc::clone(&side_effect), Duration::from_millis(500)),
            "drop me",
        );
        let driven = tokio::spawn(drive_turn(
            turn,
            Arc::clone(&listener) as Arc<dyn CsAgentListener>,
            Arc::clone(&turns),
            "thread-drop".to_string(),
        ));

        assert!(
            wait_until_set(&listener.tool_started, Duration::from_secs(5)).await,
            "tool should start executing before the future is dropped"
        );

        // Dropping the drive_turn future (what a cancelled UniFFI call does)
        // must abort the inner turn task via the guard, not leave it detached.
        driven.abort();
        let join_error = driven
            .await
            .expect_err("aborted future should not yield a value");
        assert!(join_error.is_cancelled());

        tokio::time::sleep(Duration::from_millis(700)).await;
        assert!(
            !side_effect.load(Ordering::SeqCst),
            "a dropped turn future must not leave the tool running detached"
        );
        assert!(
            !turns.cancel("thread-drop"),
            "the guard must deregister the turn when the future is dropped"
        );
    }

    /// Cancel on an idle thread is a quiet false — no panic on registry or FFI.
    #[test]
    fn cancel_with_no_active_turn_is_a_noop() {
        let turns = TurnRegistry::default();
        assert!(!turns.cancel("idle-thread"));

        // Same through the FFI surface object (no panic, returns false).
        let agent = CodescribeAgent::default();
        assert!(!agent.cancel_turn("idle-thread".to_string()));
    }

    /// Resolve must match session + thread + call exactly; wrong keys stay pending.
    #[tokio::test]
    async fn approval_broker_resumes_only_the_exact_session_thread_and_call() {
        let broker = Arc::new(ApprovalBroker::default());
        let request = ToolApprovalRequest {
            call_id: "call-exact".to_string(),
            session_id: "session-exact".to_string(),
            thread_id: "thread-exact".to_string(),
            tool: "mcp__desktop-commander__write_file".to_string(),
            origin: ToolOrigin::Mcp {
                server: "desktop-commander".to_string(),
                upstream_tool: "write_file".to_string(),
            },
            risk: codescribe_core::agent::ToolRisk::Mutating,
            summary: "write workspace file".to_string(),
            command: None,
            cwd: None,
            paths: vec!["/workspace/file".to_string()],
        };
        let pending = broker.begin(request);
        assert!(!broker.resolve("session-exact", "wrong-thread", "call-exact", true, false));
        assert!(!broker.resolve("wrong-session", "thread-exact", "call-exact", true, false));
        assert!(broker.resolve("session-exact", "thread-exact", "call-exact", true, false));
        assert!(pending.await);
    }

    /// Dropping a thread's pending approvals resolves each awaiter as denied.
    #[tokio::test]
    async fn cancelling_thread_rejects_pending_approval() {
        let broker = Arc::new(ApprovalBroker::default());
        let pending = broker.begin(ToolApprovalRequest {
            call_id: "call-cancel".to_string(),
            session_id: "session-cancel".to_string(),
            thread_id: "thread-cancel".to_string(),
            tool: "mcp__desktop-commander__write_file".to_string(),
            origin: ToolOrigin::Mcp {
                server: "desktop-commander".to_string(),
                upstream_tool: "write_file".to_string(),
            },
            risk: codescribe_core::agent::ToolRisk::Mutating,
            summary: "write workspace file".to_string(),
            command: None,
            cwd: None,
            paths: vec!["/workspace/file".to_string()],
        });
        broker.cancel_thread("thread-cancel");
        assert!(!pending.await);
    }

    /// A cancel after successful completion is a no-op; the finished text stands
    /// and the thread still accepts a subsequent turn.
    #[tokio::test]
    async fn completed_turn_is_not_broken_by_a_late_cancel() {
        let listener = Arc::new(RecordingListener::default());
        let turns = Arc::new(TurnRegistry::default());

        let turn = scripted_turn(text_turn_script("all done"), ToolRegistry::new(), "first");
        let (final_text, messages) = drive_turn(
            turn,
            Arc::clone(&listener) as Arc<dyn CsAgentListener>,
            Arc::clone(&turns),
            "thread-seq".to_string(),
        )
        .await
        .expect("uncancelled turn completes");
        assert_eq!(final_text, "all done");
        assert!(messages.iter().any(|m| m.role == Role::Assistant));

        // A cancel arriving after completion finds nothing to abort — the
        // finished result above stays intact (no retroactive corruption).
        assert!(!turns.cancel("thread-seq"));

        // And the thread still accepts the next turn.
        let next = scripted_turn(text_turn_script("all done"), ToolRegistry::new(), "second");
        let (final_text, _) = drive_turn(
            next,
            Arc::clone(&listener) as Arc<dyn CsAgentListener>,
            turns,
            "thread-seq".to_string(),
        )
        .await
        .expect("next turn on the same thread must work");
        assert_eq!(final_text, "all done");
    }
}
