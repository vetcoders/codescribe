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
/// Bridge / Settings wire this to push Active STT updates without polling.
#[allow(dead_code)] // consumed by bridge once UniFFI serving-status surface lands
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

/// Human-readable Active STT row. Never projects configured preference.
pub fn format_active_stt_label(verdict: &LastServingVerdict) -> String {
    let engine = match verdict.engine.as_str() {
        "local_apple" => "Apple on-device",
        "local_whisper" if verdict.fallback_used => "Whisper (fallback)",
        "local_whisper" => "Whisper",
        "streaming_whisper" => "Streaming Whisper",
        "cloud_stt" => "Cloud",
        other if !other.is_empty() => other,
        _ => "Unknown",
    };
    let mode = match verdict.routing_mode.to_ascii_lowercase().as_str() {
        "always" => "Always final pass",
        "off" => "Off final pass",
        _ => "Smart final pass",
    };
    match verdict.disposition.as_deref() {
        Some("skipped") => format!("{engine} · {mode} · skipped"),
        Some(disp) if !disp.is_empty() => format!("{engine} · {mode} · {disp}"),
        _ => format!("{engine} · {mode}"),
    }
}

/// Placeholder when no stop has published a serving verdict yet.
pub fn active_stt_awaiting_label() -> &'static str {
    "Not yet served"
}

/// Format from optional snapshot — `None` uses the awaiting placeholder.
pub fn format_active_stt_optional(verdict: Option<&LastServingVerdict>) -> String {
    match verdict {
        Some(v) => format_active_stt_label(v),
        None => active_stt_awaiting_label().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apple_to_whisper_fallback_status_is_not_apple() {
        let verdict = LastServingVerdict {
            engine: "local_whisper".to_string(),
            routing_mode: "smart".to_string(),
            disposition: Some("changed".to_string()),
            fallback_used: true,
        };
        let label = format_active_stt_label(&verdict);
        assert!(
            label.contains("Whisper"),
            "fallback must name Whisper, got {label}"
        );
        assert!(
            label.contains("fallback"),
            "fallback must be explicit, got {label}"
        );
        assert!(
            !label.contains("Apple"),
            "Apple→Whisper must not display Apple, got {label}"
        );
        assert!(label.contains("Smart final pass"));
        assert_eq!(
            format_active_stt_optional(None),
            active_stt_awaiting_label()
        );
        assert_eq!(format_active_stt_optional(Some(&verdict)), label);
    }

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
        assert_eq!(
            format_active_stt_label(&current),
            "Apple on-device · Smart final pass · unchanged"
        );
        clear_last_serving();
    }
}
