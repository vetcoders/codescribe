//! Voice-assistive agent delivery broadcast.
//!
//! The hotkey / voice-assistive send path streams the agent reply as
//! [`codescribe_core::agent::AgentUiEvent`]s inside
//! [`crate::controller::helpers`]'s `apply_agent_ui_event`. That drain runs in
//! the `codescribe` app crate, which sits *below* the `codescribe-ffi` bridge in
//! the dependency graph (`bridge -> codescribe -> codescribe-core`). It therefore
//! cannot call the Swift-facing UniFFI listener directly.
//!
//! This module owns a process-global `tokio::sync::broadcast` channel that the
//! app publishes turn / delta / tool / done events into; the bridge subscribes
//! and forwards each event onto a `CsAgentDeliveryListener`. It is the exact
//! mirror of the transcription forwarder (`bridge::hotkeys::spawn_event_forwarder`
//! over `RecordingController::subscribe_events`) — same broadcast + `Lagged`/
//! `Closed` handling — but semantically dedicated to AgentChat so it never mixes
//! with the overlay/dictation `CsTranscriptionListener` stream.
//!
//! Legacy AppKit overlay delivery removed the sink that used to render these
//! events; this broadcast is its replacement. Persistence to disk still happens
//! in `apply`'s caller (`persist_runtime_thread`); this channel only carries the
//! *live* render.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use tokio::sync::{broadcast, watch};

/// Broadcast channel capacity. Sized like the transcription IPC channel (256): a
/// burst of token deltas during a fast reply must not lag a slow consumer into a
/// permanent tear-down. `Lagged` is handled gracefully on the bridge forwarder.
const AGENT_DELIVERY_CHANNEL_CAPACITY: usize = 256;

/// One voice-assistive agent delivery event. Provider-agnostic; mirrors the
/// subset of `AgentUiEvent` the chat UI renders, plus a [`TurnStarted`] opener
/// that carries the correlation `thread_id` and the user's transcript so the UI
/// can open a fresh You-bubble + assistant placeholder before the reply streams.
///
/// [`TurnStarted`]: AgentDeliveryEvent::TurnStarted
#[derive(Debug, Clone, PartialEq)]
pub enum AgentDeliveryEvent {
    /// A new voice turn began. `thread_id` is the core runtime's
    /// `thread_store_id` (the persistence id, disjoint from the SwiftUI store's
    /// per-thread `UUID`); `user_text` is the finalized transcript sent to the
    /// agent.
    TurnStarted {
        thread_id: String,
        user_text: String,
    },
    TextDelta(String),
    TextDone(String),
    ReasoningDelta(String),
    ToolExecuting {
        name: String,
        id: String,
    },
    ToolResult {
        name: String,
        id: String,
        summary: String,
        is_error: bool,
    },
    Done,
    Error(String),
    /// The keyed voice turn was explicitly stopped. This is a terminal event,
    /// distinct from provider failure: Swift preserves partial text and settles
    /// pending tools without rendering an error or refreshing persisted history.
    Cancelled {
        thread_id: String,
    },
}

static AGENT_DELIVERY_TX: OnceLock<broadcast::Sender<AgentDeliveryEvent>> = OnceLock::new();
static AGENT_DELIVERY_TURNS: OnceLock<AgentDeliveryTurnRegistry> = OnceLock::new();

#[derive(Default)]
struct AgentDeliveryTurnRegistry {
    turns: Mutex<HashMap<String, Vec<AgentDeliveryTurnEntry>>>,
    next_token: AtomicU64,
}

struct AgentDeliveryTurnEntry {
    token: u64,
    cancel: watch::Sender<bool>,
}

/// Cancellation receiver owned by one controller turn. Registration and cancel
/// lookup use this module's short synchronous mutex, never the shared async
/// `AgentRuntimeState` mutex that remains locked for the full provider/tool send.
pub(crate) struct AgentDeliveryTurnCancellation {
    thread_id: String,
    token: u64,
    cancelled: watch::Receiver<bool>,
    active: bool,
}

impl AgentDeliveryTurnRegistry {
    fn register(&self, thread_id: &str) -> AgentDeliveryTurnCancellation {
        let token = self.next_token.fetch_add(1, Ordering::Relaxed);
        let (cancel, cancelled) = watch::channel(false);
        self.turns
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .entry(thread_id.to_string())
            .or_default()
            .push(AgentDeliveryTurnEntry { token, cancel });
        AgentDeliveryTurnCancellation {
            thread_id: thread_id.to_string(),
            token,
            cancelled,
            active: true,
        }
    }

    fn cancel(&self, thread_id: &str) -> bool {
        let turns = self.turns.lock().unwrap_or_else(|error| error.into_inner());
        let Some(entries) = turns.get(thread_id) else {
            return false;
        };
        for entry in entries {
            entry.cancel.send_replace(true);
        }
        !entries.is_empty()
    }

    /// Atomically close one turn's external cancellation window and report
    /// whether Stop won before the close. `cancel()` takes the same mutex, so
    /// there is no check-then-remove race between provider completion and a
    /// concurrent Swift Stop call.
    fn deregister(&self, thread_id: &str, token: u64) -> bool {
        let mut turns = self.turns.lock().unwrap_or_else(|error| error.into_inner());
        let mut was_cancelled = false;
        if let Some(entries) = turns.get_mut(thread_id) {
            was_cancelled = entries
                .iter()
                .find(|entry| entry.token == token)
                .is_some_and(|entry| *entry.cancel.borrow());
            entries.retain(|entry| entry.token != token);
            if entries.is_empty() {
                turns.remove(thread_id);
            }
        }
        was_cancelled
    }
}

impl AgentDeliveryTurnCancellation {
    pub(crate) async fn cancelled(&mut self) {
        if *self.cancelled.borrow() {
            return;
        }
        loop {
            match self.cancelled.changed().await {
                Ok(()) if *self.cancelled.borrow() => return,
                Ok(()) => continue,
                Err(_) => {
                    // `finish()` deregisters a successful terminal and drops the
                    // sender. Closure is disarm, not cancellation; remain pending
                    // until the sibling send future completes this select.
                    std::future::pending::<()>().await;
                }
            }
        }
    }

    /// Close the external cancellation window before forwarding a successful
    /// terminal. This prevents a late Stop from producing Done + Cancelled.
    pub(crate) fn finish(&mut self) -> bool {
        if !self.active {
            return false;
        }
        let was_cancelled = delivery_turn_registry().deregister(&self.thread_id, self.token);
        self.active = false;
        was_cancelled
    }
}

impl Drop for AgentDeliveryTurnCancellation {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

fn delivery_turn_registry() -> &'static AgentDeliveryTurnRegistry {
    AGENT_DELIVERY_TURNS.get_or_init(AgentDeliveryTurnRegistry::default)
}

pub(crate) fn register_agent_delivery_turn(thread_id: &str) -> AgentDeliveryTurnCancellation {
    delivery_turn_registry().register(thread_id)
}

/// Cancel every active controller-owned voice turn for the exact delivery thread
/// id. Safe and synchronous for UniFFI; returns false when the turn is already
/// terminal or was never registered.
pub fn cancel_agent_delivery_turn(thread_id: &str) -> bool {
    delivery_turn_registry().cancel(thread_id)
}

fn sender() -> &'static broadcast::Sender<AgentDeliveryEvent> {
    AGENT_DELIVERY_TX.get_or_init(|| broadcast::channel(AGENT_DELIVERY_CHANNEL_CAPACITY).0)
}

/// Publish a delivery event to all subscribers. Lock-free and non-blocking.
///
/// A send with no live subscribers returns `Err` (the event is dropped) — that
/// is expected and intentionally ignored: the reply is still persisted to disk
/// by the send path regardless of whether a UI is currently listening.
pub fn publish_agent_delivery_event(event: AgentDeliveryEvent) {
    let _ = sender().send(event);
}

/// Subscribe to the voice-assistive agent delivery stream. The bridge calls this
/// once at startup and spawns a forwarder task that translates each event onto
/// the registered `CsAgentDeliveryListener`.
pub fn subscribe_agent_delivery() -> broadcast::Receiver<AgentDeliveryEvent> {
    sender().subscribe()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast::error::RecvError;

    /// Receive on `rx` until `matches(event)` holds, skipping any interleaved
    /// events from other tests that share this process-global channel. Bounded so
    /// a missing event fails the test instead of hanging.
    async fn recv_until(
        rx: &mut broadcast::Receiver<AgentDeliveryEvent>,
        mut matches: impl FnMut(&AgentDeliveryEvent) -> bool,
    ) -> AgentDeliveryEvent {
        for _ in 0..1024 {
            match rx.recv().await {
                Ok(event) if matches(&event) => return event,
                Ok(_) => continue,
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => panic!("delivery channel closed unexpectedly"),
            }
        }
        panic!("expected event never arrived on the delivery channel");
    }

    #[tokio::test]
    async fn published_turn_started_reaches_a_subscriber() {
        // Unique thread id so a concurrent test on the shared global channel can
        // never satisfy this test's matcher.
        let thread_id = "t_turn_started_reaches_subscriber".to_string();
        let mut rx = subscribe_agent_delivery();
        publish_agent_delivery_event(AgentDeliveryEvent::TurnStarted {
            thread_id: thread_id.clone(),
            user_text: "hello".to_string(),
        });
        let received = recv_until(&mut rx, |event| {
            matches!(event, AgentDeliveryEvent::TurnStarted { thread_id: t, .. } if *t == thread_id)
        })
        .await;
        assert_eq!(
            received,
            AgentDeliveryEvent::TurnStarted {
                thread_id,
                user_text: "hello".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn delta_then_done_preserve_sender_order() {
        // A single sender guarantees per-channel FIFO for its own events even
        // when other tests interleave; assert order across a uniquely-tagged
        // pair by filtering to just this test's markers.
        let tag = "delta_then_done_preserve_sender_order";
        let delta = format!("{tag}:delta");
        let mut rx = subscribe_agent_delivery();
        publish_agent_delivery_event(AgentDeliveryEvent::TextDelta(delta.clone()));
        publish_agent_delivery_event(AgentDeliveryEvent::Error(tag.to_string()));

        let first = recv_until(&mut rx, |event| {
            matches!(event, AgentDeliveryEvent::TextDelta(s) if *s == delta)
                || matches!(event, AgentDeliveryEvent::Error(s) if s == tag)
        })
        .await;
        assert_eq!(first, AgentDeliveryEvent::TextDelta(delta));
        let second = recv_until(
            &mut rx,
            |event| matches!(event, AgentDeliveryEvent::Error(s) if s == tag),
        )
        .await;
        assert_eq!(second, AgentDeliveryEvent::Error(tag.to_string()));
    }

    #[tokio::test]
    async fn keyed_turn_registry_cancels_without_runtime_mutex_and_cleans_up() {
        let thread_id = "registry_cancel_without_runtime_mutex";
        let mut cancellation = register_agent_delivery_turn(thread_id);

        assert!(cancel_agent_delivery_turn(thread_id));
        tokio::time::timeout(std::time::Duration::from_secs(1), cancellation.cancelled())
            .await
            .expect("registered cancellation must wake promptly");

        cancellation.finish();
        assert!(
            !cancel_agent_delivery_turn(thread_id),
            "finished turns must not leave a cancellable registry entry"
        );
    }

    #[test]
    fn publish_without_subscriber_is_silent() {
        // No subscriber attached: the send returns Err internally but the public
        // API must not panic or block (the disk persist path owns durability).
        publish_agent_delivery_event(AgentDeliveryEvent::Error(
            "publish_without_subscriber_is_silent".to_string(),
        ));
    }
}
