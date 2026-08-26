/// Concrete `DeltaSink` and `EventSink` adapters for pipeline consumers.
///
/// - `CallbackSink`: backward-compat bridge wrapping `Arc<dyn Fn(&str) + Send + Sync>`
/// - `CollectorSink`: test helper that collects all deltas
/// - `CollectorEventSink`: test helper that collects all engine events
use std::sync::{Arc, Mutex};

use crate::pipeline::contracts::{DeltaSink, EngineEvent, EventSink, TranscriptDelta};

/// Backward-compatible adapter: wraps a plain `Fn(&str)` closure into `DeltaSink`.
pub struct CallbackSink {
    callback: Arc<dyn Fn(&str) + Send + Sync>,
}

impl CallbackSink {
    /// Wrap an already-shared closure. See [`from_callback`] to wrap a bare one.
    pub fn new(callback: Arc<dyn Fn(&str) + Send + Sync>) -> Self {
        Self { callback }
    }
}

impl DeltaSink for CallbackSink {
    /// Forward only the delta text; the rest of the [`TranscriptDelta`] is
    /// dropped, since the wrapped closure takes a plain `&str`.
    fn apply(&self, delta: &TranscriptDelta) {
        (self.callback)(&delta.delta);
    }
}

/// Convenience constructor: wrap a plain `Fn(&str)` closure into `Arc<dyn DeltaSink>`.
///
/// This is the recommended entry point for external consumers who just want
/// to receive transcript deltas as `&str` without importing `CallbackSink` directly.
pub fn from_callback<F>(f: F) -> Arc<dyn DeltaSink>
where
    F: Fn(&str) + Send + Sync + 'static,
{
    Arc::new(CallbackSink::new(Arc::new(f)))
}

/// Test helper: collects all delta strings for assertions.
pub struct CollectorSink {
    collected: Mutex<Vec<String>>,
}

impl Default for CollectorSink {
    /// Empty collector — same as [`CollectorSink::new`].
    fn default() -> Self {
        Self::new()
    }
}

impl CollectorSink {
    /// An empty collector.
    pub fn new() -> Self {
        Self {
            collected: Mutex::new(Vec::new()),
        }
    }

    /// Snapshot the deltas seen so far, in arrival order.
    ///
    /// # Panics
    ///
    /// Panics if the lock is poisoned. Deliberate for a test helper: a panicked
    /// producer should fail the assertion rather than be papered over.
    pub fn collected(&self) -> Vec<String> {
        self.collected.lock().unwrap().clone()
    }
}

impl DeltaSink for CollectorSink {
    /// Record the delta text, discarding the rest of the delta.
    fn apply(&self, delta: &TranscriptDelta) {
        self.collected.lock().unwrap().push(delta.delta.clone());
    }
}

// ═══════════════════════════════════════════════════════════
// EventSink adapters
// ═══════════════════════════════════════════════════════════

/// Fan-out sink that forwards each event to multiple sinks.
pub struct FanoutEventSink {
    sinks: Vec<Arc<dyn EventSink>>,
}

impl FanoutEventSink {
    /// Fan out to `sinks`, which are notified in the given order.
    pub fn new(sinks: Vec<Arc<dyn EventSink>>) -> Self {
        Self { sinks }
    }

    /// The two-sink case, already boxed as an `Arc<dyn EventSink>`.
    pub fn pair(a: Arc<dyn EventSink>, b: Arc<dyn EventSink>) -> Arc<dyn EventSink> {
        Arc::new(Self::new(vec![a, b]))
    }
}

impl EventSink for FanoutEventSink {
    /// Deliver to every sink in order, synchronously on the caller's thread.
    ///
    /// No isolation between sinks: a slow one delays the rest, and a panicking
    /// one stops the remaining sinks from seeing the event at all.
    fn on_event(&self, event: &EngineEvent) {
        for sink in &self.sinks {
            sink.on_event(event);
        }
    }
}

/// Test helper: collects all engine events for assertions.
pub struct CollectorEventSink {
    events: Mutex<Vec<EngineEvent>>,
}

impl Default for CollectorEventSink {
    /// Empty event collector — same as [`CollectorEventSink::new`].
    fn default() -> Self {
        Self::new()
    }
}

impl CollectorEventSink {
    /// An empty collector.
    pub fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
        }
    }

    /// Every event seen so far, in arrival order.
    ///
    /// # Panics
    ///
    /// Panics if the lock is poisoned — see [`CollectorSink::collected`] for why
    /// that is the wanted behaviour in a test helper. The same applies to the
    /// filtered accessors below.
    pub fn events(&self) -> Vec<EngineEvent> {
        self.events.lock().unwrap().clone()
    }

    /// Text of every `Preview` event, in order.
    pub fn previews(&self) -> Vec<String> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                EngineEvent::Preview { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// Every `Drop` event as a `(kind, text)` pair — what was suppressed and why.
    pub fn drops(&self) -> Vec<(crate::pipeline::contracts::DropKind, String)> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                EngineEvent::Drop { kind, text, .. } => Some((kind.clone(), text.clone())),
                _ => None,
            })
            .collect()
    }

    /// Text of every `UtteranceFinal` event, in order.
    pub fn finals(&self) -> Vec<String> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                EngineEvent::UtteranceFinal { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }
}

impl EventSink for CollectorEventSink {
    /// Record every event verbatim, with no filtering.
    fn on_event(&self, event: &EngineEvent) {
        self.events.lock().unwrap().push(event.clone());
    }
}

/// Unit tests for delta/event sink adapters, collectors, and fanout.
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// CallbackSink forwards TranscriptDelta text into the wrapped closure.
    #[test]
    fn test_callback_sink_forwards() {
        let received = Arc::new(Mutex::new(String::new()));
        let r = received.clone();
        let sink = CallbackSink::new(Arc::new(move |s: &str| {
            *r.lock().unwrap() = s.to_string();
        }));
        sink.apply(&TranscriptDelta::from_raw("hello"));
        assert_eq!(*received.lock().unwrap(), "hello");
    }

    /// `from_callback` builds an Arc DeltaSink that accumulates successive deltas.
    #[test]
    fn test_from_callback_convenience() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let r = received.clone();
        let sink = super::from_callback(move |s: &str| {
            r.lock().unwrap().push(s.to_string());
        });
        sink.apply(&TranscriptDelta::from_raw("alpha"));
        sink.apply(&TranscriptDelta::from_raw("beta"));
        assert_eq!(*received.lock().unwrap(), vec!["alpha", "beta"]);
    }

    /// CollectorSink records every applied delta string in arrival order.
    #[test]
    fn test_collector_sink_collects() {
        let sink = CollectorSink::new();
        sink.apply(&TranscriptDelta::from_raw("one"));
        sink.apply(&TranscriptDelta::from_raw("two"));
        sink.apply(&TranscriptDelta::from_raw("three"));
        assert_eq!(sink.collected(), vec!["one", "two", "three"]);
    }

    // ── CollectorEventSink ──

    /// CollectorEventSink stores every engine event and filters previews/drops/finals.
    #[test]
    fn test_collector_event_sink_collects_all() {
        let sink = CollectorEventSink::new();
        sink.on_event(&EngineEvent::Preview {
            rev: 1,
            text: "hello".to_string(),
        });
        sink.on_event(&EngineEvent::Drop {
            kind: crate::pipeline::contracts::DropKind::Hallucination,
            text: "thank you".to_string(),
            reason: "pattern".to_string(),
        });
        sink.on_event(&EngineEvent::UtteranceFinal {
            utterance_id: 1,
            text: "hello world".to_string(),
            raw_text: "hello world".to_string(),
            start_ts: 0.0,
            end_ts: 2.0,
            segments: Vec::new(),
            vad_speech_pct: Some(100.0),
            avg_logprob: None,
            compression_ratio: None,
            confidence_flags: Vec::new(),
        });

        assert_eq!(sink.events().len(), 3);
        assert_eq!(sink.previews(), vec!["hello"]);
        assert_eq!(sink.drops().len(), 1);
        assert_eq!(sink.finals(), vec!["hello world"]);
    }

    /// FanoutEventSink delivers each event to every configured child sink.
    #[test]
    fn test_fanout_event_sink_forwards_to_all() {
        let a = Arc::new(CollectorEventSink::new());
        let b = Arc::new(CollectorEventSink::new());
        let fanout = FanoutEventSink::pair(a.clone() as Arc<dyn EventSink>, b.clone());

        fanout.on_event(&EngineEvent::Preview {
            rev: 1,
            text: "hello".to_string(),
        });

        assert_eq!(a.events().len(), 1);
        assert_eq!(b.events().len(), 1);
    }
}
