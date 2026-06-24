use serde::{Deserialize, Serialize};

use crate::pipeline::contracts::{
    DropKind, EngineEvent, TranscriptSegment, TranscriptionConfidenceFlag,
};

#[derive(Debug, Serialize, Deserialize)]
pub enum IpcCommand {
    // Config
    GetConfig,
    SaveConfig {
        config: Box<crate::config::Config>,
    },
    ReloadRuntimeConfig,

    // Prompts
    GetPrompt {
        prompt_type: String,
    },
    SavePrompt {
        prompt_type: String,
        content: String,
    },
    ResetPrompt {
        prompt_type: String,
    },

    // AI / Chat
    SendMessage {
        message: String,
    },
    ResetContext,
    FormatTranscript {
        text: String,
        language: Option<String>,
        assistive: bool,
    },
    TranscribeFile {
        path: String,
    },

    // Status
    GetStatus,

    // Recording
    StartRecording {
        assistive: bool,
    },
    StopRecording,

    // Event stream
    Subscribe,
    Unsubscribe,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum IpcResponse {
    Config(Box<crate::config::Config>),
    Prompt(String),
    Message(String),
    Status(AppStatus),
    Ok,
    Error(String),
    Event(IpcEvent),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppStatus {
    pub state: String, // "idle", "recording", "busy"
    pub ai_formatting: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcEvent {
    pub timestamp: String, // RFC3339 UTC
    #[serde(flatten)]
    pub payload: IpcEventPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event")]
pub enum IpcEventPayload {
    #[serde(rename = "engine")]
    Engine(EngineEventWire),
    #[serde(rename = "state_change")]
    StateChange { from: String, to: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineEventWire {
    VadStart {
        speech_prob: f32,
        ts_ms: u64,
    },
    VadEnd {
        speech_prob: f32,
        ts_ms: u64,
    },
    NoSpeech {
        reason: String,
    },
    Preview {
        rev: u64,
        text: String,
    },
    Correction {
        rev: u64,
        text: String,
        previous_text: String,
    },
    UtteranceFinal {
        utterance_id: u64,
        text: String,
        start_ts: f32,
        end_ts: f32,
        segments: Vec<TranscriptSegment>,
        vad_speech_pct: Option<f32>,
        avg_logprob: Option<f32>,
        compression_ratio: Option<f32>,
        quality_gate_dropped: bool,
        confidence_flags: Vec<TranscriptionConfidenceFlag>,
    },
    Drop {
        kind: String,
        text: String,
        reason: String,
    },
    Stats {
        dropped_audio_chunks: u64,
        hallucination_drops: u64,
        semantic_gate_drops: u64,
        filtered_empty_drops: u64,
        corrections_applied: u64,
        total_utterances: u64,
        partial_runs_total: u64,
        trigger_utterance_count: u64,
        trigger_speech_count: u64,
        trigger_timer_count: u64,
        partial_stale_count: u64,
        partial_coalesced_count: u64,
        partial_dropped_count: u64,
    },
    Warning {
        code: String,
        message: String,
    },
}

impl From<&EngineEvent> for EngineEventWire {
    fn from(value: &EngineEvent) -> Self {
        match value {
            EngineEvent::VadStart { speech_prob, ts_ms } => Self::VadStart {
                speech_prob: *speech_prob,
                ts_ms: *ts_ms,
            },
            EngineEvent::VadEnd { speech_prob, ts_ms } => Self::VadEnd {
                speech_prob: *speech_prob,
                ts_ms: *ts_ms,
            },
            EngineEvent::NoSpeech { reason } => Self::NoSpeech {
                reason: reason.clone(),
            },
            EngineEvent::Preview { rev, text } => Self::Preview {
                rev: *rev,
                text: text.clone(),
            },
            EngineEvent::Correction {
                rev,
                text,
                previous_text,
            } => Self::Correction {
                rev: *rev,
                text: text.clone(),
                previous_text: previous_text.clone(),
            },
            EngineEvent::UtteranceFinal {
                utterance_id,
                text,
                start_ts,
                end_ts,
                segments,
                vad_speech_pct,
                avg_logprob,
                compression_ratio,
                quality_gate_dropped,
                confidence_flags,
                ..
            } => Self::UtteranceFinal {
                utterance_id: *utterance_id,
                text: text.clone(),
                start_ts: *start_ts,
                end_ts: *end_ts,
                segments: segments.clone(),
                vad_speech_pct: *vad_speech_pct,
                avg_logprob: *avg_logprob,
                compression_ratio: *compression_ratio,
                quality_gate_dropped: *quality_gate_dropped,
                confidence_flags: confidence_flags.clone(),
            },
            EngineEvent::Drop { kind, text, reason } => Self::Drop {
                kind: drop_kind_to_wire(kind).to_string(),
                text: text.clone(),
                reason: reason.clone(),
            },
            EngineEvent::Stats {
                dropped_audio_chunks,
                hallucination_drops,
                semantic_gate_drops,
                filtered_empty_drops,
                corrections_applied,
                total_utterances,
                partial_runs_total,
                trigger_utterance_count,
                trigger_speech_count,
                trigger_timer_count,
                partial_stale_count,
                partial_coalesced_count,
                partial_dropped_count,
            } => Self::Stats {
                dropped_audio_chunks: *dropped_audio_chunks,
                hallucination_drops: *hallucination_drops,
                semantic_gate_drops: *semantic_gate_drops,
                filtered_empty_drops: *filtered_empty_drops,
                corrections_applied: *corrections_applied,
                total_utterances: *total_utterances,
                partial_runs_total: *partial_runs_total,
                trigger_utterance_count: *trigger_utterance_count,
                trigger_speech_count: *trigger_speech_count,
                trigger_timer_count: *trigger_timer_count,
                partial_stale_count: *partial_stale_count,
                partial_coalesced_count: *partial_coalesced_count,
                partial_dropped_count: *partial_dropped_count,
            },
            EngineEvent::Warning { code, message } => Self::Warning {
                code: code.clone(),
                message: message.clone(),
            },
        }
    }
}

fn drop_kind_to_wire(kind: &DropKind) -> &'static str {
    match kind {
        DropKind::Hallucination => "hallucination",
        DropKind::SemanticGate => "semantic_gate",
        DropKind::OverlapEmpty => "overlap_empty",
        DropKind::FilteredEmpty => "filtered_empty",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn must_object(value: Value) -> serde_json::Map<String, Value> {
        value.as_object().cloned().expect("json object")
    }

    #[test]
    fn utterance_final_wire_omits_raw_text() {
        let event = EngineEvent::UtteranceFinal {
            utterance_id: 42,
            text: "hello world".to_string(),
            raw_text: "SENSITIVE RAW TRANSCRIPT".to_string(),
            start_ts: 1.0,
            end_ts: 2.5,
            segments: vec![TranscriptSegment {
                text: "hello world".to_string(),
                start_ts: 1.0,
                end_ts: 2.5,
            }],
            vad_speech_pct: Some(5.0),
            avg_logprob: Some(-0.3),
            compression_ratio: Some(1.1),
            quality_gate_dropped: false,
            confidence_flags: vec![TranscriptionConfidenceFlag::VeryLowSpeech],
        };

        let wire = EngineEventWire::from(&event);
        let json = serde_json::to_value(&wire).expect("serialize wire event");
        let obj = must_object(json);

        assert_eq!(
            obj.get("type").and_then(Value::as_str),
            Some("utterance_final")
        );
        assert!(
            obj.get("raw_text").is_none(),
            "raw_text must not leak to IPC"
        );
        assert_eq!(obj.get("text").and_then(Value::as_str), Some("hello world"));
        assert!(obj.get("segments").is_some(), "segments must be present");
        assert_eq!(
            obj.get("vad_speech_pct")
                .and_then(Value::as_f64)
                .map(|v| v as f32),
            Some(5.0),
            "VAD speech ratio must survive IPC boundary"
        );
        assert_eq!(
            obj.get("avg_logprob")
                .and_then(Value::as_f64)
                .map(|v| v as f32),
            Some(-0.3),
            "confidence metadata must survive IPC boundary"
        );
        assert_eq!(
            obj.get("quality_gate_dropped").and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            obj.get("confidence_flags").and_then(Value::as_array),
            Some(&vec![Value::String("very_low_speech".to_string())])
        );
    }

    #[test]
    fn ipc_event_payload_serialization_is_engine_tagged() {
        let payload = IpcEventPayload::Engine(EngineEventWire::Preview {
            rev: 7,
            text: "preview".to_string(),
        });

        let value = serde_json::to_value(payload).expect("serialize payload");
        let obj = must_object(value);
        assert_eq!(obj.get("event").and_then(Value::as_str), Some("engine"));
    }

    #[test]
    fn no_speech_event_serializes_reason() {
        let event = EngineEvent::NoSpeech {
            reason: "vad_no_speech_detected".to_string(),
        };
        let wire = EngineEventWire::from(&event);
        let json = serde_json::to_value(&wire).expect("serialize no_speech");
        let obj = must_object(json);
        assert_eq!(obj.get("type").and_then(Value::as_str), Some("no_speech"));
        assert_eq!(
            obj.get("reason").and_then(Value::as_str),
            Some("vad_no_speech_detected")
        );
    }

    #[test]
    fn stats_event_serializes_partial_pass_fields() {
        let event = EngineEvent::Stats {
            dropped_audio_chunks: 3,
            hallucination_drops: 2,
            semantic_gate_drops: 1,
            filtered_empty_drops: 4,
            corrections_applied: 5,
            total_utterances: 6,
            partial_runs_total: 7,
            trigger_utterance_count: 8,
            trigger_speech_count: 9,
            trigger_timer_count: 10,
            partial_stale_count: 11,
            partial_coalesced_count: 12,
            partial_dropped_count: 13,
        };
        let wire = EngineEventWire::from(&event);
        let json = serde_json::to_value(&wire).expect("serialize stats");
        let obj = must_object(json);
        assert_eq!(obj.get("type").and_then(Value::as_str), Some("stats"));
        assert_eq!(
            obj.get("partial_runs_total").and_then(Value::as_u64),
            Some(7)
        );
        assert_eq!(
            obj.get("trigger_timer_count").and_then(Value::as_u64),
            Some(10)
        );
        assert_eq!(
            obj.get("partial_dropped_count").and_then(Value::as_u64),
            Some(13)
        );
    }

    #[test]
    fn legacy_vad_fallback_wire_is_rejected() {
        let legacy_json = serde_json::json!({
            "type": "vad_fallback",
            "max_prob": 0.9,
            "samples": 128
        });

        let parsed = serde_json::from_value::<EngineEventWire>(legacy_json);
        assert!(
            parsed.is_err(),
            "legacy vad_fallback variant should not deserialize"
        );
    }

    #[test]
    fn removed_legacy_wire_variants_are_rejected() {
        let legacy_payloads = [
            (
                "vad_fallback",
                serde_json::json!({
                    "type": "vad_fallback",
                    "max_prob": 0.9,
                    "samples": 128
                }),
            ),
            (
                "delta",
                serde_json::json!({
                    "type": "delta",
                    "delta": "hello"
                }),
            ),
            (
                "worker_status",
                serde_json::json!({
                    "type": "worker_status",
                    "state": "running"
                }),
            ),
        ];

        for (variant, payload) in legacy_payloads {
            let err = serde_json::from_value::<EngineEventWire>(payload)
                .expect_err("legacy variant must not deserialize");
            let err_text = err.to_string();
            assert!(
                err_text.contains(variant),
                "expected error to mention rejected variant `{variant}`, got: {err_text}"
            );
        }
    }
}
