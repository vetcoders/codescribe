/// Concrete `DeltaSink` adapters for pipeline consumers.
///
/// - `CallbackSink`: backward-compat bridge wrapping `Arc<dyn Fn(&str) + Send + Sync>`
/// - `CollectorSink`: test helper that collects all deltas
use std::sync::{Arc, Mutex};

use crate::pipeline::contracts::{DeltaSink, TranscriptDelta};

/// Backward-compatible adapter: wraps a plain `Fn(&str)` closure into `DeltaSink`.
pub struct CallbackSink {
    callback: Arc<dyn Fn(&str) + Send + Sync>,
}

impl CallbackSink {
    pub fn new(callback: Arc<dyn Fn(&str) + Send + Sync>) -> Self {
        Self { callback }
    }
}

impl DeltaSink for CallbackSink {
    fn apply(&self, delta: &TranscriptDelta) {
        (self.callback)(&delta.delta);
    }
}

/// Test helper: collects all delta strings for assertions.
pub struct CollectorSink {
    collected: Mutex<Vec<String>>,
}

impl CollectorSink {
    pub fn new() -> Self {
        Self {
            collected: Mutex::new(Vec::new()),
        }
    }

    pub fn collected(&self) -> Vec<String> {
        self.collected.lock().unwrap().clone()
    }
}

impl DeltaSink for CollectorSink {
    fn apply(&self, delta: &TranscriptDelta) {
        self.collected.lock().unwrap().push(delta.delta.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

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

    #[test]
    fn test_collector_sink_collects() {
        let sink = CollectorSink::new();
        sink.apply(&TranscriptDelta::from_raw("one"));
        sink.apply(&TranscriptDelta::from_raw("two"));
        sink.apply(&TranscriptDelta::from_raw("three"));
        assert_eq!(sink.collected(), vec!["one", "two", "three"]);
    }
}
