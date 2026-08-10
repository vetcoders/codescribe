//! Last serving-verdict owner for runtime STT truth.
//!
//! Settings "Active STT" must consume this owner — not project configured
//! `sttEngine` / `finalPassMode`. The controller publishes after each
//! adjudication so the UI can show Apple→Whisper fallback honestly.

use std::sync::{Arc, OnceLock, RwLock};

/// Last engine/mode/disposition that actually served a stop path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastServingVerdict {
    /// Actual engine label (`local_apple`, `local_whisper`, `streaming_whisper`, `cloud_stt`).
    pub engine: String,
    /// Final-pass routing mode that governed the stop (`smart` / `always` / `off`).
    pub routing_mode: String,
    /// Final-pass disposition when one ran (`skipped`, `changed`, …).
    pub disposition: Option<String>,
    /// True when the serving engine was a runtime fallback (e.g. Apple→Whisper).
    pub fallback_used: bool,
}

type ServingStatusSink = Arc<dyn Fn(LastServingVerdict) + Send + Sync + 'static>;

fn store() -> &'static RwLock<Option<LastServingVerdict>> {
    static STORE: OnceLock<RwLock<Option<LastServingVerdict>>> = OnceLock::new();
    STORE.get_or_init(|| RwLock::new(None))
}

fn sink_slot() -> &'static RwLock<Option<ServingStatusSink>> {
    static SINK: OnceLock<RwLock<Option<ServingStatusSink>>> = OnceLock::new();
    SINK.get_or_init(|| RwLock::new(None))
}

/// Register (or replace) a process-local listener for serving-status changes.
/// NOT wired yet: the shipped Settings path is snapshot-on-refresh/panel-entry
/// via UniFFI `current_serving_verdict()` (polling, not push). This sink exists
/// for a future push upgrade; until a caller lands it stays dead code.
#[allow(dead_code)]
pub fn set_serving_status_sink(sink: Option<ServingStatusSink>) {
    let mut guard = sink_slot()
        .write()
        .unwrap_or_else(|error| error.into_inner());
    *guard = sink;
}

/// Publish the last serving verdict after adjudication.
pub fn publish_last_serving(verdict: LastServingVerdict) {
    {
        let mut guard = store().write().unwrap_or_else(|error| error.into_inner());
        *guard = Some(verdict.clone());
    }
    if let Ok(guard) = sink_slot().read()
        && let Some(sink) = guard.as_ref()
    {
        sink(verdict);
    }
}

/// Snapshot the last published serving verdict, if any.
pub fn current_last_serving() -> Option<LastServingVerdict> {
    store()
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

/// Clear the store (tests / recovery).
#[cfg(test)]
pub fn clear_last_serving() {
    let mut guard = store().write().unwrap_or_else(|error| error.into_inner());
    *guard = None;
}

// Label formatting lives Swift-side (`formatActiveSTT` in SettingsViewModel,
// covered by SettingsTruthTests) — one display owner, no duplicate here.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_and_current_roundtrip() {
        clear_last_serving();
        assert!(current_last_serving().is_none());
        publish_last_serving(LastServingVerdict {
            engine: "local_apple".to_string(),
            routing_mode: "smart".to_string(),
            disposition: Some("unchanged".to_string()),
            fallback_used: false,
        });
        let current = current_last_serving().expect("published");
        assert_eq!(current.engine, "local_apple");
        assert_eq!(current.routing_mode, "smart");
        assert_eq!(current.disposition.as_deref(), Some("unchanged"));
        assert!(!current.fallback_used);
        clear_last_serving();
    }
}
