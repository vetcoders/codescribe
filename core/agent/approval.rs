//! Shared tool approval ownership for chat and consultation hosts.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::{ToolApprovalRequest, ToolOrigin};

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
    /// Original permission preview; snapshots never reconstruct it from text.
    request: ToolApprovalRequest,
    /// Distinguishes this registration from a later use of the same key.
    token: Arc<()>,
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
        use crate::agent::permissions::{AgentPermissions, PermissionLevel};
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
pub struct ApprovalBroker {
    pending: Mutex<HashMap<ApprovalKey, PendingApproval>>,
    changed: tokio::sync::watch::Sender<u64>,
}

impl Default for ApprovalBroker {
    fn default() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            changed: tokio::sync::watch::channel(0).0,
        }
    }
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
    /// Coalescing invalidation signal, not an event log or approval authority.
    /// Subscribers always re-read pending state; late subscribers get an initial
    /// invalidation so an already-pending card cannot remain invisible.
    pub fn subscribe_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        let mut receiver = self.changed.subscribe();
        receiver.mark_changed();
        receiver
    }

    fn notify_changed(&self) {
        self.changed
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }

    /// Park a tool call and hand back the future its execution awaits.
    ///
    /// The future resolves to the user's verdict, or to `false` if the sender is
    /// dropped — an unanswered call is never an implicit allow. A guard inside
    /// the future evicts the entry however it completes, so an abandoned call
    /// leaves no stale row behind.
    pub fn begin(
        self: &Arc<Self>,
        request: ToolApprovalRequest,
    ) -> crate::agent::ToolApprovalFuture {
        let grant_target = match &request.origin {
            ToolOrigin::Mcp {
                server,
                upstream_tool,
            } => GrantTarget::Mcp {
                server: server.clone(),
                upstream_tool: upstream_tool.clone(),
            },
            ToolOrigin::Native => GrantTarget::Native {
                identity: crate::agent::permissions::tool_identity(&request.origin, &request.tool),
            },
        };
        let key = ApprovalKey {
            session_id: request.session_id.clone(),
            thread_id: request.thread_id.clone(),
            call_id: request.call_id.clone(),
        };
        let (tx, rx) = tokio::sync::oneshot::channel();
        let token = Arc::new(());
        {
            let mut pending = recover(self.pending.lock());
            if pending.contains_key(&key) {
                return Box::pin(async { false });
            }
            pending.insert(
                key.clone(),
                PendingApproval {
                    request,
                    tx,
                    grant_target,
                    token: Arc::clone(&token),
                },
            );
            self.notify_changed();
        }
        // Capture the guard before polling: dropping an unpolled future must
        // also evict its registration.
        let guard = PendingApprovalGuard {
            broker: Arc::clone(self),
            key,
            token,
        };
        Box::pin(async move {
            let _guard = guard;
            rx.await.unwrap_or(false)
        })
    }

    /// Snapshot outstanding requests for exactly one conversation. UI recovery
    /// after a missed notification reads this owner rather than replaying tools.
    /// A snapshot grants nothing; resolve still requires an outstanding exact key.
    pub fn pending_for_thread(&self, thread_id: &str) -> Vec<ToolApprovalRequest> {
        let mut requests = recover(self.pending.lock())
            .values()
            .filter(|entry| entry.request.thread_id == thread_id)
            .map(|entry| entry.request.clone())
            .collect::<Vec<_>>();
        requests.sort_by(|a, b| (&a.session_id, &a.call_id).cmp(&(&b.session_id, &b.call_id)));
        requests
    }

    /// Deliver a verdict to the exactly-matching pending call, returning `false`
    /// when none matches. Persisting a remembered grant happens before the call
    /// resumes, so the tool's next invocation cannot race its own grant write.
    pub fn resolve(
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
        self.notify_changed();
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
    pub fn cancel_thread(&self, thread_id: &str) {
        let mut pending = recover(self.pending.lock());
        let keys = pending
            .keys()
            .filter(|key| key.thread_id == thread_id)
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            pending.remove(&key);
        }
        self.notify_changed();
    }
}

/// Evicts a pending approval from the broker when its awaiting future finishes
/// — answered, cancelled, or dropped. Without it, an abandoned call would keep
/// its row forever and a later card could resolve a call nobody is awaiting.
struct PendingApprovalGuard {
    broker: Arc<ApprovalBroker>,
    key: ApprovalKey,
    token: Arc<()>,
}

impl Drop for PendingApprovalGuard {
    /// Remove this call's pending row so a finished awaiter cannot be resolved
    /// again by a stale Swift approval card.
    fn drop(&mut self) {
        let mut pending = recover(self.broker.pending.lock());
        if pending
            .get(&self.key)
            .is_some_and(|entry| Arc::ptr_eq(&entry.token, &self.token))
        {
            pending.remove(&self.key);
            self.broker.notify_changed();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> ToolApprovalRequest {
        ToolApprovalRequest {
            call_id: "call".into(),
            session_id: "session".into(),
            thread_id: "thread".into(),
            tool: "write_file".into(),
            origin: ToolOrigin::Native,
            risk: crate::agent::ToolRisk::Mutating,
            summary: "write a file".into(),
            command: None,
            cwd: None,
            paths: vec![],
        }
    }

    #[tokio::test]
    async fn change_subscription_recovers_late_and_coalesced_notifications() {
        let broker = Arc::new(ApprovalBroker::default());
        let pending = broker.begin(request());
        let mut changes = broker.subscribe_changes();
        assert!(changes.has_changed().unwrap());
        changes.borrow_and_update();
        assert_eq!(broker.pending_for_thread("thread").len(), 1);
        drop(pending);
        let successor = broker.begin(request());
        // Multiple changes may collapse, but the current pending owner survives.
        assert!(changes.has_changed().unwrap());
        changes.borrow_and_update();
        assert_eq!(broker.pending_for_thread("thread").len(), 1);
        drop(successor);
        assert!(changes.has_changed().unwrap());
        assert!(broker.pending_for_thread("thread").is_empty());
    }

    #[tokio::test]
    async fn snapshot_recovers_only_outstanding_requests_in_the_selected_thread() {
        let broker = Arc::new(ApprovalBroker::default());
        let expected = request();
        let first = broker.begin(expected.clone());
        let mut other = expected.clone();
        other.thread_id = "other-thread".into();
        let second = broker.begin(other.clone());
        assert_eq!(broker.pending_for_thread("thread"), vec![expected]);
        assert_eq!(broker.pending_for_thread("other-thread"), vec![other]);
        assert!(broker.pending_for_thread("missing").is_empty());
        // Re-reading is observational: it neither consumes nor approves a call.
        assert_eq!(broker.pending_for_thread("thread").len(), 1);
        assert!(broker.resolve("session", "thread", "call", false, false));
        assert!(!first.await);
        assert!(broker.pending_for_thread("thread").is_empty());
        drop(second);
        assert!(broker.pending_for_thread("other-thread").is_empty());
    }

    #[tokio::test]
    async fn duplicate_registration_cannot_replace_the_original_approval() {
        let broker = Arc::new(ApprovalBroker::default());
        let first = broker.begin(request());
        assert!(!broker.begin(request()).await);
        assert!(broker.resolve("session", "thread", "call", true, false));
        assert!(first.await);
    }

    #[test]
    fn dropping_unpolled_request_removes_its_pending_card() {
        let broker = Arc::new(ApprovalBroker::default());
        drop(broker.begin(request()));
        assert!(!broker.resolve("session", "thread", "call", true, false));
        assert!(recover(broker.pending.lock()).is_empty());
    }

    #[tokio::test]
    async fn old_guard_cannot_evict_a_new_registration_after_cancellation() {
        let broker = Arc::new(ApprovalBroker::default());
        let first = broker.begin(request());
        broker.cancel_thread("thread");
        let second = broker.begin(request());
        assert!(!first.await);
        assert!(broker.resolve("session", "thread", "call", true, false));
        assert!(second.await);
    }
}
