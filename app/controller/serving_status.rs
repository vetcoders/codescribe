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
    /// Route that governed the stop: `live` for the reducer/ledger live lane
    /// (no final pass runs there today), or a final-pass mode
    /// (`smart` / `always` / `off`) when one governs the stop again.
    pub routing_mode: String,
    /// Final-pass disposition when one ran (`skipped`, `changed`, …).
    pub disposition: Option<String>,
    /// True when the serving engine was a runtime fallback (e.g. Apple→Whisper).
    pub fallback_used: bool,
}

/// Route label for a stop served entirely by the live lane.
pub const LIVE_ROUTING_MODE: &str = "live";

impl LastServingVerdict {
    /// Verdict for a take served by the live streaming session.
    ///
    /// `streaming_engine_label` is the recorder's own route label
    /// (`StreamingRecorder::streaming_engine_label`, e.g. `live_apple`). The
    /// live route has one engine and no runtime switch, so `fallback_used` is
    /// false and there is no final-pass disposition. The recorder's `live_*`
    /// vocabulary is folded into the `local_*` engine vocabulary this owner and
    /// the Swift formatter share; any other label passes through verbatim so an
    /// unknown route renders as itself instead of as a guess.
    pub fn from_live_session(streaming_engine_label: &str) -> Self {
        let engine = match streaming_engine_label {
            "live_apple" => "local_apple".to_string(),
            "live_whisper" => "local_whisper".to_string(),
            other => other.to_string(),
        };
        Self {
            engine,
            routing_mode: LIVE_ROUTING_MODE.to_string(),
            disposition: None,
            fallback_used: false,
        }
    }
}

/// Push listener for serving-status changes (see [`set_serving_status_sink`]).
type ServingStatusSink = Arc<dyn Fn(LastServingVerdict) + Send + Sync + 'static>;

/// Process-global slot holding the most recent verdict.
fn store() -> &'static RwLock<Option<LastServingVerdict>> {
    /// Lazy once-cell for the last published serving verdict.
    static STORE: OnceLock<RwLock<Option<LastServingVerdict>>> = OnceLock::new();
    STORE.get_or_init(|| RwLock::new(None))
}

/// Process-global slot holding the optional push listener.
fn sink_slot() -> &'static RwLock<Option<ServingStatusSink>> {
    /// Lazy once-cell for the optional serving-status push listener.
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

/// Serialize tests that read or clear the process-global store. Tests run on
/// parallel threads; without this, one test's `clear_last_serving` lands
/// between another's publish and read. Async-aware so async stop-path tests
/// may hold it across awaits (`lock().await`); sync tests use `blocking_lock`.
#[cfg(test)]
pub fn test_store_lock() -> &'static tokio::sync::Mutex<()> {
    /// Lazy once-cell for the test-only store serialization lock.
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

// Label formatting lives Swift-side (`formatActiveSTT` in SettingsViewModel,
// covered by SettingsTruthTests) — one display owner, no duplicate here.

/// Publish/snapshot owner round-trips for Settings "Active STT".
#[cfg(test)]
mod tests {
    use super::*;

    /// `publish_last_serving` is visible to `current_last_serving` until cleared.
    #[test]
    fn publish_and_current_roundtrip() {
        let _serialized = test_store_lock().blocking_lock();
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

    /// The recorder's `live_apple` route folds into the shared `local_apple`
    /// engine vocabulary; an unknown route label is reported verbatim.
    #[test]
    fn live_session_verdict_folds_recorder_label_into_engine_vocabulary() {
        let apple = LastServingVerdict::from_live_session("live_apple");
        assert_eq!(apple.engine, "local_apple");
        assert_eq!(apple.routing_mode, LIVE_ROUTING_MODE);
        assert_eq!(apple.disposition, None);
        assert!(!apple.fallback_used);

        let unknown = LastServingVerdict::from_live_session("live_moshi");
        assert_eq!(unknown.engine, "live_moshi");
    }
}
