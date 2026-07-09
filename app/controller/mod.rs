//! Recording pipeline state machine controller
//!
//! This module implements the core hotkey-driven state machine for Codescribe.
//! It manages recording lifecycle, state transitions, and interaction with the
//! transcription backend.
//!
//! ## State Machine
//!
//! ```text
//! IDLE + hold_down → (wait 800ms) → REC_HOLD
//! IDLE + toggle_press → REC_TOGGLE (continuous)
//! REC_HOLD + hold_up → BUSY (process)
//! REC_TOGGLE + silence → send (no stop)
//! REC_TOGGLE + toggle_press → IDLE (stop)
//! BUSY → (transcribe + format + paste) → IDLE
//! ```
//!
//! ## Hold-to-Talk Delay
//!
//! Users frequently tap Ctrl accidentally, so we require a configurable dwell time
//! (default 800ms) before the recorder actually starts. Assistive hold bindings
//! keep a 400ms floor even if settings lower the generic hold delay. This prevents
//! accidental Emil sessions while preserving quick toggle-mode for power users.

mod helpers;
mod types;

pub use helpers::{
    is_assistive_session, is_conversation_session, set_assistive_session, set_conversation_session,
};
pub use types::{HotkeyAction, HotkeyInput, HotkeyType, State, TranscriptionActionContractMode};

use crate::presentation::emitter::PresentationEmitter;
use crate::stream_postprocess::StreamPostProcessor;
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, RwLock, broadcast};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::audio::streaming_recorder::StreamingRecorder;
use crate::config::models::ModelManager;
use crate::config::{Config, UserSettings};
use crate::os::clipboard;
use crate::os::hotkeys::HoldMode;
use crate::os::selection::{
    AssistiveContext, build_assistive_input, capture_assistive_context,
    capture_assistive_context_with_prior_frontmost, capture_frontmost_app_only,
    capture_frontmost_app_only_with_prior_frontmost, get_recent_assistive_context,
    store_recent_assistive_context,
};

// Moshi conversation engine and audio output
use codescribe_core::conversation::{ConversationEngine, MoshiConfig};
use codescribe_core::ipc::{IpcEvent, IpcEventPayload};
use codescribe_core::tts::AudioPlayer;

use codescribe_core::pipeline::contracts::{
    FileTranscriptionOptions, FinalPassDisposition, TranscriptionConfidenceFlag,
    TranscriptionVerdict,
};

use helpers::{
    SessionTelemetrySnapshot, SharedSessionTelemetry, new_session_telemetry, raw_save_enabled,
    reset_agent_runtime_for_new_thread as reset_agent_runtime_for_new_thread_impl,
    reset_session_telemetry, send_assistive_with_agent_runtime, snapshot_session_telemetry,
};
use types::{
    RecordingFallbackClass, RecordingTranscriptSource, RecordingTruthMetadata, ValidatedAudioPath,
};

const LIVE_PROFILE_BUFFER_DELAY_MS: u64 = 280;
const LIVE_PROFILE_TYPING_CPS: f32 = 90.0;
const LIVE_PROFILE_EMIT_WORDS_MAX: u64 = 2;
const LIVE_PROFILE_INTERIM_SEC: f32 = 1.2;
const NO_OVERLAY_PROFILE_INTERIM_SEC: f32 = 8.0;
const ASSISTIVE_HOLD_START_DELAY_FLOOR_MS: u64 = 400;

fn effective_hold_start_delay_ms(configured_ms: u64, assistive: bool) -> u64 {
    if assistive {
        configured_ms.max(ASSISTIVE_HOLD_START_DELAY_FLOOR_MS)
    } else {
        configured_ms
    }
}

const TOGGLE_STOP_ADJUDICATE_TIMEOUT: Duration = Duration::from_secs(120);
const STOP_TIMEOUT: Duration = TOGGLE_STOP_ADJUDICATE_TIMEOUT;

#[cfg(test)]
fn toggle_stop_adjudicate_timeout() -> Duration {
    STOP_TIMEOUT
}

#[cfg(test)]
static PROCESS_RECORDING_TEST_HANG: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
struct ProcessRecordingHangGuard;

#[cfg(test)]
fn hang_process_recording_for_test() -> ProcessRecordingHangGuard {
    PROCESS_RECORDING_TEST_HANG.store(true, Ordering::SeqCst);
    ProcessRecordingHangGuard
}

#[cfg(test)]
impl Drop for ProcessRecordingHangGuard {
    fn drop(&mut self) {
        PROCESS_RECORDING_TEST_HANG.store(false, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone, Copy)]
struct ActionQualityProbe {
    raw_chars: usize,
    final_chars: usize,
    raw_final_diff_ratio: f32,
    correction_ratio: f32,
    drop_ratio: f32,
}

fn normalize_for_diff(s: &str) -> String {
    let trimmed = s.trim_start();
    // Lowercase first char only (preserving rest of original case)
    let mut chars = trimmed.chars();
    match chars.next() {
        Some(c) => c.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

impl ActionQualityProbe {
    fn from_transcripts(
        raw_text: &str,
        final_text: &str,
        post_stats: &crate::stream_postprocess::StreamPostProcessStats,
    ) -> Self {
        let raw_chars = raw_text.chars().count();
        let final_chars = final_text.chars().count();

        let (backspaces, inserted_chars) =
            codescribe_core::pipeline::contracts::TranscriptDelta::from_diff(
                &normalize_for_diff(raw_text),
                &normalize_for_diff(final_text),
            )
            .map(|delta| {
                let backspaces = delta
                    .delta
                    .chars()
                    .filter(|c| *c == codescribe_core::pipeline::contracts::BACKSPACE)
                    .count();
                let inserted = delta.delta.chars().count().saturating_sub(backspaces);
                (backspaces, inserted)
            })
            .unwrap_or((0, 0));

        let span = raw_chars.max(final_chars).max(1);
        let raw_final_diff_ratio = ((backspaces + inserted_chars) as f32 / span as f32).min(1.0);
        let correction_ratio = (backspaces as f32 / raw_chars.max(1) as f32).min(1.0);
        let drop_ratio = if post_stats.input_chunks == 0 {
            0.0
        } else {
            post_stats.dropped_chunks as f32 / post_stats.input_chunks as f32
        };

        Self {
            raw_chars,
            final_chars,
            raw_final_diff_ratio,
            correction_ratio,
            drop_ratio,
        }
    }
}

fn apply_runtime_transcription_profile(config: &Config, assistive: bool) -> bool {
    let overlay_enabled = config.transcription_overlay_enabled;
    let settings = UserSettings::load();

    let buffer_delay_ms = settings
        .buffer_delay_ms
        .unwrap_or(LIVE_PROFILE_BUFFER_DELAY_MS);
    let typing_cps = settings.typing_cps.unwrap_or(LIVE_PROFILE_TYPING_CPS);
    let emit_words_max = settings
        .emit_words_max
        .unwrap_or(LIVE_PROFILE_EMIT_WORDS_MAX);
    let interim_sec = if !assistive && !overlay_enabled {
        NO_OVERLAY_PROFILE_INTERIM_SEC
    } else {
        settings
            .buffered_interim_sec
            .unwrap_or(LIVE_PROFILE_INTERIM_SEC)
    };

    unsafe {
        std::env::set_var(
            "TRANSCRIPTION_OVERLAY_ENABLED",
            if overlay_enabled { "1" } else { "0" },
        );
        std::env::set_var("CODESCRIBE_BUFFER_DELAY_MS", buffer_delay_ms.to_string());
        std::env::set_var("CODESCRIBE_TYPING_CPS", format!("{typing_cps:.1}"));
        std::env::set_var("CODESCRIBE_EMIT_WORDS_MAX", emit_words_max.to_string());
        std::env::set_var(
            "CODESCRIBE_BUFFERED_INTERIM_SEC",
            format!("{interim_sec:.1}"),
        );
    }

    overlay_enabled
}

fn non_empty_transcript(text: Option<String>) -> Option<String> {
    text.and_then(|text| {
        if text.trim().is_empty() {
            None
        } else {
            Some(text)
        }
    })
}

#[derive(Debug, Clone, Default)]
struct RecordingTruthVerdict {
    raw_text: Option<String>,
    transcript_source: Option<RecordingTranscriptSource>,
    fallback_class: Option<RecordingFallbackClass>,
    no_speech_reason: Option<String>,
    speech_pct: Option<f32>,
    avg_logprob: Option<f32>,
    /// Typed confidence flags (engine-owned + app-level provenance).
    /// Stored as the core enum rather than `Vec<String>` so downstream
    /// consumers do not need to re-parse tokens and new variants surface
    /// as compile errors instead of silent misses.
    confidence_flags: Vec<TranscriptionConfidenceFlag>,
    /// VAD speech sparkline preserved from the core `VadVerdict` so it
    /// can survive to `truth.json` on disk (previously dropped at the
    /// app boundary — tracked as Kłamstwo 7).
    sparkline: Option<String>,
    /// Disposition of the explicit file-level final pass, when one ran.
    /// None means no final pass was attempted for this verdict.
    final_pass_disposition: Option<FinalPassDisposition>,
    commit_trigger: Option<String>,
    display_status: String,
}

fn push_typed_flag(
    flags: &mut Vec<TranscriptionConfidenceFlag>,
    flag: TranscriptionConfidenceFlag,
) {
    if !flags.contains(&flag) {
        flags.push(flag);
    }
}

fn apply_ai_noop_signal(
    assistive: bool,
    is_ai_noop: bool,
    confidence_flags: &mut Vec<TranscriptionConfidenceFlag>,
    commit_trigger: &mut Option<String>,
) {
    if !is_ai_noop || assistive {
        return;
    }

    push_typed_flag(
        confidence_flags,
        TranscriptionConfidenceFlag::AiNoopDetected,
    );
    *commit_trigger = Some("ai_noop".to_string());
}

fn truth_review_trigger(
    fallback_class: Option<RecordingFallbackClass>,
    no_speech_reason: Option<&str>,
    confidence_flags: &[TranscriptionConfidenceFlag],
) -> Option<String> {
    if no_speech_reason.is_some() {
        return Some("no_reliable_speech".to_string());
    }

    // Priority order mirrors the original string-based trigger so display
    // behaviour is unchanged by the type-safety refactor.
    for priority_flag in [
        TranscriptionConfidenceFlag::PossibleHallucinationLogprob,
        TranscriptionConfidenceFlag::VeryLowSpeech,
        TranscriptionConfidenceFlag::StreamingPreviewUsedAsVerdict,
        TranscriptionConfidenceFlag::CloudFallbackUsed,
    ] {
        if confidence_flags.contains(&priority_flag) {
            return Some(priority_flag.to_string());
        }
    }

    match fallback_class {
        Some(RecordingFallbackClass::Acceptable) | None => None,
        Some(RecordingFallbackClass::Degraded) => Some("degraded_fallback".to_string()),
        Some(RecordingFallbackClass::Unsafe) => Some("unsafe_fallback".to_string()),
    }
}

fn truth_display_status(
    source: Option<RecordingTranscriptSource>,
    fallback_class: Option<RecordingFallbackClass>,
    no_speech_reason: Option<&str>,
    confidence_flags: &[TranscriptionConfidenceFlag],
) -> String {
    if no_speech_reason.is_some() {
        return "No reliable speech detected".to_string();
    }

    if confidence_flags.contains(&TranscriptionConfidenceFlag::PossibleHallucinationLogprob) {
        return "Possible hallucination".to_string();
    }

    if confidence_flags.contains(&TranscriptionConfidenceFlag::VeryLowSpeech) {
        return "Very low speech".to_string();
    }

    match (source, fallback_class) {
        (Some(RecordingTranscriptSource::StreamingFallback), _) => "Streaming fallback".to_string(),
        (Some(source), Some(fallback_class)) => {
            format!("{} ({})", source.label(), fallback_class.label())
        }
        (Some(source), None) => source.label().to_string(),
        (None, Some(fallback_class)) => fallback_class.label().to_string(),
        (None, None) => "Transcript ready".to_string(),
    }
}

// allow(too_many_arguments): verdict aggregates 9 independent recording-truth
// signals collected at one call site; a params struct would only restate the
// same nine names. Revisit if call sites multiply.
#[allow(clippy::too_many_arguments)]
fn build_truth_verdict(
    raw_text: Option<String>,
    transcript_source: Option<RecordingTranscriptSource>,
    fallback_class: Option<RecordingFallbackClass>,
    no_speech_reason: Option<String>,
    speech_pct: Option<f32>,
    avg_logprob: Option<f32>,
    confidence_flags: Vec<TranscriptionConfidenceFlag>,
    sparkline: Option<String>,
    final_pass_disposition: Option<FinalPassDisposition>,
) -> RecordingTruthVerdict {
    let commit_trigger = truth_review_trigger(
        fallback_class,
        no_speech_reason.as_deref(),
        &confidence_flags,
    );
    let display_status = truth_display_status(
        transcript_source,
        fallback_class,
        no_speech_reason.as_deref(),
        &confidence_flags,
    );

    RecordingTruthVerdict {
        raw_text,
        transcript_source,
        fallback_class,
        no_speech_reason,
        speech_pct,
        avg_logprob,
        confidence_flags,
        sparkline,
        final_pass_disposition,
        commit_trigger,
        display_status,
    }
}

fn adjudicate_recording_truth(
    use_local_stt: bool,
    local_final_pass_attempted: bool,
    local_final_pass_verdict: Option<TranscriptionVerdict>,
    streaming_text: String,
    cloud_verdict: Option<crate::client::CloudTranscriptionVerdict>,
    session_telemetry: &SessionTelemetrySnapshot,
) -> RecordingTruthVerdict {
    let streaming_text = non_empty_transcript(Some(streaming_text));
    let cloud_verdict = cloud_verdict.filter(|verdict| !verdict.text.trim().is_empty());

    if use_local_stt && let Some(verdict) = local_final_pass_verdict {
        let speech_pct = verdict.vad.as_ref().map(|vad| vad.speech_pct);
        let avg_logprob = verdict.raw.avg_logprob;
        let fallback_class = if verdict.confidence_flags.iter().any(|flag| {
            matches!(
                flag,
                TranscriptionConfidenceFlag::VeryLowSpeech
                    | TranscriptionConfidenceFlag::PossibleHallucinationLogprob
                    | TranscriptionConfidenceFlag::QualityGateDropped
            )
        }) {
            Some(RecordingFallbackClass::Unsafe)
        } else {
            None
        };

        // Preserve the typed confidence flags as-is (no stringification).
        let confidence_flags = verdict.confidence_flags.clone();
        let no_speech_reason = verdict
            .vad
            .as_ref()
            .and_then(|vad| vad.no_speech_reason.clone());
        // Sparkline lives in the VAD sub-verdict. Only surface it when VAD
        // actually produced one so empty strings don't pollute truth.json.
        let sparkline = verdict.vad.as_ref().and_then(|vad| {
            if vad.sparkline.is_empty() {
                None
            } else {
                Some(vad.sparkline.clone())
            }
        });
        // Final-pass disposition is only meaningful when the engine ran
        // an explicit final pass (hold path). Toggle path leaves this None.
        let final_pass_disposition = verdict.final_pass.as_ref().map(|fp| fp.disposition);

        let raw_text = if no_speech_reason.is_some() {
            None
        } else {
            non_empty_transcript(Some(verdict.text))
        };

        return build_truth_verdict(
            raw_text,
            Some(RecordingTranscriptSource::LocalFinalPass),
            fallback_class,
            no_speech_reason,
            speech_pct,
            avg_logprob,
            confidence_flags,
            sparkline,
            final_pass_disposition,
        );
    }

    if let Some(reason) = &session_telemetry.no_speech_reason {
        return build_truth_verdict(
            None,
            None,
            None,
            Some(reason.clone()),
            None,
            None,
            Vec::new(),
            None,
            None,
        );
    }

    if use_local_stt {
        let mut confidence_flags: Vec<TranscriptionConfidenceFlag> = Vec::new();
        if local_final_pass_attempted {
            push_typed_flag(
                &mut confidence_flags,
                TranscriptionConfidenceFlag::LocalFinalPassUnavailable,
            );
        }

        if let Some(cloud_verdict) = cloud_verdict {
            let mut fallback_flags = confidence_flags.clone();
            for flag in &cloud_verdict.confidence_flags {
                push_typed_flag(&mut fallback_flags, *flag);
            }
            push_typed_flag(
                &mut fallback_flags,
                TranscriptionConfidenceFlag::CloudFallbackUsed,
            );
            return build_truth_verdict(
                Some(cloud_verdict.text),
                Some(RecordingTranscriptSource::CloudFallback),
                Some(RecordingFallbackClass::Degraded), // cloud fallback is no longer "Acceptable" (silent), it must be explicit
                None,
                None,
                None,
                fallback_flags,
                None,
                None,
            );
        }

        if let Some(text) = streaming_text {
            let mut fallback_flags = confidence_flags.clone();
            push_typed_flag(
                &mut fallback_flags,
                TranscriptionConfidenceFlag::UnverifiedStream,
            );
            push_typed_flag(
                &mut fallback_flags,
                TranscriptionConfidenceFlag::StreamingPreviewUsedAsVerdict,
            );
            return build_truth_verdict(
                Some(text),
                Some(RecordingTranscriptSource::StreamingFallback),
                Some(RecordingFallbackClass::Degraded), // streaming is always degraded as a final verdict
                None,
                None,
                None,
                fallback_flags,
                None,
                None,
            );
        }
    } else {
        if let Some(cloud_verdict) = cloud_verdict {
            return build_truth_verdict(
                Some(cloud_verdict.text),
                Some(RecordingTranscriptSource::CloudPrimary),
                None,
                None,
                None,
                None,
                cloud_verdict.confidence_flags,
                None,
                None,
            );
        }

        if let Some(text) = streaming_text {
            let mut confidence_flags: Vec<TranscriptionConfidenceFlag> = Vec::new();
            push_typed_flag(
                &mut confidence_flags,
                TranscriptionConfidenceFlag::CloudPrimaryMissing,
            );
            push_typed_flag(
                &mut confidence_flags,
                TranscriptionConfidenceFlag::UnverifiedStream,
            );
            push_typed_flag(
                &mut confidence_flags,
                TranscriptionConfidenceFlag::StreamingPreviewUsedAsVerdict,
            );
            return build_truth_verdict(
                Some(text),
                Some(RecordingTranscriptSource::StreamingFallback),
                Some(RecordingFallbackClass::Degraded),
                None,
                None,
                None,
                confidence_flags,
                None,
                None,
            );
        }
    }

    build_truth_verdict(
        None,
        None,
        None,
        Some(
            session_telemetry
                .no_speech_reason
                .clone()
                .unwrap_or_else(|| "empty_transcript_without_no_speech_event".to_string()),
        ),
        None,
        None,
        Vec::new(),
        None,
        None,
    )
}

fn recording_mode_label(
    assistive: bool,
    hold_mode: HoldMode,
    force_raw: bool,
    force_ai: bool,
) -> &'static str {
    if assistive {
        match hold_mode {
            HoldMode::Chat => "chat",
            HoldMode::Selection => "selection",
            HoldMode::Raw => "assistive",
        }
    } else if force_raw {
        "raw"
    } else if force_ai {
        "format"
    } else {
        "toggle"
    }
}

fn maybe_wrap_transcript_for_delivery(text: &str, config: &Config, mode: &str) -> String {
    if !config.transcript_tagging_enabled {
        return text.to_string();
    }

    codescribe_core::transcript_tagging::wrap_transcript(
        text,
        &config.transcript_tag_template,
        mode,
        config.whisper_language.as_str(),
    )
}

fn toggle_final_pass_enabled() -> bool {
    std::env::var("CODESCRIBE_TOGGLE_FINAL_PASS")
        .ok()
        .map(|v| {
            !matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "" | "0" | "false" | "no" | "off"
            )
        })
        .unwrap_or(true)
}

fn should_use_toggle_adjudicated_stop(
    current_state: State,
    assistive: bool,
    toggle_final_pass: bool,
) -> bool {
    current_state == State::RecToggle && !assistive && toggle_final_pass
}

fn should_apply_incoming_mode_flags(current_state: State, event: &HotkeyInput) -> bool {
    matches!(event.action, HotkeyAction::Down | HotkeyAction::Press)
        && !(event.key_type == HotkeyType::Toggle && current_state == State::RecToggle)
}

fn is_hotkey_start_event(event: &HotkeyInput) -> bool {
    matches!(
        (event.key_type, event.action),
        (HotkeyType::Hold, HotkeyAction::Down)
            | (HotkeyType::Toggle, HotkeyAction::Press)
            | (HotkeyType::Conversation, HotkeyAction::Press)
    )
}

/// An assistive *start* hotkey — FN+Shift hold-down, an assistive toggle press,
/// or any start event flagged `assistive` (Chat / Selection / assistive toggle).
/// These are the "Talk Anytime" inputs the user fires to add a new voice intent
/// while Emil/the agent is still answering.
fn is_assistive_start_event(event: &HotkeyInput) -> bool {
    is_hotkey_start_event(event) && event.assistive
}

/// Block a *new* hotkey start while a previously-dispatched agent turn is still
/// streaming. This fires only at `State::Idle` — the controller has already
/// returned the mic/transcription pipeline; the agent is answering in the
/// background (a detached `tokio::spawn`, see `send_assistive_with_agent_runtime`).
///
/// Exception — **Assistive Talk Anytime**: assistive start events are allowed
/// through so the user can record a *new* voice intent while the agent answers.
/// The resulting utterance is captured into the existing pending-follow-up
/// buffer (`should_capture_pending_followup` → `get_or_create_pending_followup_index`),
/// not dropped — the living intent grows instead of being ignored. Non-assistive
/// (raw) dictation starts stay blocked: barging a raw transcript into a live
/// agent turn is never wanted, and blocking preserves the single-pipeline
/// guarantee for the dictation path.
///
/// `agent_send_in_flight` is passed in (rather than read from the global) so the
/// decision is a pure function and unit-testable without touching shared state.
fn should_block_hotkey_during_agent_send(
    current_state: State,
    event: &HotkeyInput,
    agent_send_in_flight: bool,
) -> bool {
    current_state == State::Idle
        && agent_send_in_flight
        && is_hotkey_start_event(event)
        && !is_assistive_start_event(event)
}

fn transcript_output_category(output_kind: crate::state::history::TranscriptKind) -> &'static str {
    match output_kind {
        crate::state::history::TranscriptKind::Raw => "Transcript",
        crate::state::history::TranscriptKind::Cloud => "Cloud transcript",
        crate::state::history::TranscriptKind::FormattedTranscript => "Formatted transcript",
        crate::state::history::TranscriptKind::AssistantInterpretation => {
            "Assistant interpretation"
        }
        crate::state::history::TranscriptKind::FormattingFailed => {
            "Formatting failed, raw preserved"
        }
        crate::state::history::TranscriptKind::Failed => "Failed transcript",
    }
}

fn compose_final_status(
    display_status: &str,
    output_kind: crate::state::history::TranscriptKind,
) -> String {
    if display_status.trim().is_empty() {
        return transcript_output_category(output_kind).to_string();
    }

    match output_kind {
        crate::state::history::TranscriptKind::Failed => display_status.to_string(),
        _ => format!(
            "{} • {}",
            display_status,
            transcript_output_category(output_kind)
        ),
    }
}

fn truth_engine_label(source: Option<RecordingTranscriptSource>) -> Option<String> {
    source.map(|source| match source {
        RecordingTranscriptSource::LocalFinalPass => "local_whisper".to_string(),
        RecordingTranscriptSource::ToggleSessionAdjudicated => "local_whisper".to_string(),
        RecordingTranscriptSource::CloudPrimary => "cloud_stt".to_string(),
        RecordingTranscriptSource::CloudFallback => "cloud_stt".to_string(),
        RecordingTranscriptSource::Streaming => "streaming_whisper".to_string(),
        RecordingTranscriptSource::StreamingFallback => "streaming_whisper".to_string(),
    })
}

fn write_truth_sidecar_logged(path: &std::path::Path, metadata: &RecordingTruthMetadata) {
    match types::write_truth_sidecar(path, metadata) {
        Ok(sidecar_path) => debug!("Truth sidecar saved: {}", sidecar_path.display()),
        Err(error) => warn!(
            "Failed to write truth sidecar for {}: {}",
            path.display(),
            error
        ),
    }
}

const QUALITY_GATE_MIN_CHARS: usize = 24;
const SHORT_AI_QUALITY_GATE_MIN_CHARS: usize = 10;
const QUALITY_GATE_DROP_RATIO: f32 = 0.35;
const QUALITY_GATE_DIFF_RATIO: f32 = 0.62;
const QUALITY_GATE_CORRECTION_RATIO: f32 = 0.40;

struct AtomicFlagGuard {
    flag: Arc<AtomicBool>,
}

impl AtomicFlagGuard {
    fn new(flag: Arc<AtomicBool>) -> Self {
        flag.store(true, Ordering::SeqCst);
        Self { flag }
    }
}

impl Drop for AtomicFlagGuard {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone, Default)]
struct ProcessRecordingOutcome {
    no_speech_reason: Option<String>,
    commit_trigger: Option<String>,
    transcript_present: bool,
}

impl ProcessRecordingOutcome {
    fn no_speech(reason: impl Into<String>) -> Self {
        Self {
            no_speech_reason: Some(reason.into()),
            commit_trigger: None,
            transcript_present: false,
        }
    }
}

fn should_allow_full_user_bubble_rewrite(
    skip_user_bubble: bool,
    append_mode: bool,
    live_stream_session: bool,
) -> bool {
    !skip_user_bubble && !append_mode && !live_stream_session
}

fn should_apply_transcription_action_contract(assistive: bool, live_stream_session: bool) -> bool {
    !assistive && !live_stream_session
}

fn evaluate_quality_commit_trigger(
    force_raw: bool,
    quality_probe: &ActionQualityProbe,
    output_kind: crate::state::history::TranscriptKind,
) -> Option<&'static str> {
    let short_ai_formatted = output_kind
        == crate::state::history::TranscriptKind::FormattedTranscript
        && quality_probe.raw_chars.max(quality_probe.final_chars)
            >= SHORT_AI_QUALITY_GATE_MIN_CHARS;
    if force_raw {
        return None;
    }
    if output_kind == crate::state::history::TranscriptKind::FormattingFailed {
        return Some("ai_failed_fallback");
    }
    if quality_probe.raw_chars < QUALITY_GATE_MIN_CHARS
        && quality_probe.final_chars < QUALITY_GATE_MIN_CHARS
        && !short_ai_formatted
    {
        return None;
    }
    if quality_probe.drop_ratio >= QUALITY_GATE_DROP_RATIO {
        return Some("high_drop_ratio");
    }
    if quality_probe.raw_final_diff_ratio >= QUALITY_GATE_DIFF_RATIO {
        return Some("high_rewrite_ratio");
    }
    if quality_probe.correction_ratio >= QUALITY_GATE_CORRECTION_RATIO {
        return Some("high_correction_ratio");
    }
    None
}

/// Rotate runtime + thread identity and return generation once backend reset completes.
pub async fn reset_agent_runtime_for_new_thread() -> Result<u64> {
    reset_agent_runtime_for_new_thread_impl().await
}

/// Recording controller managing state machine and lifecycle
pub struct RecordingController {
    /// Application configuration
    config: Arc<RwLock<Config>>,

    /// Current state
    state: Arc<RwLock<State>>,

    /// Audio recorder instance
    recorder: Arc<Mutex<Option<StreamingRecorder>>>,

    /// Whether AI assistive mode is enabled for the current session.
    ///
    /// This is true for:
    /// - Hold modes: Chat (Shift) / Selection (Cmd)
    /// - Assistive toggle (right Option double-tap, if enabled)
    assistive_mode: Arc<RwLock<bool>>,
    /// Current hold intent (Raw/Chat/Selection) for the active session.
    hold_mode: Arc<RwLock<HoldMode>>,

    /// Whether to force RAW mode (Ctrl Hold without Shift = always raw, ignores AI toggle)
    /// Toggle mode (Double Option) keeps this false and respects AI_FORMATTING_ENABLED setting.
    force_raw_mode: Arc<RwLock<bool>>,
    /// Whether to force AI formatting for the current session (e.g., left double Option)
    force_ai_mode: Arc<RwLock<bool>>,

    /// Current session ID for tracking
    session_id: Arc<RwLock<Option<String>>>,

    /// Task handle for delayed hold-start (800ms default)
    hold_start_task: Arc<Mutex<Option<JoinHandle<()>>>>,
    /// Monotonic generation for hold-start tasks.
    ///
    /// Every cancel/reschedule bumps this value. Spawned tasks compare their
    /// captured generation before/after critical awaits to avoid stale-start races.
    hold_start_generation: Arc<AtomicU64>,
    /// Guard flag used to prevent idle-recovery from killing a freshly-starting session.
    start_transition_in_flight: Arc<AtomicBool>,

    /// Lock to serialize finish_recording calls
    serial_lock: Arc<Mutex<()>>,

    /// Flag set by VAD (silence detection) when recording should auto-stop
    vad_triggered: Arc<AtomicBool>,

    /// Assistive hands-off loop active (Right Option toggle)
    assistive_loop_active: Arc<AtomicBool>,

    /// Toggle session: track whether we've already appended user/assistant text
    toggle_user_has_text: Arc<AtomicBool>,
    toggle_assistant_has_text: Arc<AtomicBool>,

    /// Best-effort selected-text/app context captured for assistive sessions.
    ///
    /// Must be captured BEFORE showing any overlay window, because overlays
    /// may steal focus and destroy the user's selection context.
    assistive_context: Arc<RwLock<Option<AssistiveContext>>>,
    /// App that was frontmost when the user initiated a hold session, before
    /// Codescribe badge/overlay UI can become frontmost.
    pre_overlay_frontmost_app: Arc<RwLock<Option<String>>>,

    /// Sample offset (in the recorder buffer) marking the start of the next
    /// incremental segment. Advances on each `commit_segment` call so segment
    /// snapshots don't overlap. Resets to 0 on new toggle session start.
    ///
    /// Used by Commit / Augment overlay buttons to clip a WAV slice from the
    /// active recorder without stopping the stream.
    last_segment_audio_offset: Arc<AtomicUsize>,

    // ═══════════════════════════════════════════════════════════
    // Conversation mode (Moshi full-duplex)
    // ═══════════════════════════════════════════════════════════
    /// Moshi conversation engine (lazy-initialized on first use)
    conversation_engine: Arc<Mutex<Option<ConversationEngine>>>,

    /// Audio player for conversation responses (lazy-initialized)
    audio_player: Arc<Mutex<Option<AudioPlayer>>>,

    /// Flag to signal conversation mode should stop
    conversation_stop_flag: Arc<AtomicBool>,

    /// Session generation counter - increments on each conversation start.
    /// Spawn tasks capture this value and compare before UI updates to prevent
    /// cross-session race conditions (old tasks updating new session's UI).
    conversation_generation: Arc<AtomicU64>,

    /// Task handle for conversation audio processing loop
    conversation_task: Arc<Mutex<Option<JoinHandle<()>>>>,

    /// Broadcast stream for IPC subscribers.
    event_broadcast: broadcast::Sender<IpcEvent>,
    /// Per-session telemetry from engine events (`NoSpeech`, `Stats`).
    session_telemetry: SharedSessionTelemetry,
}

impl RecordingController {
    fn recorder_unavailable_error(context: &str) -> anyhow::Error {
        warn!("{context}: streaming recorder unavailable; voice capture is disabled");
        anyhow::anyhow!("{context}: streaming recorder unavailable")
    }

    fn init_streaming_recorder(context: &str) -> Option<StreamingRecorder> {
        match StreamingRecorder::new() {
            Ok(recorder) => Some(recorder),
            Err(error) => {
                warn!("{context}: failed to initialize streaming recorder: {error}");
                None
            }
        }
    }

    fn recorder_from_guard_mut<'a>(
        recorder_guard: &'a mut Option<StreamingRecorder>,
        context: &str,
    ) -> Result<&'a mut StreamingRecorder> {
        recorder_guard
            .as_mut()
            .ok_or_else(|| Self::recorder_unavailable_error(context))
    }

    fn recorder_from_guard<'a>(
        recorder_guard: &'a Option<StreamingRecorder>,
        context: &str,
    ) -> Result<&'a StreamingRecorder> {
        recorder_guard
            .as_ref()
            .ok_or_else(|| Self::recorder_unavailable_error(context))
    }

    /// Create a new recording controller with configuration loaded from disk
    pub fn new() -> Self {
        Self::with_config(Config::load(), "RecordingController::new")
    }

    /// Create a new recording controller without populating secrets from Keychain.
    ///
    /// Used by the SwiftUI redesign dictation bridge: starting local recording must
    /// not ask for API-key access as an incidental side effect.
    pub fn new_without_keychain() -> Self {
        Self::with_config(
            Config::load_without_keychain(),
            "RecordingController::new_without_keychain",
        )
    }

    fn with_config(config: Config, recorder_context: &str) -> Self {
        info!(
            "Initializing RecordingController (hold_delay={}ms, beep={}, language={:?})",
            config.hold_start_delay_ms, config.beep_on_start, config.whisper_language
        );

        let recorder = Self::init_streaming_recorder(recorder_context);

        if !cfg!(test) {
            match ModelManager::new() {
                Ok(model_manager) => {
                    if let Ok(models) = model_manager.list_models()
                        && !models.is_empty()
                    {
                        info!("Available local models: {:?}", models);
                    }
                }
                Err(error) => warn!("Model manager unavailable during startup: {error}"),
            }

            if !crate::whisper::is_initialized() {
                // Best-effort BACKGROUND prewarm — never block recording readiness.
                //
                // Product invariant: recording readiness is NOT engine readiness.
                // Audio capture must start the moment the user presses record; the
                // live pipeline and the final pass lazy-load the engine on first use.
                // A failed prewarm is a warning, not an app or recording failure.
                // The idle-unload reaper (commit 2b8bb1f) may legitimately drop the
                // engine later and the next call reloads it — pinning it here would
                // undo that GPU/host-memory reclaim.
                //
                // Warm the ACTIVE router engine (Apple SpeechAnalyzer on macOS 26+,
                // Candle on fallback/older macOS) AND run a synthetic warmup
                // inference, so the first dictation pays neither model-load nor
                // Metal kernel-compilation latency — matching the old always-instant
                // behaviour where the long-lived daemon was warm before first use.
                std::thread::Builder::new()
                    .name("stt-prewarm".into())
                    .spawn(|| {
                        if let Err(e) = crate::stt::prewarm_active_engine() {
                            warn!(
                                "STT background prewarm failed (will lazy-load on first use): {}",
                                e
                            );
                        }
                    })
                    .ok();
            }
        }

        let config = Arc::new(RwLock::new(config));
        if recorder.is_none() {
            warn!("Recorder unavailable at controller init; voice capture is disabled");
        }
        let (event_broadcast, _) = broadcast::channel::<IpcEvent>(256);
        let session_telemetry = new_session_telemetry();

        Self {
            config,
            state: Arc::new(RwLock::new(State::Idle)),
            recorder: Arc::new(Mutex::new(recorder)),
            assistive_mode: Arc::new(RwLock::new(false)),
            hold_mode: Arc::new(RwLock::new(HoldMode::Raw)),
            force_raw_mode: Arc::new(RwLock::new(false)),
            force_ai_mode: Arc::new(RwLock::new(false)),
            session_id: Arc::new(RwLock::new(None)),
            hold_start_task: Arc::new(Mutex::new(None)),
            hold_start_generation: Arc::new(AtomicU64::new(0)),
            start_transition_in_flight: Arc::new(AtomicBool::new(false)),
            serial_lock: Arc::new(Mutex::new(())),
            vad_triggered: Arc::new(AtomicBool::new(false)),
            assistive_loop_active: Arc::new(AtomicBool::new(false)),
            toggle_user_has_text: Arc::new(AtomicBool::new(false)),
            toggle_assistant_has_text: Arc::new(AtomicBool::new(false)),
            assistive_context: Arc::new(RwLock::new(None)),
            pre_overlay_frontmost_app: Arc::new(RwLock::new(None)),
            last_segment_audio_offset: Arc::new(AtomicUsize::new(0)),
            // Conversation mode (lazy init)
            conversation_engine: Arc::new(Mutex::new(None)),
            audio_player: Arc::new(Mutex::new(None)),
            conversation_stop_flag: Arc::new(AtomicBool::new(false)),
            conversation_generation: Arc::new(AtomicU64::new(0)),
            conversation_task: Arc::new(Mutex::new(None)),
            event_broadcast,
            session_telemetry,
        }
    }

    /// Get current state
    pub async fn current_state(&self) -> State {
        *self.state.read().await
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<IpcEvent> {
        self.event_broadcast.subscribe()
    }

    async fn set_state(&self, new_state: State) {
        Self::set_state_with_broadcast(&self.state, &self.event_broadcast, new_state).await;
    }

    async fn show_processing_badge_if_enabled(&self) {
        let hold_indicator = self.config.read().await.hold_indicator;
        if hold_indicator {
            crate::os::hold_badge::show_badge_for_mode(
                crate::os::hold_badge::BadgeMode::Processing,
            );
        }
    }

    async fn set_state_with_broadcast(
        state: &Arc<RwLock<State>>,
        event_broadcast: &broadcast::Sender<IpcEvent>,
        new_state: State,
    ) {
        let old_state = {
            let mut guard = state.write().await;
            let old = *guard;
            *guard = new_state;
            old
        };

        if old_state != new_state {
            // Recording ended → always tear down the cursor badge (covers finalize,
            // cancel, error, no-speech — any path back to Idle).
            if new_state == State::Idle {
                crate::os::hold_badge::hide_hold_badge();
            }
            let _ = event_broadcast.send(IpcEvent {
                timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                payload: IpcEventPayload::StateChange {
                    from: old_state.to_ipc_str().to_string(),
                    to: new_state.to_ipc_str().to_string(),
                },
            });
        }
    }

    /// Replace controller configuration at runtime
    pub async fn set_config(&self, config: Config) {
        *self.config.write().await = config;
    }

    /// Snapshot of current controller configuration
    pub async fn get_config(&self) -> Config {
        self.config.read().await.clone()
    }

    /// Check if VAD (silence detection) has triggered auto-stop
    pub fn is_vad_triggered(&self) -> bool {
        self.vad_triggered.load(Ordering::SeqCst)
    }

    /// Clear the VAD triggered flag
    pub fn clear_vad_triggered(&self) {
        self.vad_triggered.store(false, Ordering::SeqCst);
    }

    /// Cancel any pending delayed hold-start task
    async fn cancel_pending_hold_start(&self) {
        let generation = self.hold_start_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let mut task_guard = self.hold_start_task.lock().await;
        if let Some(task) = task_guard.take() {
            if task.is_finished() {
                let _ = task.await;
            } else {
                debug!("Invalidated pending hold-start task (generation={generation})");
            }
        }
        *self.pre_overlay_frontmost_app.write().await = None;
    }

    fn clear_recorder_callbacks(recorder: &mut StreamingRecorder) {
        recorder.set_utterance_callback(None);
        recorder.set_utterance_silence_sec(None);
        recorder.set_event_sink(None);
    }

    async fn ensure_recorder_ready_for_start(
        recorder: &mut StreamingRecorder,
        context: &str,
    ) -> Result<()> {
        if recorder.recorder.is_active() {
            warn!("{context}: recorder already active before start; forcing stale-session stop");
            recorder
                .stop_and_discard_path()
                .await
                .with_context(|| format!("{context}: failed stale-session stop"))?;
            info!("{context}: stale recorder stopped before start");
        }

        Self::clear_recorder_callbacks(recorder);
        Ok(())
    }

    /// Atomically reset the full set of session-lifecycle fields owned by the
    /// controller and flip `state` to Idle as the final mutation.
    ///
    /// This is the single source of truth for which fields constitute "session
    /// state" so the various reset entry points (start-failure, finished
    /// recording, toggle-stop, nuclear reset) can no longer drift apart in the
    /// subset of fields they clear (P3.1). Each caller keeps its own UI /
    /// telemetry / status-string tail.
    ///
    /// Ordering note (P2.2): every satellite flag is cleared before
    /// `set_state(State::Idle)` so cross-thread readers (e.g. the VAD monitor
    /// polling `current_state`) never observe Idle alongside stale flags.
    async fn reset_session_fields(&self) {
        *self.assistive_mode.write().await = false;
        *self.hold_mode.write().await = HoldMode::Raw;
        *self.force_raw_mode.write().await = false;
        *self.force_ai_mode.write().await = false;
        *self.session_id.write().await = None;
        *self.assistive_context.write().await = None;
        *self.pre_overlay_frontmost_app.write().await = None;
        self.start_transition_in_flight
            .store(false, Ordering::SeqCst);
        self.assistive_loop_active.store(false, Ordering::SeqCst);
        self.toggle_user_has_text.store(false, Ordering::SeqCst);
        self.toggle_assistant_has_text
            .store(false, Ordering::SeqCst);
        // `state` becomes Idle only once the rest of the session state is consistent.
        self.set_state(State::Idle).await;
    }

    async fn reset_session_after_start_failure(&self, context: &str) {
        warn!("{context}: resetting controller flags after failed start");
        self.reset_session_fields().await;
        set_assistive_session(false);
        reset_session_telemetry(&self.session_telemetry);
    }

    async fn reset_finished_recording_state(&self) {
        self.reset_session_fields().await;
        set_assistive_session(false);
    }

    async fn handle_processed_recording_result(
        &self,
        assistive: bool,
        result: &Result<ProcessRecordingOutcome>,
    ) {
        match result {
            Ok(outcome) => {
                info!("Processing finished successfully. State reset to IDLE.");

                // The transcription just freed large transient buffers (audio,
                // mel, model scratch). Hand those freed-but-retained pages back
                // to the OS now, while idle, instead of letting phys_footprint
                // creep up across a long session.
                codescribe_core::memory::release_freed_heap();

                if let Some(reason) = outcome.no_speech_reason.as_deref() {
                    info!("NoSpeech outcome in finish_recording: reason={reason}");
                } else if !assistive {
                    let cfg = self.config.read().await.clone();

                    if outcome.transcript_present
                        && cfg.transcription_overlay_enabled
                        && !(cfg.quick_notes_enabled && cfg.quick_notes_save_only)
                    {
                        let reason = outcome
                            .commit_trigger
                            .as_deref()
                            .unwrap_or("quality_gate_clean");
                        info!("COMMIT decision: trigger={reason}");
                    } else if cfg.quick_notes_enabled && cfg.quick_notes_save_only {
                        info!("COMMIT decision: skipped (quick_notes_save_only)");
                    } else {
                        info!("COMMIT decision: skipped (quality gate clean)");
                    }
                }
            }
            Err(e) => {
                error!("Processing failed: {}", e);
            }
        }
    }

    fn is_already_in_progress_error(error: &anyhow::Error) -> bool {
        error
            .to_string()
            .contains("Recording is already in progress")
    }

    async fn recover_stale_recorder_if_idle(&self) {
        if self.start_transition_in_flight.load(Ordering::SeqCst) {
            debug!("RECOVERY decision: skip idle-recovery while start transition is in-flight");
            return;
        }

        let _serial_guard = self.serial_lock.lock().await;

        if self.start_transition_in_flight.load(Ordering::SeqCst) {
            debug!(
                "RECOVERY decision: skip idle-recovery after lock (start transition still active)"
            );
            return;
        }

        if *self.state.read().await != State::Idle {
            return;
        }

        let mut recorder_guard = self.recorder.lock().await;
        let Some(recorder) = recorder_guard.as_mut() else {
            return;
        };
        if !recorder.recorder.is_active() {
            return;
        }

        warn!("Recorder recovery: detected active stream while controller is IDLE; forcing stop");
        if let Err(e) = recorder.stop_and_discard_path().await {
            warn!("Recorder recovery: forced stop failed: {e}");
        }
        Self::clear_recorder_callbacks(recorder);
        drop(recorder_guard);

        *self.assistive_mode.write().await = false;
        *self.hold_mode.write().await = HoldMode::Raw;
        *self.force_raw_mode.write().await = false;
        *self.force_ai_mode.write().await = false;
        *self.assistive_context.write().await = None;
        *self.session_id.write().await = None;
        self.assistive_loop_active.store(false, Ordering::SeqCst);
        self.toggle_user_has_text.store(false, Ordering::SeqCst);
        self.toggle_assistant_has_text
            .store(false, Ordering::SeqCst);
        set_assistive_session(false);
        reset_session_telemetry(&self.session_telemetry);
        info!("RECOVERY decision: stale active stream cleared, controller remains IDLE");
    }

    fn configure_hold_event_sink(
        recorder: &mut StreamingRecorder,
        preview_deltas_enabled: bool,
        event_broadcast: broadcast::Sender<IpcEvent>,
        session_telemetry: SharedSessionTelemetry,
    ) {
        let tb = recorder.transcript_buffer_handle();
        let delta_sink = preview_deltas_enabled.then(|| {
            Arc::new(helpers::RoutingDeltaSink)
                as Arc<dyn codescribe_core::pipeline::contracts::DeltaSink>
        });
        let pe: Arc<dyn codescribe_core::pipeline::contracts::EventSink> =
            Arc::new(PresentationEmitter::new(tb, delta_sink, None));
        let ipc_sink: Arc<dyn codescribe_core::pipeline::contracts::EventSink> =
            Arc::new(helpers::IpcBroadcastSink::new(event_broadcast));
        let telemetry_sink: Arc<dyn codescribe_core::pipeline::contracts::EventSink> =
            Arc::new(helpers::SessionTelemetrySink::new(session_telemetry));
        recorder.set_event_sink(Some(Arc::new(
            codescribe_core::pipeline::sinks::FanoutEventSink::new(vec![
                pe,
                ipc_sink,
                telemetry_sink,
            ]),
        )));
    }

    fn configure_toggle_event_sink(
        recorder: &mut StreamingRecorder,
        preview_deltas_enabled: bool,
        _flush_voice_chat_on_vad_end: bool,
        event_broadcast: broadcast::Sender<IpcEvent>,
        session_telemetry: SharedSessionTelemetry,
    ) {
        // Hands-off is ONE continuous recorder session (ADR 2026-05-28 Faza 1).
        // Normal hands-off uses cumulative SessionRendered deltas in the transcription overlay.
        //
        // Assistive hands-off is intentionally callback-driven: every finalized utterance
        // appends into the current chat user bubble, and VAD end commits that bubble to the
        // agent without stopping the recorder. Do not route assistive live preview deltas
        // into the same bubble, or previews and finals will duplicate.
        let tb = recorder.transcript_buffer_handle();
        let delta_sink = preview_deltas_enabled.then(|| {
            Arc::new(helpers::RoutingDeltaSink)
                as Arc<dyn codescribe_core::pipeline::contracts::DeltaSink>
        });
        let pe = PresentationEmitter::new(tb, delta_sink, None);

        let pe: Arc<dyn codescribe_core::pipeline::contracts::EventSink> = Arc::new(pe);
        let ipc_sink: Arc<dyn codescribe_core::pipeline::contracts::EventSink> =
            Arc::new(helpers::IpcBroadcastSink::new(event_broadcast));
        let telemetry_sink: Arc<dyn codescribe_core::pipeline::contracts::EventSink> =
            Arc::new(helpers::SessionTelemetrySink::new(session_telemetry));
        recorder.set_event_sink(Some(Arc::new(
            codescribe_core::pipeline::sinks::FanoutEventSink::new(vec![
                pe,
                ipc_sink,
                telemetry_sink,
            ]),
        )));
    }

    /// Handle hotkey event - main entry point for state machine
    ///
    /// # Arguments
    /// * `event` - The hotkey event to process
    ///
    /// This method implements the state machine logic and delegates to
    /// appropriate handlers based on current state and event type.
    ///
    /// ## Mode Determination (NEW architecture):
    /// - **Hold + assistive=false**: force RAW mode (ignores AI_FORMATTING_ENABLED)
    /// - **Hold + assistive=true**: force Assistive mode (Shift pressed = AI augmentation)
    /// - **Toggle + force_ai=true**: force AI formatting (normal hands-off)
    /// - **Toggle + assistive=true**: force Assistive hands-off
    pub async fn handle_hotkey_event(&self, event: HotkeyInput) -> Result<()> {
        let mut current_state = self.current_state().await;

        if current_state == State::Idle {
            self.recover_stale_recorder_if_idle().await;
            current_state = self.current_state().await;
        }

        debug!(
            "Hotkey event: type={:?} action={:?} assistive={} hold_mode={:?} force_raw={} force_ai={} state={}",
            event.key_type,
            event.action,
            event.assistive,
            event.hold_mode,
            event.force_raw,
            event.force_ai,
            current_state
        );

        if should_block_hotkey_during_agent_send(
            current_state,
            &event,
            helpers::is_agent_send_in_flight(),
        ) {
            info!("Agent response is still streaming; ignoring hotkey start");
            return Ok(());
        }

        // Update mode flags from event (supports mid-hold mode changes via Press events).
        // A toggle press while already in RecToggle means "stop this session"; it must not
        // rewrite the active session identity with the key that happened to stop it.
        if should_apply_incoming_mode_flags(current_state, &event) {
            match event.key_type {
                HotkeyType::Hold => {
                    *self.hold_mode.write().await = event.hold_mode;
                    match event.hold_mode {
                        HoldMode::Raw => {
                            // If we're already in an assistive session (Chat/Selection) and the user
                            // releases Shift/Cmd while still holding Ctrl, the event tap will emit a
                            // HoldUpdate back to Raw. We *do not* want to flip the UI back to the
                            // transcription overlay mid-session (it looks like the chat "blinks"
                            // and then disappears).
                            //
                            // We treat assistive mode as "latched" for the duration of a recording.
                            if matches!(current_state, State::RecHold | State::RecToggle)
                                && *self.assistive_mode.read().await
                            {
                                debug!("Ignoring Raw hold-mode update during assistive session");
                                return Ok(());
                            }

                            *self.assistive_mode.write().await = false;
                            *self.assistive_context.write().await = None;
                            *self.force_raw_mode.write().await = !event.force_ai;
                            *self.force_ai_mode.write().await = event.force_ai;

                            if matches!(current_state, State::RecHold | State::RecToggle) {
                                set_assistive_session(false);
                            }
                        }
                        HoldMode::Chat => {
                            *self.assistive_mode.write().await = true;
                            *self.force_raw_mode.write().await = false;
                            *self.force_ai_mode.write().await = false;
                            *self.assistive_context.write().await = None;

                            // If we switch modes while already recording, update UI immediately.
                            if matches!(current_state, State::RecHold | State::RecToggle) {
                                let prior_frontmost_app =
                                    self.pre_overlay_frontmost_app.read().await.clone();
                                let ctx = tokio::task::spawn_blocking(move || {
                                    capture_frontmost_app_only_with_prior_frontmost(
                                        prior_frontmost_app,
                                    )
                                })
                                .await
                                .unwrap_or_default();
                                *self.assistive_context.write().await = Some(ctx);
                                set_assistive_session(true);
                            }
                        }
                        HoldMode::Selection => {
                            *self.assistive_mode.write().await = true;
                            *self.force_raw_mode.write().await = false;
                            *self.force_ai_mode.write().await = false;
                            *self.assistive_context.write().await = None;

                            // If we switch modes while already recording, update UI immediately.
                            if matches!(current_state, State::RecHold | State::RecToggle) {
                                let prior_frontmost_app =
                                    self.pre_overlay_frontmost_app.read().await.clone();
                                let ctx = tokio::task::spawn_blocking(move || {
                                    capture_assistive_context_with_prior_frontmost(
                                        prior_frontmost_app,
                                    )
                                })
                                .await
                                .unwrap_or_default();
                                *self.assistive_context.write().await = Some(ctx);
                                set_assistive_session(true);
                            }
                        }
                    }
                }
                HotkeyType::Toggle => {
                    *self.hold_mode.write().await = HoldMode::Raw;
                    *self.assistive_context.write().await = None;

                    *self.assistive_mode.write().await = event.assistive;
                    *self.force_raw_mode.write().await = event.force_raw;
                    *self.force_ai_mode.write().await = event.force_ai;
                }
                HotkeyType::Conversation => {
                    *self.hold_mode.write().await = HoldMode::Raw;
                    *self.assistive_context.write().await = None;
                    // Conversation mode - full-duplex (no raw/ai flags)
                    *self.assistive_mode.write().await = false;
                    *self.force_raw_mode.write().await = false;
                    *self.force_ai_mode.write().await = false;
                }
            }
        } else if matches!(event.action, HotkeyAction::Press)
            && event.key_type == HotkeyType::Toggle
            && current_state == State::RecToggle
        {
            debug!(
                "Preserving active toggle session flags during stop event (event assistive={} force_raw={} force_ai={})",
                event.assistive, event.force_raw, event.force_ai
            );
        }

        // Ignore all hotkeys when busy. `State::Busy` covers the active audio
        // pipeline: recorder drain → transcription → (for the hold/toggle
        // dictation path) the final assistive agent turn, which is awaited while
        // `serial_lock` is held. Letting a second start through here would race a
        // live audio/transcription pipeline, so it stays blocked unconditionally
        // (acceptance: "non-assistive busy/audio/transcription paths remain
        // protected; do not run two audio pipelines concurrently").
        //
        // Assistive "Talk Anytime" is handled one gate up, at the `Idle` agent-
        // send gate (`should_block_hotkey_during_agent_send`): once a turn is
        // dispatched in the background the controller returns to `Idle` and the
        // mic is free, which is the only state where overlapping a new recording
        // is safe.
        if current_state == State::Busy {
            info!("App busy; ignoring hotkey event");
            return Ok(());
        }

        // Route to appropriate handler
        match event.key_type {
            HotkeyType::Hold => self.handle_hold_event(event).await,
            HotkeyType::Toggle => self.handle_toggle_event(event).await,
            HotkeyType::Conversation => self.handle_conversation_event(event).await,
        }
    }

    /// Handle hold-type hotkey events
    async fn handle_hold_event(&self, event: HotkeyInput) -> Result<()> {
        match event.action {
            HotkeyAction::Down => {
                let current_state = self.current_state().await;
                if current_state == State::Idle {
                    self.schedule_hold_start(event.assistive).await?;
                }
            }
            HotkeyAction::Up => {
                let current_state = self.current_state().await;
                if current_state == State::RecHold {
                    info!("Hold released; finishing recording");
                    self.finish_recording().await?;
                } else {
                    // Cancel the delayed start if user released before delay elapsed
                    self.cancel_pending_hold_start().await;
                }
            }
            HotkeyAction::Press => {
                // Hold keys don't use press events
            }
        }
        Ok(())
    }

    /// Handle toggle-type hotkey events
    async fn handle_toggle_event(&self, event: HotkeyInput) -> Result<()> {
        if event.action != HotkeyAction::Press {
            return Ok(());
        }

        let current_state = self.current_state().await;

        match current_state {
            State::Idle => {
                self.start_toggle_recording(event.assistive).await?;
            }
            State::RecToggle => {
                info!("Toggle pressed; entering stop flow (state=REC_TOGGLE)");
                self.assistive_loop_active.store(false, Ordering::SeqCst);
                let toggle_assistive = *self.assistive_mode.read().await;
                if toggle_assistive {
                    // Assistive hands-off: finalize the cumulative Emil bubble and invoke
                    // the agent ONCE with the complete session. (ADR 2026-05-28 Faza 1.)
                    self.stop_toggle_recording().await?;
                } else {
                    // Normal hands-off: ALWAYS save WAV + run the final pass and enter
                    // decision mode. Never discard the session audio. (ADR 2026-05-28 Faza 1.)
                    self.stop_toggle_and_adjudicate().await?;
                }
            }
            State::RecHold => {
                // Safety/UX: if a hands-off toggle is triggered while in hold recording
                // (e.g., due to short HOLD_START_DELAY_MS or user timing), allow it to stop.
                // We only do this for RAW toggle to avoid surprising behavior for Option toggles.
                if event.force_raw {
                    info!("RAW toggle pressed during hold recording; finishing recording");
                    self.assistive_loop_active.store(false, Ordering::SeqCst);
                    self.finish_recording().await?;
                } else {
                    debug!("Toggle event ignored in REC_HOLD (force_raw=false)");
                }
            }
            State::Busy => {
                warn!(
                    "Toggle pressed while previous stop is still processing (state=BUSY). \
                     If recording badge persists, stop watchdog will force recovery within 45s."
                );
            }
            _ => {
                debug!("Toggle event ignored in state {}", current_state);
            }
        }

        Ok(())
    }

    /// Handle conversation-mode hotkey events (Ctrl+Option)
    ///
    /// Conversation mode is full-duplex: simultaneous mic → Moshi → speaker.
    async fn handle_conversation_event(&self, event: HotkeyInput) -> Result<()> {
        match event.action {
            HotkeyAction::Down => {
                let current_state = self.current_state().await;
                if current_state == State::Idle {
                    self.start_conversation_mode().await?;
                }
            }
            HotkeyAction::Up => {
                let current_state = self.current_state().await;
                if current_state == State::Conversation {
                    info!("Conversation mode key released; stopping");
                    self.stop_conversation_mode().await?;
                }
            }
            HotkeyAction::Press => {
                // Conversation keys don't use press events
            }
        }
        Ok(())
    }

    /// Start conversation mode (full-duplex Moshi)
    ///
    /// Initializes ConversationEngine and AudioPlayer, then starts the audio
    /// processing loop that feeds mic input to Moshi and plays responses.
    async fn start_conversation_mode(&self) -> Result<()> {
        info!("Starting conversation mode (Moshi full-duplex)");

        {
            let recorder_guard = self.recorder.lock().await;
            if recorder_guard.is_none() {
                let error = Self::recorder_unavailable_error("Conversation-start");
                return Err(error);
            }
        }

        // 1. Initialize ConversationEngine if needed (lazy init)
        {
            let mut engine_guard = self.conversation_engine.lock().await;
            if engine_guard.is_none() {
                info!("Lazy-initializing ConversationEngine...");
                let config = MoshiConfig::default();
                match ConversationEngine::new(config) {
                    Ok(mut engine) => {
                        // Pre-initialize to load models now (rather than on first audio)
                        if let Err(e) = engine.init() {
                            error!("ConversationEngine init failed: {}", e);
                            return Err(e);
                        }
                        *engine_guard = Some(engine);
                        info!("ConversationEngine initialized successfully");
                    }
                    Err(e) => {
                        error!("Failed to create ConversationEngine: {}", e);
                        return Err(e);
                    }
                }
            }
        }

        // 2. Initialize AudioPlayer if needed (lazy init)
        {
            let mut player_guard = self.audio_player.lock().await;
            if player_guard.is_none() {
                info!("Lazy-initializing AudioPlayer...");
                match AudioPlayer::new() {
                    Ok(player) => {
                        *player_guard = Some(player);
                        info!("AudioPlayer initialized");
                    }
                    Err(e) => {
                        warn!("AudioPlayer init failed, using dummy: {}", e);
                        *player_guard = Some(AudioPlayer::dummy());
                    }
                }
            }
        }

        // 3. Reset stop flag and increment session generation
        self.conversation_stop_flag.store(false, Ordering::SeqCst);
        let generation = self.conversation_generation.fetch_add(1, Ordering::SeqCst) + 1;
        info!("Starting conversation session generation {}", generation);

        // 4. Set conversation session flag
        helpers::set_conversation_session(true);

        // 5. Transition to CONVERSATION state
        self.set_state(State::Conversation).await;
        info!("STATE TRANSITION: IDLE → CONVERSATION");

        // 7. Start the conversation audio processing task
        let engine = Arc::clone(&self.conversation_engine);
        let player = Arc::clone(&self.audio_player);
        let stop_flag = Arc::clone(&self.conversation_stop_flag);
        let generation_arc = Arc::clone(&self.conversation_generation);
        let state = Arc::clone(&self.state);
        let recorder = Arc::clone(&self.recorder);
        let event_broadcast = self.event_broadcast.clone();

        let task = tokio::spawn(async move {
            Self::conversation_audio_loop(
                engine,
                player,
                recorder,
                stop_flag,
                generation_arc,
                generation,
                state,
                event_broadcast,
            )
            .await;
        });

        *self.conversation_task.lock().await = Some(task);

        Ok(())
    }

    /// The main conversation audio processing loop
    ///
    /// Runs in a background task: captures audio → ConversationEngine → speaker
    // allow(too_many_arguments): spawn boundary of the conversation loop — each
    // Arc/channel is moved into the task; bundling into a struct would hide
    // which shared handles cross the thread boundary.
    #[allow(clippy::too_many_arguments)]
    async fn conversation_audio_loop(
        engine: Arc<Mutex<Option<ConversationEngine>>>,
        player: Arc<Mutex<Option<AudioPlayer>>>,
        recorder: Arc<Mutex<Option<StreamingRecorder>>>,
        stop_flag: Arc<AtomicBool>,
        generation_counter: Arc<AtomicU64>,
        my_generation: u64,
        state: Arc<RwLock<State>>,
        event_broadcast: broadcast::Sender<IpcEvent>,
    ) {
        info!(
            "Conversation audio loop started (generation {})",
            my_generation
        );

        // Create audio channel for conversation mode
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<f32>>(100);

        // Guard against concurrent playback
        let playback_active = Arc::new(AtomicBool::new(false));

        // Start recorder with callback that sends to our channel
        let tx_clone = tx.clone();
        {
            let mut rec_guard = recorder.lock().await;
            let rec = match Self::recorder_from_guard_mut(&mut rec_guard, "Conversation-loop start")
            {
                Ok(rec) => rec,
                Err(error) => {
                    error!("Conversation mode unavailable: {error}");
                    drop(rec_guard);
                    // Full cleanup on failure: state, session flag, badge
                    Self::set_state_with_broadcast(&state, &event_broadcast, State::Idle).await;
                    helpers::set_conversation_session(false);
                    codescribe_core::memory::release_freed_heap();
                    return;
                }
            };
            rec.recorder.set_callback(Box::new(move |data: &[f32]| {
                let _ = tx_clone.try_send(data.to_vec());
            }));

            if let Err(e) = rec.recorder.start().await {
                error!("Failed to start recorder for conversation: {}", e);
                // Full cleanup on failure: state, session flag, badge
                Self::set_state_with_broadcast(&state, &event_broadcast, State::Idle).await;
                helpers::set_conversation_session(false);
                codescribe_core::memory::release_freed_heap();
                return;
            }
        }

        // Get actual sample rate from recorder
        let sample_rate = {
            let rec_guard = recorder.lock().await;
            let rec = match Self::recorder_from_guard(&rec_guard, "Conversation-loop sample rate") {
                Ok(rec) => rec,
                Err(error) => {
                    error!("Conversation mode aborted: {error}");
                    drop(rec_guard);
                    Self::set_state_with_broadcast(&state, &event_broadcast, State::Idle).await;
                    helpers::set_conversation_session(false);
                    codescribe_core::memory::release_freed_heap();
                    return;
                }
            };
            rec.recorder.actual_sample_rate()
        };
        info!("Conversation mode: recording at {}Hz", sample_rate);

        // Processing loop
        let mut last_response_check = std::time::Instant::now();
        let response_check_interval = Duration::from_millis(100);

        while !stop_flag.load(Ordering::SeqCst) {
            // Process incoming audio chunks
            match tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
                Ok(Some(samples)) => {
                    // Feed audio to ConversationEngine
                    let mut engine_guard = engine.lock().await;
                    if let Some(ref mut eng) = *engine_guard
                        && let Err(e) = eng.process_audio_any_rate(&samples, sample_rate)
                    {
                        warn!("ConversationEngine.process_audio error: {}", e);
                    }
                }
                Ok(None) => {
                    // Channel closed
                    break;
                }
                Err(_) => {
                    // Timeout - check for responses
                }
            }

            // Periodically check for and play responses
            if last_response_check.elapsed() >= response_check_interval {
                last_response_check = std::time::Instant::now();

                let mut engine_guard = engine.lock().await;
                if let Some(ref mut eng) = *engine_guard
                    && let Some(response_samples) = eng.get_response()
                {
                    let response_len = response_samples.len();
                    let response_rate = eng.sample_rate();
                    drop(engine_guard); // Release lock before blocking playback

                    info!(
                        "Playing response: {} samples ({:.2}s @ {}Hz)",
                        response_len,
                        response_len as f32 / response_rate as f32,
                        response_rate
                    );

                    // Guard: skip if playback already in progress
                    if playback_active.swap(true, Ordering::SeqCst) {
                        info!("Skipping response - playback already active");
                        continue;
                    }

                    // Play response audio in separate blocking task (non-blocking for loop)
                    // This preserves full-duplex: we can still process mic while playing
                    let player_clone = Arc::clone(&player);
                    let playback_active_clone = Arc::clone(&playback_active);

                    let handle = tokio::runtime::Handle::current();
                    // Run the playback body on a blocking worker. catch_unwind is
                    // placed INSIDE the closure so it actually wraps the playback
                    // body that runs on the worker thread (the previous version
                    // wrapped only the spawn_blocking() call, which never panics
                    // synchronously, so a panic in p.play()/block_on/UI update was
                    // never caught). On Err we log the panic payload as the root
                    // cause (P1.2).
                    //
                    // Reliability caveat: under panic="abort" (release builds) a
                    // panic aborts the process before catch_unwind or the
                    // PlaybackGuard Drop can run, so this recovery is effective
                    // only under panic="unwind" (debug/tests). The real fix for
                    // the release crash symptom is owned by the panic group
                    // (panic hook P0.1 + abort/unwind decision P1.1).
                    tokio::task::spawn_blocking(move || {
                        // Resets playback_active when this scope exits (also on an
                        // unwinding panic; NOT under panic="abort", see above).
                        struct PlaybackGuard(Arc<AtomicBool>);
                        impl Drop for PlaybackGuard {
                            fn drop(&mut self) {
                                self.0.store(false, Ordering::SeqCst);
                            }
                        }
                        let _guard = PlaybackGuard(Arc::clone(&playback_active_clone));

                        let body = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            // Block this thread for playback, but don't block the async loop
                            let player_guard = handle.block_on(player_clone.lock());
                            if let Some(ref p) = *player_guard
                                && let Err(e) = p.play(&response_samples, response_rate)
                            {
                                warn!("AudioPlayer.play error: {}", e);
                            }
                        }));

                        if let Err(panic_payload) = body {
                            let root_cause = panic_payload
                                .downcast_ref::<&str>()
                                .map(|s| s.to_string())
                                .or_else(|| panic_payload.downcast_ref::<String>().cloned())
                                .unwrap_or_else(|| "<non-string panic payload>".to_string());
                            warn!(
                                "Playback task panicked (root cause: {root_cause}); \
                                 playback_active reset by guard"
                            );
                        }
                        // _guard dropped here, resetting playback_active.
                    });
                }
            }
        }

        // Cleanup: stop recorder
        {
            let mut rec_guard = recorder.lock().await;
            if let Some(rec) = rec_guard.as_mut() {
                let _ = rec.recorder.stop().await;
            }
        }

        // Full cleanup if loop exits unexpectedly (e.g., channel closed)
        // This ensures state/UI consistency even without stop_conversation_mode()
        // CRITICAL: Only cleanup if THIS is still the current session (generation check)
        // This prevents "old loop kills new session" race when stop_conversation_mode() times out
        let current_gen = generation_counter.load(Ordering::SeqCst);
        let current_state = *state.read().await;

        if current_state == State::Conversation && current_gen == my_generation {
            // This loop owns the current session - safe to cleanup
            stop_flag.store(true, Ordering::SeqCst);

            Self::set_state_with_broadcast(&state, &event_broadcast, State::Idle).await;
            helpers::set_conversation_session(false);
            // Return freed host memory to the OS after a conversation session
            // (the dictation stop path already does this; conversation exits did
            // not, leaving malloc retention). Memory-lifecycle only.
            codescribe_core::memory::release_freed_heap();
            info!(
                "Loop cleanup: conversation ended unexpectedly (gen {})",
                my_generation
            );
        } else if current_gen != my_generation {
            // New session started - don't touch anything
            info!(
                "Loop cleanup skipped: new session started (my_gen={}, current_gen={})",
                my_generation, current_gen
            );
        }

        info!("Conversation audio loop ended (gen {})", my_generation);
    }

    /// Stop conversation mode
    ///
    /// Signals the audio loop to stop and waits for cleanup.
    async fn stop_conversation_mode(&self) -> Result<()> {
        info!("Stopping conversation mode");

        // 1. Signal stop
        self.conversation_stop_flag.store(true, Ordering::SeqCst);

        // 2. Clear conversation session flag (before any cleanup)
        helpers::set_conversation_session(false);

        // 3. Stop recorder BEFORE waiting for task (prevents leak on abort)
        {
            let mut rec_guard = self.recorder.lock().await;
            if let Some(rec) = rec_guard.as_mut() {
                let _ = rec.recorder.stop().await;
                info!("Recorder stopped in stop_conversation_mode");
            } else {
                warn!("stop_conversation_mode: recorder unavailable during stop");
            }
        }

        // 4. Wait for conversation task to finish (with timeout)
        let task = self.conversation_task.lock().await.take();
        if let Some(handle) = task {
            match tokio::time::timeout(Duration::from_secs(3), handle).await {
                Ok(Ok(())) => info!("Conversation task finished cleanly"),
                Ok(Err(e)) => warn!("Conversation task panicked: {}", e),
                Err(_) => {
                    warn!("Conversation task timeout - task will be aborted");
                    // Task aborted, but recorder already stopped above - no leak
                }
            }
        }

        // 6. Reset ConversationEngine state
        {
            let mut engine_guard = self.conversation_engine.lock().await;
            if let Some(ref mut eng) = *engine_guard {
                eng.reset();
            }
        }

        // 7. Transition back to IDLE
        self.set_state(State::Idle).await;
        // Return freed host memory after a conversation session (see note above).
        codescribe_core::memory::release_freed_heap();
        info!("STATE TRANSITION: CONVERSATION → IDLE");

        Ok(())
    }

    /// Schedule delayed recording start for hold mode
    async fn schedule_hold_start(&self, assistive: bool) -> Result<()> {
        // Hold mode never runs the assistive loop
        self.assistive_loop_active.store(false, Ordering::SeqCst);
        let config = self.config.read().await.clone();
        let configured_delay_ms = config.hold_start_delay_ms;
        let delay_ms = effective_hold_start_delay_ms(configured_delay_ms, assistive);
        let beep = config.beep_on_start;
        let sound_volume = config.sound_volume;
        let language = config.whisper_language;

        let hold_mode = Arc::clone(&self.hold_mode);

        debug!(
            "Scheduling hold-start after {}ms delay (configured={}ms, assistive={}, hold_mode={:?})",
            delay_ms,
            configured_delay_ms,
            assistive,
            *hold_mode.read().await
        );

        // Cancel any existing delayed start
        self.cancel_pending_hold_start().await;
        let task_generation = self.hold_start_generation.load(Ordering::SeqCst);

        let pre_overlay_frontmost_app = tokio::task::spawn_blocking(capture_frontmost_app_only)
            .await
            .ok()
            .and_then(|ctx| ctx.frontmost_app);
        *self.pre_overlay_frontmost_app.write().await = pre_overlay_frontmost_app;

        // Reset VAD flag for new session
        self.vad_triggered.store(false, Ordering::SeqCst);

        let state = Arc::clone(&self.state);
        let session_id = Arc::clone(&self.session_id);
        let recorder = Arc::clone(&self.recorder);
        let delay = Duration::from_millis(delay_ms);
        let vad_flag = Arc::clone(&self.vad_triggered);
        let assistive_context = Arc::clone(&self.assistive_context);
        let pre_overlay_frontmost_app = Arc::clone(&self.pre_overlay_frontmost_app);
        let event_broadcast = self.event_broadcast.clone();
        let serial_lock = Arc::clone(&self.serial_lock);
        let hold_start_generation = Arc::clone(&self.hold_start_generation);
        let start_transition_in_flight = Arc::clone(&self.start_transition_in_flight);
        let session_telemetry = Arc::clone(&self.session_telemetry);

        let task = tokio::spawn(async move {
            // Wait for the configured delay
            tokio::time::sleep(delay).await;

            if hold_start_generation.load(Ordering::SeqCst) != task_generation {
                debug!("Hold-start cancelled: superseded generation before lock");
                return;
            }

            // Serialize with other start/stop operations.
            let _serial_guard = serial_lock.lock().await;

            if hold_start_generation.load(Ordering::SeqCst) != task_generation {
                debug!("Hold-start cancelled: superseded generation while waiting for lock");
                return;
            }

            // Check if we're still in IDLE state
            let current_state = *state.read().await;
            if current_state != State::Idle {
                debug!("Hold-start cancelled: state changed to {}", current_state);
                return;
            }

            if hold_start_generation.load(Ordering::SeqCst) != task_generation {
                debug!("Hold-start cancelled: superseded generation before recorder start");
                return;
            }

            let current_state = *state.read().await;
            if current_state != State::Idle {
                debug!(
                    "Hold-start cancelled before recorder start: state changed to {}",
                    current_state
                );
                return;
            }

            let _start_guard = AtomicFlagGuard::new(Arc::clone(&start_transition_in_flight));

            // Generate session ID
            let new_session_id = Uuid::new_v4().to_string();
            *session_id.write().await = Some(new_session_id.clone());

            info!("Starting hold recording (session={})", new_session_id);

            let hold_mode = *hold_mode.read().await;
            let is_assistive = matches!(hold_mode, HoldMode::Chat | HoldMode::Selection);
            // Cursor-following recording badge (config-gated): red for hold dictation,
            // purple for assistive/agent. Works headless — no overlay needed.
            if config.hold_indicator {
                crate::os::hold_badge::show_badge_for_mode(if is_assistive {
                    crate::os::hold_badge::BadgeMode::Assistive
                } else {
                    crate::os::hold_badge::BadgeMode::Hold
                });
            }
            let overlay_enabled = apply_runtime_transcription_profile(&config, is_assistive);

            // Start the recorder (skip in tests: no CoreAudio device needed)
            // hang_sec is derived from hardcoded VAD defaults (single source of truth).
            let mut rec_guard = recorder.lock().await;
            let rec = match Self::recorder_from_guard_mut(&mut rec_guard, "Hold-start") {
                Ok(rec) => rec,
                Err(error) => {
                    error!("Hold-start aborted: {error}");
                    drop(rec_guard);
                    *session_id.write().await = None;
                    set_assistive_session(false);
                    return;
                }
            };
            if let Err(e) = Self::ensure_recorder_ready_for_start(rec, "Hold-start preflight").await
            {
                error!("Hold-start aborted: {e}");
                drop(rec_guard);
                *session_id.write().await = None;
                set_assistive_session(false);
                return;
            }
            // Hold-to-talk: the key-down is the source of truth. Don't auto-stop mid-hold.
            rec.recorder.config.auto_silence = false;
            rec.recorder.set_on_vad_stop(move || {
                info!("VAD callback: setting vad_triggered flag");
                vad_flag.store(true, Ordering::SeqCst);
            });

            // Set session mode for delta routing BEFORE starting the pipeline,
            // so the very first deltas route to the correct overlay.
            set_assistive_session(is_assistive);
            reset_session_telemetry(&session_telemetry);

            // Runtime pipeline is always event-based. Hold mode has no utterance callback;
            // text is finalized on key-up in `finish_recording`.
            Self::configure_hold_event_sink(
                rec,
                is_assistive || overlay_enabled,
                event_broadcast.clone(),
                Arc::clone(&session_telemetry),
            );
            if !cfg!(test) {
                let language_hint = language.whisper_hint().map(str::to_string);
                // Audio-first cold start: do not preflight Whisper here. The
                // recorder starts feedback now while STT lazy-loads behind the
                // StreamingRecorder backlog.
                let start_result = rec.start_event_session(language_hint.clone()).await;
                if let Err(e) = start_result {
                    if Self::is_already_in_progress_error(&e) {
                        warn!("Hold-start hit stale recorder lock; forcing stop and retrying once");
                        if let Err(stop_err) = rec.stop_and_discard_path().await {
                            warn!("Hold-start stale-recorder recovery failed: {stop_err}");
                        }
                        Self::clear_recorder_callbacks(rec);
                        Self::configure_hold_event_sink(
                            rec,
                            is_assistive || overlay_enabled,
                            event_broadcast.clone(),
                            Arc::clone(&session_telemetry),
                        );
                        let retry_result = rec.start_event_session(language_hint).await;
                        if let Err(retry_err) = retry_result {
                            error!("Failed to start recorder after recovery: {retry_err}");
                            Self::clear_recorder_callbacks(rec);
                            *session_id.write().await = None;
                            set_assistive_session(false);
                            return;
                        }
                    } else {
                        error!("Failed to start recorder: {e}");
                        Self::clear_recorder_callbacks(rec);
                        *session_id.write().await = None;
                        set_assistive_session(false);
                        return;
                    }
                }
            }

            if hold_start_generation.load(Ordering::SeqCst) != task_generation {
                warn!("Hold-start superseded after recorder start; stopping stale session");
                if rec.recorder.is_active()
                    && let Err(stop_err) = rec.stop_and_discard_path().await
                {
                    warn!("Hold-start stale-session stop failed: {stop_err}");
                }
                Self::clear_recorder_callbacks(rec);
                *session_id.write().await = None;
                set_assistive_session(false);
                return;
            }
            drop(rec_guard);

            // Transition to REC_HOLD as soon as recorder starts to avoid IDLE/active races.
            Self::set_state_with_broadcast(&state, &event_broadcast, State::RecHold).await;
            info!(
                "STATE TRANSITION: IDLE → REC_HOLD (assistive={})",
                is_assistive
            );

            // Play start beep if enabled
            if beep {
                crate::audio::play_sound_with_volume("Tink", sound_volume);
            }

            if is_assistive {
                // Capture context BEFORE starting (paste-back / frontmost tracking).
                let prior_frontmost_app = pre_overlay_frontmost_app.read().await.clone();
                let ctx = match hold_mode {
                    HoldMode::Selection => tokio::task::spawn_blocking(move || {
                        capture_assistive_context_with_prior_frontmost(prior_frontmost_app)
                    })
                    .await
                    .unwrap_or_default(),
                    HoldMode::Chat | HoldMode::Raw => tokio::task::spawn_blocking(move || {
                        capture_frontmost_app_only_with_prior_frontmost(prior_frontmost_app)
                    })
                    .await
                    .unwrap_or_default(),
                };
                *assistive_context.write().await = Some(ctx);
            } else {
                // Capture frontmost app for paste actions (no selection/clipboard).
                let ctx = tokio::task::spawn_blocking(capture_frontmost_app_only)
                    .await
                    .unwrap_or_default();
                *assistive_context.write().await = Some(ctx);
            }
        });

        *self.hold_start_task.lock().await = Some(task);
        Ok(())
    }

    /// Start recording in toggle mode (immediate, no delay)
    async fn start_toggle_recording(&self, is_assistive: bool) -> Result<()> {
        // Acquire serial lock to prevent race conditions
        let _guard = self.serial_lock.lock().await;

        // Double-check state under lock
        let current_state = *self.state.read().await;
        if current_state != State::Idle {
            debug!(
                "start_toggle_recording: state already changed to {}",
                current_state
            );
            return Ok(());
        }
        let _start_guard = AtomicFlagGuard::new(Arc::clone(&self.start_transition_in_flight));

        // Generate session ID
        let new_session_id = Uuid::new_v4().to_string();
        *self.session_id.write().await = Some(new_session_id.clone());

        if is_assistive {
            *self.assistive_mode.write().await = true;
            *self.force_raw_mode.write().await = false;
            *self.force_ai_mode.write().await = false;
        }
        self.assistive_loop_active
            .store(is_assistive, Ordering::SeqCst);
        self.toggle_user_has_text.store(false, Ordering::SeqCst);
        self.toggle_assistant_has_text
            .store(false, Ordering::SeqCst);

        info!("Starting toggle recording (session={})", new_session_id);

        let config = self.config.read().await.clone();
        // Cursor-following recording badge (config-gated): pulsing red for toggle /
        // hands-off, purple for assistive/agent.
        if config.hold_indicator {
            crate::os::hold_badge::show_badge_for_mode(if is_assistive {
                crate::os::hold_badge::BadgeMode::Assistive
            } else {
                crate::os::hold_badge::BadgeMode::Toggle
            });
        }
        let language = config.whisper_language;
        let toggle_silence_sec = config.toggle_silence_sec;
        let beep_enabled = config.beep_on_start;
        let sound_volume = config.sound_volume;
        let overlay_enabled = apply_runtime_transcription_profile(&config, is_assistive);

        // Start the recorder
        let mut recorder_guard = self.recorder.lock().await;
        let recorder = match Self::recorder_from_guard_mut(&mut recorder_guard, "Toggle-start") {
            Ok(recorder) => recorder,
            Err(error) => {
                drop(recorder_guard);
                self.reset_session_after_start_failure("Toggle-start").await;
                return Err(error);
            }
        };
        if let Err(e) =
            Self::ensure_recorder_ready_for_start(recorder, "Toggle-start preflight").await
        {
            drop(recorder_guard);
            self.reset_session_after_start_failure("Toggle-start preflight")
                .await;
            return Err(e);
        }

        // Toggle mode: continuous recording; silence only triggers per-utterance send.
        recorder.recorder.config.auto_silence = false;
        recorder.recorder.set_on_vad_stop(|| {});
        recorder.set_utterance_silence_sec(Some(toggle_silence_sec));

        // Set session mode for delta routing BEFORE starting the pipeline,
        // so the very first deltas route to the correct overlay.
        set_assistive_session(is_assistive);
        reset_session_telemetry(&self.session_telemetry);

        // Runtime pipeline is always event-based.
        Self::configure_toggle_event_sink(
            recorder,
            !is_assistive && overlay_enabled,
            is_assistive,
            self.event_broadcast.clone(),
            Arc::clone(&self.session_telemetry),
        );

        // Skip actual audio stream in tests (no CoreAudio device needed)
        let language_hint = language.whisper_hint().map(str::to_string);
        // Audio-first cold start: do not preflight Whisper here. The recorder
        // starts feedback now while STT lazy-loads behind the StreamingRecorder backlog.
        if !cfg!(test)
            && let Err(e) = recorder.start_event_session(language_hint.clone()).await
        {
            if Self::is_already_in_progress_error(&e) {
                warn!("Toggle start hit stale recorder lock; forcing stop and retrying once");
                if let Err(stop_err) = recorder.stop_and_discard_path().await {
                    warn!("Toggle stale-recorder recovery failed: {stop_err}");
                }
                Self::clear_recorder_callbacks(recorder);
                Self::configure_toggle_event_sink(
                    recorder,
                    !is_assistive && overlay_enabled,
                    is_assistive,
                    self.event_broadcast.clone(),
                    Arc::clone(&self.session_telemetry),
                );
                if let Err(retry_err) = recorder.start_event_session(language_hint).await {
                    drop(recorder_guard);
                    self.reset_session_after_start_failure("Toggle-start retry")
                        .await;
                    return Err(anyhow::anyhow!(
                        "Failed to start event session after recovery: {retry_err}"
                    ));
                }
            } else {
                drop(recorder_guard);
                self.reset_session_after_start_failure("Toggle-start").await;
                return Err(e);
            }
        }
        drop(recorder_guard);

        // Transition to REC_TOGGLE immediately after recorder starts.
        self.set_state(State::RecToggle).await;
        info!("STATE TRANSITION: IDLE → REC_TOGGLE (pulsing badge)");

        // Reset incremental segment marker — the next Commit/Augment clips
        // from sample 0 of this new toggle session, not from any leftover
        // offset of a prior session.
        self.last_segment_audio_offset.store(0, Ordering::SeqCst);

        // Play start beep if enabled
        if beep_enabled {
            crate::audio::play_sound_with_volume("Tink", sound_volume);
        }

        if is_assistive {
            // Toggle-assistive is a hands-off chat loop with optional selection context.
            // Capture selection when available (best-effort), otherwise just app name.
            let ctx = tokio::task::spawn_blocking(capture_assistive_context)
                .await
                .unwrap_or_default();
            tokio::task::spawn_blocking({
                let ctx = ctx.clone();
                move || store_recent_assistive_context(&ctx)
            })
            .await
            .ok();
            *self.assistive_context.write().await = Some(ctx);
        } else {
            // Capture frontmost app for paste actions (no selection/clipboard).
            let ctx = tokio::task::spawn_blocking(capture_frontmost_app_only)
                .await
                .unwrap_or_default();
            *self.assistive_context.write().await = Some(ctx);
        }

        Ok(())
    }

    /// Stop the assistive (Emil) hands-off session.
    ///
    /// ADR 2026-05-28 Faza 1 contract: the assistive hands-off session is ONE
    /// continuous dictation. Deltas have already streamed cumulatively into the Emil
    /// bubble during recording (no per-utterance finalize). On stop we:
    ///   1. stop the recorder and capture the WHOLE session transcript + WAV,
    ///   2. retain the full audio (no discard),
    ///   3. finalize the bubble and invoke the agent ONCE with the complete message.
    ///
    /// Emil therefore answers the entire dictation as a single user message — never
    /// a stream of fragmented per-sentence shots.
    async fn stop_toggle_recording(&self) -> Result<()> {
        // Ignore if not recording
        if *self.state.read().await != State::RecToggle {
            return Ok(());
        }

        info!("Stopping toggle recording (assistive hands-off — single agent message)");

        let assistive = *self.assistive_mode.read().await;
        let config = self.config.read().await.clone();

        // Stop recording, capture full-session transcript + WAV path. We use `stop()`
        // (not `stop_and_discard_path()`) so the full audio is retained for the session.
        let mut recorder_guard = self.recorder.lock().await;
        let mut stop_error: Option<anyhow::Error> = None;
        let mut session_transcript = String::new();
        let mut session_audio_path: Option<std::path::PathBuf> = None;
        if let Some(recorder) = recorder_guard.as_mut() {
            if !cfg!(test) {
                match recorder.stop().await {
                    Ok((transcript, audio_path)) => {
                        session_transcript = transcript;
                        session_audio_path = audio_path;
                    }
                    Err(e) => {
                        warn!("Toggle stop: recorder stop failed; continuing cleanup: {e}");
                        stop_error = Some(e);
                    }
                }
            }
            Self::clear_recorder_callbacks(recorder);
        } else {
            let error = Self::recorder_unavailable_error("Toggle-stop");
            warn!("Toggle stop: {error}; continuing cleanup");
            stop_error = Some(error);
        }
        drop(recorder_guard);

        // Persist the full session audio (operator contract: hands-off never drops audio).
        if config.dump_audio_logs
            && let Some(path) = &session_audio_path
        {
            let _ = crate::state::history::save_audio(
                path.as_path(),
                chrono::Local::now(),
                Some(session_transcript.as_str()),
                crate::state::history::TranscriptKind::Raw,
            );
        }

        // Snapshot assistive context BEFORE clearing session state.
        let assistive_ctx = if assistive {
            let preserved = self.assistive_context.read().await.clone().filter(|ctx| {
                ctx.frontmost_app.is_some()
                    || ctx
                        .selected_text
                        .as_deref()
                        .is_some_and(|text| !text.trim().is_empty())
            });
            if preserved.is_some() {
                preserved
            } else {
                tokio::task::spawn_blocking(|| {
                    get_recent_assistive_context(Duration::from_secs(90))
                })
                .await
                .ok()
                .flatten()
            }
        } else {
            None
        };

        // Reset state. `assistive_context` was already captured above (preserved
        // path), so clearing it here via the shared helper is safe.
        self.reset_session_fields().await;

        // Assistive: finalize the cumulative bubble and invoke the agent ONCE with the
        // complete session message.
        if assistive && !session_transcript.trim().is_empty() {
            let ctx = assistive_ctx.unwrap_or_default();
            let assistive_input = build_assistive_input(&session_transcript, &ctx);
            helpers::send_assistive_with_agent_runtime(
                assistive_input,
                config.whisper_language,
                config.ai_assistive_max_tokens,
            )
            .await;
        }

        if let Some(e) = stop_error {
            return Err(anyhow::anyhow!("Failed to stop recorder: {e}"));
        }

        Ok(())
    }

    async fn stop_toggle_and_adjudicate(&self) -> Result<()> {
        if cfg!(test) {
            return self.stop_toggle_recording().await;
        }

        if *self.state.read().await != State::RecToggle {
            return Ok(());
        }

        // Watchdog: full stop+adjudicate (recorder.stop + final-pass STT + post-process
        // + paste) must complete within STOP_TIMEOUT. If it stalls — final-pass deadlock
        // on Metal device, RwLock contention, recorder.stop blocked on cpal callback —
        // force recovery to Idle so subsequent toggle presses register, badge clears,
        // and tray reflects truth instead of showing Idle while recording is hung.
        match tokio::time::timeout(STOP_TIMEOUT, self.stop_toggle_and_adjudicate_inner()).await {
            Ok(result) => result,
            Err(_) => {
                error!(
                    "Toggle stop+adjudicate stalled >{}s — forcing recovery to Idle. \
                     Recording session abandoned; future toggle presses will start fresh.",
                    STOP_TIMEOUT.as_secs()
                );
                self.recover_from_stuck_stop().await;
                Err(anyhow::anyhow!(
                    "Toggle stop timeout after {}s; state forced to Idle",
                    STOP_TIMEOUT.as_secs()
                ))
            }
        }
    }

    async fn stop_toggle_and_adjudicate_inner(&self) -> Result<()> {
        // Phase-timed instrumentation: the watchdog above wraps this entire fn
        // in a 45s timeout, but until now we couldn't tell WHICH await hung.
        // Operator reported "hands-off, double option, który potrafi wywołać
        // nagrywanie, ale nie potrafi zakończyć nagrywania" — confirmed in
        // ~/.codescribe/logs/codescribe.log @ 2026-05-13 23:03:22 PDT
        // where "Stopping toggle recording with final-pass adjudication" was
        // followed by 41s of silence before watchdog forced recovery.
        // These per-phase elapsed logs will identify the exact hang point next
        // time it reproduces. Logs MUST stay info! so they survive at default
        // tracing level — debug! gets filtered out in release.
        let stop_start = std::time::Instant::now();
        info!("stop_toggle_inner: PHASE 0 — acquiring serial_lock");
        let _guard = self.serial_lock.lock().await;
        info!(
            "stop_toggle_inner: PHASE 0 — serial_lock acquired in {:?}",
            stop_start.elapsed()
        );

        if *self.state.read().await != State::RecToggle {
            return Ok(());
        }

        info!("Stopping toggle recording with final-pass adjudication");

        let assistive = *self.assistive_mode.read().await;
        let hold_mode = *self.hold_mode.read().await;
        let force_raw = *self.force_raw_mode.read().await;
        let force_ai = *self.force_ai_mode.read().await;

        // Self-deadlock guard (Rust 2024): the read guard temporary from an
        // if-let chain scrutinee outlives the chain body. Inlining the read
        // would keep the guard alive across `.write().await`, blocking the
        // write on this same task's read guard → 45s hang reproduced in
        // ~/.codescribe/logs/codescribe.log 2026-05-14T00:16:23 (PHASE 1
        // never reached; watchdog forced recovery). Materialize the snapshot
        // first so the read guard drops at the semicolon.
        let session_id_snapshot = self.session_id.read().await.clone();
        if let Some(session_id) = session_id_snapshot {
            *self.session_id.write().await = Some(format!("{session_id}:stopping"));
        }

        self.set_state(State::Busy).await;
        self.show_processing_badge_if_enabled().await;

        let result = {
            let phase1 = std::time::Instant::now();
            info!("stop_toggle_inner: PHASE 1 — locking recorder mutex");
            let mut recorder_guard = self.recorder.lock().await;
            info!(
                "stop_toggle_inner: PHASE 1 — recorder mutex acquired in {:?}",
                phase1.elapsed()
            );

            let recorder = Self::recorder_from_guard_mut(&mut recorder_guard, "Toggle-adjudicate")?;

            let phase2 = std::time::Instant::now();
            info!("stop_toggle_inner: PHASE 2 — calling recorder.stop() (cpal drain + WAV save)");
            let (streaming_text, raw_audio_path_opt) =
                recorder.stop().await.context("Failed to stop recorder")?;
            info!(
                "stop_toggle_inner: PHASE 2 — recorder.stop() returned in {:?} (streaming_text={} chars, has_wav={})",
                phase2.elapsed(),
                streaming_text.len(),
                raw_audio_path_opt.is_some()
            );

            Self::clear_recorder_callbacks(recorder);
            drop(recorder_guard);

            let phase3 = std::time::Instant::now();
            info!(
                "stop_toggle_inner: PHASE 3 — process_stopped_recording (truth selection + post-process + paste/handoff decision)"
            );
            let r = self
                .process_stopped_recording(
                    streaming_text,
                    raw_audio_path_opt,
                    assistive,
                    hold_mode,
                    force_raw,
                    force_ai,
                    Some(RecordingTranscriptSource::ToggleSessionAdjudicated),
                )
                .await;
            info!(
                "stop_toggle_inner: PHASE 3 — process_stopped_recording completed in {:?} (ok={})",
                phase3.elapsed(),
                r.is_ok()
            );
            r
        };

        let phase4 = std::time::Instant::now();
        self.toggle_user_has_text.store(false, Ordering::SeqCst);
        self.toggle_assistant_has_text
            .store(false, Ordering::SeqCst);
        self.reset_finished_recording_state().await;
        self.handle_processed_recording_result(assistive, &result)
            .await;
        info!(
            "stop_toggle_inner: PHASE 4 — cleanup + result handler completed in {:?} (total stop time: {:?})",
            phase4.elapsed(),
            stop_start.elapsed()
        );

        result.map(|_| ())
    }

    /// Recovery path when stop_toggle_and_adjudicate exceeds STOP_TIMEOUT.
    ///
    /// Forces state to Idle and clears all toggle-related flags so subsequent
    /// toggle presses register cleanly. Does NOT attempt to recover the recorder —
    /// it may be in arbitrary state; a fresh `start_toggle_recording` reinitializes
    /// through the normal path. UI surfaces (badge, voice-chat status, overlay)
    /// are restored to Idle visuals so the user gets honest feedback that recording
    /// is no longer alive.
    async fn recover_from_stuck_stop(&self) {
        warn!("Recovery: forcing controller to Idle after stuck stop");
        self.reset_finished_recording_state().await;
    }

    pub async fn stop_recording_from_external_surface(&self) -> Result<()> {
        let current_state = self.current_state().await;
        let assistive = *self.assistive_mode.read().await;
        if should_use_toggle_adjudicated_stop(current_state, assistive, toggle_final_pass_enabled())
        {
            self.stop_toggle_and_adjudicate().await
        } else {
            self.finish_recording().await
        }
    }

    /// Stop recording, transcribe, format, and paste the result
    ///
    /// This is the core processing pipeline that:
    /// 1. Stops the audio recorder
    /// 2. Transcribes the audio via backend
    /// 3. Formats the transcript (if assistive mode enabled)
    /// 4. Pastes the result into the active application
    pub async fn finish_recording(&self) -> Result<()> {
        // Cancel any pending hold-start
        self.cancel_pending_hold_start().await;

        // Acquire serial lock to prevent concurrent finish calls
        let _guard = self.serial_lock.lock().await;

        self.finish_recording_locked().await
    }

    /// Internal finish_recording implementation (assumes lock is held)
    async fn finish_recording_locked(&self) -> Result<()> {
        let current_state = *self.state.read().await;

        // Ignore if we're not recording
        if matches!(current_state, State::Idle | State::Busy) {
            warn!(
                "finish_recording called while state={}; ignoring (race?)",
                current_state
            );
            return Ok(());
        }

        info!("Finishing recording (state={})", current_state);

        // Transition to BUSY
        debug!("STATE TRANSITION: {} → BUSY", current_state);
        self.set_state(State::Busy).await;
        self.show_processing_badge_if_enabled().await;

        // Get session ID and mode flags before we reset them
        let session_id = self.session_id.read().await.clone();
        let assistive = *self.assistive_mode.read().await;
        let hold_mode = *self.hold_mode.read().await;
        let force_raw = *self.force_raw_mode.read().await;
        let force_ai = *self.force_ai_mode.read().await;

        let result = match tokio::time::timeout(
            STOP_TIMEOUT,
            self.process_recording(session_id, assistive, hold_mode, force_raw, force_ai),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => {
                error!(
                    "Hold stop processing stalled >{}s — forcing recovery to Idle. \
                     Recording session abandoned; future hotkeys will start fresh.",
                    STOP_TIMEOUT.as_secs()
                );
                self.recover_from_stuck_stop().await;
                return Err(anyhow::anyhow!(
                    "Hold stop timeout after {}s; state forced to Idle",
                    STOP_TIMEOUT.as_secs()
                ));
            }
        };

        self.reset_finished_recording_state().await;
        self.handle_processed_recording_result(assistive, &result)
            .await;

        result.map(|_| ())
    }

    // allow(too_many_arguments): single-call-site pipeline seam carrying the
    // stop-recording context; grouping into a struct is planned with the
    // controller decomposition cut (see prune report follow-ups).
    #[allow(clippy::too_many_arguments)]
    async fn process_stopped_recording(
        &self,
        streaming_text: String,
        raw_audio_path_opt: Option<PathBuf>,
        assistive: bool,
        hold_mode: HoldMode,
        force_raw: bool,
        force_ai: bool,
        transcript_source_override: Option<RecordingTranscriptSource>,
    ) -> Result<ProcessRecordingOutcome> {
        let audio_path = if let Some(path) = raw_audio_path_opt {
            match ValidatedAudioPath::new(&path) {
                Ok(p) => Some(p),
                Err(e) => {
                    warn!("Invalid audio path: {}", e);
                    None
                }
            }
        } else {
            None
        };

        let recording_timestamp = chrono::Local::now();

        let config = self.config.read().await.clone();
        let language = config.whisper_language;
        let language_opt = language.whisper_hint();
        let use_local_stt = config.use_local_stt;
        let raw_save_enabled = raw_save_enabled(assistive);

        let cloud_config = if use_local_stt {
            None
        } else {
            match (config.stt_endpoint.clone(), config.stt_api_key.clone()) {
                (Some(endpoint), Some(api_key))
                    if !endpoint.trim().is_empty() && !api_key.trim().is_empty() =>
                {
                    Some((endpoint, api_key))
                }
                _ => None,
            }
        };

        let assistive_loop = assistive && self.assistive_loop_active.load(Ordering::SeqCst);

        let mut local_final_pass_verdict = None;
        let mut cloud_verdict_opt = None;
        let mut cloud_handle: Option<JoinHandle<Result<crate::client::CloudTranscriptionVerdict>>> =
            None;
        let mut local_final_pass_attempted = false;

        if let Some((cloud_endpoint, cloud_api_key)) = cloud_config {
            if let Some(path) = &audio_path {
                let cloud_path = path.as_path().to_path_buf();
                let cloud_language = language_opt.map(str::to_string);
                cloud_handle = Some(tokio::spawn(async move {
                    crate::client::transcribe_cloud(
                        &cloud_path,
                        cloud_language.as_deref(),
                        &cloud_endpoint,
                        &cloud_api_key,
                    )
                    .await
                }));
            } else {
                warn!("Cloud STT disabled: no audio file available");
            }
        } else if !use_local_stt {
            warn!("Cloud STT disabled: STT_ENDPOINT/STT_API_KEY missing");
        }

        let local_final_pass_enabled = std::env::var("CODESCRIBE_LOCAL_STT_FINAL_PASS")
            .ok()
            .map(|v| !matches!(v.to_lowercase().as_str(), "0" | "false" | "no" | "off"))
            .unwrap_or(true);

        if use_local_stt && local_final_pass_enabled {
            if let Some(path) = &audio_path {
                local_final_pass_attempted = true;
                let wav_path = path.as_path().to_path_buf();
                let lang = language_opt.map(str::to_string);

                info!(
                    "Running final-pass local STT adjudicator: {}",
                    wav_path.display()
                );

                match tokio::task::spawn_blocking(move || {
                    crate::whisper::transcribe_file_verdict(
                        &wav_path,
                        lang.as_deref(),
                        FileTranscriptionOptions::default(),
                    )
                })
                .await
                {
                    Ok(Ok(verdict)) => {
                        info!(
                            "Final-pass verdict captured ({} chars, speech_pct={:?}, avg_logprob={:?})",
                            verdict.text.len(),
                            verdict.vad.as_ref().map(|vad| vad.speech_pct),
                            verdict.raw.avg_logprob
                        );
                        local_final_pass_verdict = Some(verdict);
                    }
                    Ok(Err(e)) => warn!("Final-pass transcription failed: {}", e),
                    Err(e) => warn!("Final-pass transcription task failed: {}", e),
                }
            } else {
                warn!("Final-pass local STT skipped: no audio file available");
            }
        }

        if !use_local_stt {
            if let Some(handle) = cloud_handle.take() {
                info!("Awaiting cloud STT as selected transcript backend");
                match handle.await {
                    Ok(Ok(verdict)) => cloud_verdict_opt = Some(verdict),
                    Ok(Err(e)) => error!("Cloud transcription failed: {}", e),
                    Err(e) => error!("Cloud transcription task failed: {}", e),
                }
            } else {
                warn!("Cloud backend unavailable (cloud disabled or missing credentials)");
            }
        }

        let session_telemetry = snapshot_session_telemetry(&self.session_telemetry);
        let mut truth_verdict = adjudicate_recording_truth(
            use_local_stt,
            local_final_pass_attempted,
            local_final_pass_verdict,
            streaming_text,
            cloud_verdict_opt.clone(),
            &session_telemetry,
        );
        if transcript_source_override.is_some()
            && matches!(
                truth_verdict.transcript_source,
                Some(RecordingTranscriptSource::LocalFinalPass)
            )
        {
            truth_verdict.transcript_source = transcript_source_override;
            truth_verdict.display_status = truth_display_status(
                truth_verdict.transcript_source,
                truth_verdict.fallback_class,
                truth_verdict.no_speech_reason.as_deref(),
                &truth_verdict.confidence_flags,
            );
        }
        if let Some(source) = truth_verdict.transcript_source {
            if let Some(text) = truth_verdict.raw_text.as_ref() {
                info!(
                    "Adjudicated transcript source={} chars={} fallback={:?} flags={:?}",
                    source.label(),
                    text.len(),
                    truth_verdict.fallback_class,
                    truth_verdict.confidence_flags
                );
            } else {
                info!(
                    "Adjudicated transcript source={} without final text (status={})",
                    source.label(),
                    truth_verdict.display_status
                );
            }
        }

        let raw_text = match truth_verdict.raw_text.clone() {
            Some(text) if !text.trim().is_empty() => text,
            Some(_) | None => {
                let reason = session_telemetry
                    .no_speech_reason
                    .clone()
                    .unwrap_or_else(|| "empty_transcript_without_no_speech_event".to_string());
                if let Some(stats) = session_telemetry.stats.as_ref() {
                    info!(
                        "NoSpeech outcome: reason={} utterances={} hallu_drops={} semantic_drops={} filtered_empty={} corrections={} dropped_chunks={} partial_runs={} partial_trigger_utt={} partial_trigger_speech={} partial_trigger_watchdog={} partial_stale={} partial_coalesced={} partial_dropped={}",
                        reason,
                        stats.total_utterances,
                        stats.hallucination_drops,
                        stats.semantic_gate_drops,
                        stats.filtered_empty_drops,
                        stats.corrections_applied,
                        stats.dropped_audio_chunks,
                        stats.partial_runs_total,
                        stats.trigger_utterance_count,
                        stats.trigger_speech_count,
                        stats.trigger_timer_count,
                        stats.partial_stale_count,
                        stats.partial_coalesced_count,
                        stats.partial_dropped_count
                    );
                } else {
                    info!("NoSpeech outcome: reason={} stats=unavailable", reason);
                }
                if assistive_loop {
                    warn!("NoSpeech in assistive loop; continuing hands-off listening");
                }

                let final_status = if truth_verdict.display_status.trim().is_empty() {
                    "No reliable speech detected".to_string()
                } else {
                    truth_verdict.display_status.clone()
                };
                let mode_label =
                    recording_mode_label(assistive, hold_mode, force_raw, force_ai).to_string();
                let truth_metadata = RecordingTruthMetadata {
                    source: truth_verdict.transcript_source,
                    engine: truth_engine_label(truth_verdict.transcript_source),
                    mode: Some(mode_label),
                    fallback_class: truth_verdict.fallback_class,
                    fallback_used: truth_verdict.fallback_class.is_some()
                        || matches!(
                            truth_verdict.transcript_source,
                            Some(RecordingTranscriptSource::StreamingFallback)
                        ),
                    vad_speech_pct: truth_verdict.speech_pct,
                    no_speech_reason: Some(reason.clone()),
                    avg_logprob: truth_verdict.avg_logprob,
                    confidence_flags: truth_verdict.confidence_flags.clone(),
                    sparkline: truth_verdict.sparkline.clone(),
                    final_pass_disposition: truth_verdict.final_pass_disposition,
                    commit_trigger: truth_verdict.commit_trigger.clone(),
                    display_status: Some(final_status.clone()),
                };

                let failed_entry = crate::state::history::save_entry_with_timestamp_and_slug(
                    &final_status,
                    Some(recording_timestamp),
                    crate::state::history::TranscriptKind::Failed,
                    Some("no-speech"),
                );
                write_truth_sidecar_logged(&failed_entry.path, &truth_metadata);

                if config.dump_audio_logs
                    && let Some(path) = &audio_path
                    && let Some(audio_saved_path) = crate::state::history::save_audio(
                        path.as_path(),
                        recording_timestamp,
                        Some("no-speech"),
                        crate::state::history::TranscriptKind::Failed,
                    )
                {
                    write_truth_sidecar_logged(&audio_saved_path, &truth_metadata);
                }

                return Ok(ProcessRecordingOutcome::no_speech(reason));
            }
        };

        info!("Raw transcript captured ({} chars)", raw_text.len());
        let transcript_present = !raw_text.trim().is_empty();

        let language_opt = language.whisper_hint().map(str::to_string);
        let pipeline_outcome = self
            .process_transcript_text_pipeline(types::TranscriptPipelineParams {
                raw_text,
                recording_timestamp,
                assistive,
                hold_mode,
                force_raw,
                force_ai,
                config,
                language_opt,
                raw_save_enabled,
                audio_path,
                cloud_verdict_opt,
                cloud_handle,
                transcript_source: truth_verdict.transcript_source,
                truth_fallback_class: truth_verdict.fallback_class,
                truth_no_speech_reason: truth_verdict.no_speech_reason.clone(),
                truth_speech_pct: truth_verdict.speech_pct,
                truth_avg_logprob: truth_verdict.avg_logprob,
                truth_confidence_flags: truth_verdict.confidence_flags.clone(),
                truth_sparkline: truth_verdict.sparkline.clone(),
                truth_final_pass_disposition: truth_verdict.final_pass_disposition,
                truth_commit_trigger: truth_verdict.commit_trigger.clone(),
                truth_display_status: truth_verdict.display_status.clone(),
                append_mode: false,
                live_stream_session: false,
                user_needs_separator: false,
                assistant_needs_separator: false,
                skip_user_bubble: false,
            })
            .await?;

        Ok(ProcessRecordingOutcome {
            no_speech_reason: None,
            commit_trigger: pipeline_outcome.commit_trigger,
            transcript_present,
        })
    }

    /// Process the recording: stop, transcribe, format, paste
    ///
    /// ## Mode Logic:
    /// - `assistive=true`: ALWAYS AI augmentation (HoldMode::Chat / HoldMode::Selection)
    /// - `force_raw=true`: ALWAYS raw transcript (HoldMode::Raw)
    /// - `force_ai=true`: ALWAYS AI formatting (left double Option)
    /// - Neither: Toggle mode - respects AI_FORMATTING_ENABLED setting
    async fn process_recording(
        &self,
        _session_id: Option<String>,
        assistive: bool,
        hold_mode: HoldMode,
        force_raw: bool,
        force_ai: bool,
    ) -> Result<ProcessRecordingOutcome> {
        #[cfg(test)]
        if PROCESS_RECORDING_TEST_HANG.load(Ordering::SeqCst) {
            info!("process_recording: hanging in test until stuck-stop watchdog cancels it");
            std::future::pending::<()>().await;
        }

        if cfg!(test) {
            info!(
                "process_recording: skipped in tests (assistive={}, hold_mode={:?}, force_raw={}, force_ai={})",
                assistive, hold_mode, force_raw, force_ai
            );
            return Ok(ProcessRecordingOutcome::default());
        }

        // Stop the recorder and get audio file path
        let mut recorder_guard = self.recorder.lock().await;
        let recorder = Self::recorder_from_guard_mut(&mut recorder_guard, "Process-recording")?;
        let (streaming_text, raw_audio_path_opt) =
            recorder.stop().await.context("Failed to stop recorder")?;
        Self::clear_recorder_callbacks(recorder);
        drop(recorder_guard); // Release lock

        self.process_stopped_recording(
            streaming_text,
            raw_audio_path_opt,
            assistive,
            hold_mode,
            force_raw,
            force_ai,
            None,
        )
        .await
    }

    async fn process_transcript_text_pipeline(
        &self,
        p: types::TranscriptPipelineParams,
    ) -> Result<types::TranscriptProcessOutcome> {
        let types::TranscriptPipelineParams {
            raw_text,
            recording_timestamp,
            assistive,
            hold_mode,
            force_raw,
            force_ai,
            config,
            language_opt,
            raw_save_enabled,
            audio_path,
            cloud_verdict_opt,
            cloud_handle,
            transcript_source,
            truth_fallback_class,
            truth_no_speech_reason,
            truth_speech_pct,
            truth_avg_logprob,
            mut truth_confidence_flags,
            truth_sparkline,
            truth_final_pass_disposition,
            truth_commit_trigger,
            truth_display_status,
            append_mode,
            live_stream_session,
            user_needs_separator: _user_needs_separator,
            assistant_needs_separator: _assistant_needs_separator,
            skip_user_bubble,
        } = p;
        let language_opt = language_opt.as_deref();

        // Hands-off (non-assistive) is a single RAW capture; formatting is the explicit
        // post-recording [Format] action, never an auto-format on stop. Force the raw
        // branch for the adjudicated hands-off stop so the session lands fast + raw in
        // the decision overlay — no AI round-trip on the stop path (this branch is the
        // toggle-stuck-watchdog hot path; an extra format call here re-introduces stop
        // latency). (ADR 2026-05-28 Faza 1: differentiation by action, not by mode.)
        let toggle_handsoff = !assistive
            && matches!(
                transcript_source,
                Some(RecordingTranscriptSource::ToggleSessionAdjudicated)
            );
        let force_raw = force_raw || toggle_handsoff;
        let force_ai = force_ai && !toggle_handsoff;

        // ALWAYS-ON: Final post-processing pass (lexicon + cleanup + semantic gate)
        // This ensures ALL output paths receive clean text regardless of mode.
        // Contract: every chunk/transcript passes through StreamPostProcessor before
        // reaching overlay, clipboard, augmentation, or dataset.
        let (clean_text, postprocess_stats) = {
            let mut finalizer = StreamPostProcessor::new();
            let clean_text = finalizer
                .process(&raw_text)
                .unwrap_or_else(|| raw_text.clone());
            let stats = finalizer.stats();
            (clean_text, stats)
        };
        info!(
            "Post-processed transcript ({} chars, delta={}, drops={}/{}, gate_drops={}, lexicon_rewrites={})",
            clean_text.len(),
            raw_text.len() as i64 - clean_text.len() as i64,
            postprocess_stats.dropped_chunks,
            postprocess_stats.input_chunks,
            postprocess_stats.gate_drops,
            postprocess_stats.lexicon_rewrites
        );

        let raw_entry_path = if raw_save_enabled && !live_stream_session {
            let raw_entry = crate::state::history::save_entry_with_timestamp_and_slug(
                &raw_text,
                Some(recording_timestamp),
                crate::state::history::TranscriptKind::Raw,
                Some(&raw_text),
            );
            info!("Raw transcript saved: {}", raw_entry.path.display());
            Some(raw_entry.path)
        } else {
            None
        };

        // Check for repetition loops (Whisper hallucination like "Wielki, Wielki, Wielki...")
        let has_repetition = crate::ai_formatting::has_repetition_loop(&clean_text);
        if has_repetition {
            warn!("Detected repetition loop in transcription - will clean up");
        }

        let chat_active = assistive;

        let mut effective_hold_mode = if assistive && matches!(hold_mode, HoldMode::Raw) {
            // Toggle-assistive path doesn't have a meaningful hold-mode; treat as Chat
            // but allow optional selection context if it was captured.
            HoldMode::Chat
        } else {
            hold_mode
        };
        let ai_key_available = crate::ai_formatting::has_api_key();

        // Determine final text based on mode (NEW architecture):
        //
        // 1. HoldMode::Chat / HoldMode::Selection (assistive=true): ALWAYS AI augmentation
        // 2. Ctrl Hold (force_raw=true): ALWAYS raw transcript (ignores AI toggle)
        // 3. Left double Option (force_ai=true): ALWAYS AI formatting
        // 4. Toggle (neither): respects AI_FORMATTING_ENABLED toggle
        //
        // This allows users to choose mode via hotkey:
        // - Quick dictation? → Ctrl (fast, raw)
        // - Need formatting? → Double Option (respects setting)
        // - AI chat? → Hold + Shift (Chat)
        // - AI on selection? → Hold + Cmd (Selection)
        let mut is_ai_noop = false;
        let (formatted_text, output_kind, mut should_auto_paste) = if assistive {
            info!(
                "Assistive mode ({:?}): augmenting transcript via AI",
                effective_hold_mode
            );

            if chat_active
                && (skip_user_bubble
                    || !should_allow_full_user_bubble_rewrite(
                        skip_user_bubble,
                        append_mode,
                        live_stream_session,
                    ))
            {
                // Preserve controller state: the user-bubble was either finalized
                // (event pipeline) or built from deltas (delta-first path); both mark
                // that the toggle session carries user text. The full-rewrite branch
                // does not set this flag.
                self.toggle_user_has_text.store(true, Ordering::SeqCst);
            }

            let mut ctx = self
                .assistive_context
                .read()
                .await
                .clone()
                .unwrap_or_default();

            // Ensure we have a target app label (best-effort, no selection, no clipboard).
            if ctx.frontmost_app.is_none() {
                ctx.frontmost_app = tokio::task::spawn_blocking(capture_frontmost_app_only)
                    .await
                    .ok()
                    .and_then(|c| c.frontmost_app);
            }

            let missing_selection = matches!(effective_hold_mode, HoldMode::Selection)
                && ctx.selected_text.as_deref().unwrap_or("").trim().is_empty();
            if missing_selection {
                warn!(
                    "Selection mode requested, but no selected text captured; falling back to Chat mode"
                );
                effective_hold_mode = HoldMode::Chat;
            }

            // Split behavior:
            // - Chat: ignore selection.
            // - Selection: if no selection was captured, we already downgraded to Chat mode.
            let assistive_input = build_assistive_input(&clean_text, &ctx);
            if chat_active {
                send_assistive_with_agent_runtime(
                    assistive_input,
                    config.whisper_language,
                    config.ai_assistive_max_tokens,
                )
                .await;
            }
            // Agent runtime path persists full conversation in ThreadStore.
            (
                clean_text.clone(),
                crate::state::history::TranscriptKind::AssistantInterpretation,
                false,
            )
        } else if force_raw {
            // Ctrl Hold: ALWAYS raw transcript (fast dictation mode)
            // Post-processed clean_text is used (lexicon + cleanup already applied)
            if has_repetition {
                info!("Raw mode (Ctrl): applying local repetition cleanup on post-processed text");
                (
                    crate::ai_formatting::remove_simple_repetitions(&clean_text),
                    crate::state::history::TranscriptKind::Raw,
                    true,
                )
            } else {
                info!("Raw mode (Ctrl): using post-processed transcript");
                (
                    clean_text.clone(),
                    crate::state::history::TranscriptKind::Raw,
                    true,
                )
            }
        } else if force_ai {
            // Left double Option: ALWAYS formatting (no augmentation)
            // Auto-paste like hold mode — formatted text goes where the cursor is.
            let should_use_ai = ai_key_available;
            if should_use_ai {
                info!("Formatting mode (Left Option): correcting transcript via AI");

                let lang_str = language_opt.map(String::from);
                let result = crate::ai_formatting::format_text_with_status(
                    &clean_text,
                    lang_str.as_deref(),
                    false,
                    None,
                )
                .await;
                is_ai_noop = result.status == crate::ai_formatting::AiFormatStatus::AiNoop;
                let kind = match result.status {
                    crate::ai_formatting::AiFormatStatus::Applied
                    | crate::ai_formatting::AiFormatStatus::AiNoop => {
                        crate::state::history::TranscriptKind::FormattedTranscript
                    }
                    crate::ai_formatting::AiFormatStatus::Failed => {
                        crate::state::history::TranscriptKind::FormattingFailed
                    }
                    crate::ai_formatting::AiFormatStatus::Skipped => {
                        crate::state::history::TranscriptKind::Raw
                    }
                };
                (result.text, kind, true)
            } else if has_repetition {
                info!("Formatting mode (Left Option): AI unavailable, cleaning repetitions");
                (
                    crate::ai_formatting::remove_simple_repetitions(&clean_text),
                    crate::state::history::TranscriptKind::Raw,
                    true,
                )
            } else {
                info!(
                    "Formatting mode (Left Option): AI unavailable, using post-processed transcript"
                );
                (
                    clean_text.clone(),
                    crate::state::history::TranscriptKind::Raw,
                    true,
                )
            }
        } else {
            // Double Option: respects AI Formatting toggle setting
            let ai_formatting_enabled = config.ai_formatting_enabled;
            let should_use_ai = ai_formatting_enabled && ai_key_available;

            if should_use_ai {
                // Toggle ON: formatting only (no augmentation)
                info!("Formatting mode (Toggle): correcting transcript via AI");

                let lang_str = language_opt.map(String::from);
                let result = crate::ai_formatting::format_text_with_status(
                    &clean_text,
                    lang_str.as_deref(),
                    false,
                    None,
                )
                .await;
                is_ai_noop = result.status == crate::ai_formatting::AiFormatStatus::AiNoop;
                let kind = match result.status {
                    crate::ai_formatting::AiFormatStatus::Applied
                    | crate::ai_formatting::AiFormatStatus::AiNoop => {
                        crate::state::history::TranscriptKind::FormattedTranscript
                    }
                    crate::ai_formatting::AiFormatStatus::Failed => {
                        crate::state::history::TranscriptKind::FormattingFailed
                    }
                    crate::ai_formatting::AiFormatStatus::Skipped => {
                        crate::state::history::TranscriptKind::Raw
                    }
                };
                (result.text, kind, false)
            } else if has_repetition {
                // Toggle OFF with repetition: local cleanup only
                info!("Raw mode (Toggle OFF): applying local repetition cleanup");
                (
                    crate::ai_formatting::remove_simple_repetitions(&clean_text),
                    crate::state::history::TranscriptKind::Raw,
                    true,
                )
            } else {
                // Toggle OFF: using post-processed transcript
                info!("Raw mode (Toggle OFF): using post-processed transcript");
                (
                    clean_text.clone(),
                    crate::state::history::TranscriptKind::Raw,
                    true,
                )
            }
        };

        let mode_label =
            recording_mode_label(assistive, effective_hold_mode, force_raw, force_ai).to_string();
        info!(
            "Final transcript ready ({} chars, mode={})",
            formatted_text.len(),
            mode_label
        );
        let quality_probe =
            ActionQualityProbe::from_transcripts(&raw_text, &formatted_text, &postprocess_stats);
        info!(
            "Action quality guardrail: mode={} assistive={} raw_chars={} final_chars={} diff_raw_final={:.3} correction_ratio={:.3} drop_ratio={:.3} route_independent=true",
            mode_label,
            assistive,
            quality_probe.raw_chars,
            quality_probe.final_chars,
            quality_probe.raw_final_diff_ratio,
            quality_probe.correction_ratio,
            quality_probe.drop_ratio
        );
        let quality_commit_trigger = if !assistive {
            evaluate_quality_commit_trigger(force_raw, &quality_probe, output_kind)
                .map(str::to_string)
        } else {
            None
        };
        let mut commit_trigger = if !assistive {
            truth_commit_trigger.clone().or(quality_commit_trigger)
        } else {
            None
        };

        apply_ai_noop_signal(
            assistive,
            is_ai_noop,
            &mut truth_confidence_flags,
            &mut commit_trigger,
        );

        if let Some(reason) = commit_trigger.as_deref() {
            info!(
                "COMMIT decision: trigger={} mode={} diff_raw_final={:.3} correction_ratio={:.3} drop_ratio={:.3}",
                reason,
                mode_label,
                quality_probe.raw_final_diff_ratio,
                quality_probe.correction_ratio,
                quality_probe.drop_ratio
            );
        } else if !assistive {
            info!("COMMIT decision: not required by quality gate (mode={mode_label})");
        }

        if truth_no_speech_reason.is_some() || commit_trigger.is_some() {
            should_auto_paste = false;
        }
        if live_stream_session {
            should_auto_paste = false;
        }
        // Hands-off (non-assistive) is action-driven: the full transcript lands in the
        // decision overlay and the user picks [Format] / [Copy] / [Agent]. No silent
        // auto-paste — differentiation happens AFTER recording, not during. (ADR
        // 2026-05-28 Faza 1: "Różnicowanie przez akcje, nie przez tryby nagrywania".)
        if matches!(
            transcript_source,
            Some(RecordingTranscriptSource::ToggleSessionAdjudicated)
        ) {
            should_auto_paste = false;
        }

        let final_formatted_text = formatted_text.clone();

        // Surface the authoritative final transcript to external dictation surfaces
        // (the SwiftUI overlay). This is the same `final_formatted_text` that is
        // pasted (auto-delivery) and written to history (tray "Copy"), so the overlay
        // FINAL can replace its raw per-utterance streaming assembly with the clean
        // LocalFinalPass text. Emitted here — inside the awaited stop pipeline, before
        // the Idle StateChange — so it reaches the listener ahead of the stop/finalise
        // events that drive the overlay's finalize.
        if !assistive && !final_formatted_text.trim().is_empty() {
            let _ = self.event_broadcast.send(IpcEvent {
                timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                payload: IpcEventPayload::FinalTranscript {
                    text: final_formatted_text.clone(),
                },
            });
        }

        let final_status = compose_final_status(&truth_display_status, output_kind);
        let truth_metadata = RecordingTruthMetadata {
            source: transcript_source,
            engine: truth_engine_label(transcript_source),
            mode: Some(mode_label.clone()),
            fallback_class: truth_fallback_class,
            fallback_used: truth_fallback_class.is_some()
                || matches!(
                    transcript_source,
                    Some(RecordingTranscriptSource::StreamingFallback)
                ),
            vad_speech_pct: truth_speech_pct,
            no_speech_reason: truth_no_speech_reason.clone(),
            avg_logprob: truth_avg_logprob,
            confidence_flags: truth_confidence_flags.clone(),
            sparkline: truth_sparkline.clone(),
            final_pass_disposition: truth_final_pass_disposition,
            commit_trigger: commit_trigger.clone(),
            display_status: Some(final_status.clone()),
        };

        if let Some(path) = raw_entry_path.as_deref() {
            write_truth_sidecar_logged(path, &truth_metadata);
        }

        // The action-contract rewrite was retired with the AppKit delivery path;
        // keep only the live-stream skip breadcrumb (formatting is bypassed there).
        let action_contract_applies =
            should_apply_transcription_action_contract(assistive, live_stream_session)
                && config.transcription_overlay_enabled;
        if !(assistive || action_contract_applies) {
            debug!(
                "Skipping transcription action contract rewrite during live stream (mode={mode_label})"
            );
        }

        // Quick Notes: optionally save to daily note file (dictation-only).
        if !assistive && config.quick_notes_enabled {
            match crate::state::notes::append_quick_note(&formatted_text, recording_timestamp) {
                Ok(path) => {
                    info!("Quick note saved: {}", path.display());
                    #[cfg(target_os = "macos")]
                    crate::os::notifications::notify(
                        "Codescribe",
                        &format!(
                            "Saved note: {}",
                            path.file_name().and_then(|s| s.to_str()).unwrap_or("note")
                        ),
                    );
                }
                Err(e) => {
                    warn!("Quick note save failed: {}", e);
                }
            }

            // Optional: make Quick Notes "save-only".
            if config.quick_notes_save_only {
                should_auto_paste = false;
            }
        }

        // Save audio to transcriptions folder if enabled (pair with RAW for reports)
        if config.dump_audio_logs
            && let Some(path) = &audio_path
            && let Some(audio_saved_path) = crate::state::history::save_audio(
                path.as_path(),
                recording_timestamp,
                Some(&raw_text),
                crate::state::history::TranscriptKind::Raw,
            )
        {
            write_truth_sidecar_logged(&audio_saved_path, &truth_metadata);
        }

        // Overlay disabled = no decision surface. The action-driven gates above
        // (commit_trigger / toggle-adjudicated / live-stream) hand the transcript
        // to the overlay; with no overlay it would just vanish. Deliver headless by
        // pasting directly at the cursor — unless there is nothing to paste (no
        // speech) or Notes Mode chose save-only.
        let overlay_disabled = !config.transcription_overlay_enabled;
        let has_final_text = !final_formatted_text.trim().is_empty();
        let notes_save_only = config.quick_notes_enabled && config.quick_notes_save_only;
        if overlay_disabled
            && !assistive
            && truth_no_speech_reason.is_none()
            && has_final_text
            && !notes_save_only
        {
            should_auto_paste = true;
        }

        if cfg!(test) {
            info!("Skipping paste in tests (mode={})", mode_label);
        } else if should_auto_paste {
            let paste_text =
                maybe_wrap_transcript_for_delivery(&final_formatted_text, &config, &mode_label);
            // Paste the text into the active application
            clipboard::paste_text(&paste_text).context("Failed to paste text")?;
            info!("Text pasted successfully");
        } else {
            info!("Auto-paste skipped (mode={})", mode_label);
        }

        // Save final transcript (skip duplicate when RAW already stored and unchanged)
        let needs_final_save = !assistive
            && !live_stream_session
            && (!raw_save_enabled
                || output_kind != crate::state::history::TranscriptKind::Raw
                || final_formatted_text.trim() != raw_text.trim());
        if needs_final_save {
            let entry = crate::state::history::save_entry_with_timestamp_and_slug(
                &final_formatted_text,
                Some(recording_timestamp),
                output_kind,
                Some(&raw_text),
            );
            info!("Transcript saved: {}", entry.path.display());
            write_truth_sidecar_logged(&entry.path, &truth_metadata);
        } else if assistive {
            info!(
                "Assistive flow: skipping legacy final transcript save (ThreadStore is source of truth)"
            );
        } else {
            info!("Final transcript matches RAW; skipping duplicate save");
        }

        if let Some(cloud_verdict) = cloud_verdict_opt {
            let entry = crate::state::history::save_entry_with_timestamp_and_slug(
                &cloud_verdict.text,
                Some(recording_timestamp),
                crate::state::history::TranscriptKind::Cloud,
                Some(&raw_text),
            );
            info!("Cloud transcript saved: {}", entry.path.display());
            write_truth_sidecar_logged(
                &entry.path,
                &RecordingTruthMetadata {
                    source: Some(RecordingTranscriptSource::CloudPrimary),
                    engine: truth_engine_label(Some(RecordingTranscriptSource::CloudPrimary)),
                    mode: Some(mode_label.clone()),
                    fallback_class: None,
                    fallback_used: false,
                    vad_speech_pct: None,
                    no_speech_reason: None,
                    avg_logprob: None,
                    confidence_flags: cloud_verdict.confidence_flags.clone(),
                    sparkline: None,
                    final_pass_disposition: None,
                    commit_trigger: None,
                    display_status: Some(
                        RecordingTranscriptSource::CloudPrimary.label().to_string(),
                    ),
                },
            );
        } else if let Some(handle) = cloud_handle {
            let slug_hint = raw_text.clone();
            let timestamp = recording_timestamp;
            let mode_label = mode_label.clone();
            tokio::spawn(async move {
                match handle.await {
                    Ok(Ok(cloud_verdict)) => {
                        let entry = crate::state::history::save_entry_with_timestamp_and_slug(
                            &cloud_verdict.text,
                            Some(timestamp),
                            crate::state::history::TranscriptKind::Cloud,
                            Some(&slug_hint),
                        );
                        info!("Cloud transcript saved: {}", entry.path.display());
                        write_truth_sidecar_logged(
                            &entry.path,
                            &RecordingTruthMetadata {
                                source: Some(RecordingTranscriptSource::CloudPrimary),
                                engine: truth_engine_label(Some(
                                    RecordingTranscriptSource::CloudPrimary,
                                )),
                                mode: Some(mode_label),
                                fallback_class: None,
                                fallback_used: false,
                                vad_speech_pct: None,
                                no_speech_reason: None,
                                avg_logprob: None,
                                confidence_flags: cloud_verdict.confidence_flags.clone(),
                                sparkline: None,
                                final_pass_disposition: None,
                                commit_trigger: None,
                                display_status: Some(
                                    RecordingTranscriptSource::CloudPrimary.label().to_string(),
                                ),
                            },
                        );
                    }
                    Ok(Err(e)) => error!("Cloud transcription failed: {}", e),
                    Err(e) => error!("Cloud transcription task failed: {}", e),
                }
            });
        }

        Ok(types::TranscriptProcessOutcome { commit_trigger })
    }

    /// Force reset to IDLE state without stopping recorder.
    ///
    /// This is the nuclear option - use only when state is corrupted
    /// or during crash recovery.
    pub async fn reset(&self) {
        warn!("Forcing state reset to IDLE (recovery mode)");
        self.reset_state().await;
    }

    /// Internal helper to reset all state variables
    async fn reset_state(&self) {
        self.reset_session_fields().await;

        info!("State reset to IDLE complete");
    }

    /// Check if controller is in a recording state
    pub async fn is_recording(&self) -> bool {
        matches!(
            self.current_state().await,
            State::RecHold | State::RecToggle
        )
    }

    /// Check if controller is busy processing
    pub async fn is_busy(&self) -> bool {
        self.current_state().await == State::Busy
    }
}

impl Default for RecordingController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
