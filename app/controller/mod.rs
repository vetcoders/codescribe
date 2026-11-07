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

/// Admission readiness: the precondition of beginning a product recording.
pub mod admission;
/// Fn+digit agent channels. Not a take and not a `State` variant.
mod agent_channel;
pub use agent_channel::{
    AgentArchiveRequest, ChannelHudState, ChannelOpenRefusal, FOUNDER_AUDIENCE, FOUNDER_CHANNEL,
    founder_bus_path_at,
};
/// Thirty seconds without voice end a hands-free assistive take.
mod assistive_silence;
/// Per-session assistive context bag (selection, app, images).
mod context_bucket;
/// One destination throne: intent → Agent / Orient / paste. Focus is not king.
mod delivery_route;
/// Session telemetry, image attach helpers, assistive send wiring.
mod helpers;
/// Hold/toggle timing, agent-send vetoes, stop adjudication policy.
mod hotkey_policy;
/// Best-effort RMS frame feed for the loopback Voice Lab relay.
mod lab_feed;
pub(crate) mod live_archive;
pub use live_archive::ArchiveRefusal;
#[cfg(test)]
mod channel_tray_tests;
#[cfg(test)]
mod formatter_manual_max_acceptance;
#[cfg(test)]
mod formatter_selection_acceptance;
/// Production-owned, content-private PCM replay of the overlay engine cone.
pub mod production_replay;
/// Public serving-status surface for tray/UI consumers.
pub mod serving_status;
#[cfg(test)]
mod silver_start_refusal_tests;
/// Measured speech phases shared by agent channels and the assistive take.
mod speech_clock;
mod transcript_delivery;
/// Controller state, hotkey types, and recording truth metadata.
mod types;

pub use delivery_route::{
    DeliveryIntent, DeliveryRoute, OverlayPasteDelivery, OverlayPasteResult, PasteTarget,
    TERMINAL_APPS, delivery_intent_from_session, format_delivery_route_line, is_terminal_app,
    looks_executable, overlay_insert_facts, paste_hold_notice, resolve_delivery_route,
};
pub(crate) use delivery_route::{
    TranscriptProjectionAvailability, resolve_transcript_projection_availability,
};
pub use helpers::{
    is_assistive_session, publish_recording_indicator, set_assistive_session,
    set_assistive_target_thread, settle_failed_start_chrome,
};
pub use types::{HotkeyAction, HotkeyInput, HotkeyType, State};

use crate::presentation::emitter::VisibleCanvasSnapshot;
use crate::presentation::status_projection::PresentationStatusProjection;
use crate::presentation::transcript_bus::{TranscriptDelivery, TranscriptSessionEndReason};
use crate::presentation::{
    PresentationEmitter, TerminalFormatterRequest, TranscriptBus, TranscriptMode,
    TranscriptSession, UserRevisionCommit, UserRevisionIntent,
};
use anyhow::{Context, Result};
use codescribe_core::llm::ai_formatting::format_text_with_status_for_policy;
use codescribe_core::pipeline::acoustic_ledger::DocumentRevisionProvenance;
use codescribe_core::pipeline::contracts::EngineEvent;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, OwnedMutexGuard, RwLock, broadcast, mpsc, watch};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::audio::streaming_recorder::{
    CaptureStopFailure, CaptureTurnIntent, ClosedTake, StreamingRecorder, TerminalSealRefused,
};
use crate::config::models::ModelManager;
use crate::config::{Config, FormattingPolicy, PasteMode, RuntimeSettingsSnapshot, UserSettings};
use crate::os::clipboard;
use crate::os::hold_badge::BadgeMode;
use crate::os::hotkeys::{self, HoldMode};
use crate::os::selection::{
    AssistiveContext, CapturedAssistiveContext, capture_assistive_context,
    capture_assistive_context_with_image_with_prior_frontmost,
    capture_frontmost_app_only_with_prior_frontmost, is_codescribe_app,
};
use crate::os::shortcut_registry;
use context_bucket::ContextBucket;

// Moshi conversation engine and audio output
use codescribe_core::conversation::{ConversationEngine, MoshiConfig};
use codescribe_core::ipc::{EngineEventWire, IpcEvent, IpcEventPayload};
use codescribe_core::tts::AudioPlayer;

use delivery_route::DeliveryFacts;
use helpers::send_assistive_with_agent_runtime_lane;
use hotkey_policy::{
    STOP_TIMEOUT, effective_hold_start_delay_ms, hold_start_redirects_to_composer,
    should_apply_incoming_mode_flags, should_block_hotkey_during_agent_send,
    should_use_toggle_adjudicated_stop, toggle_final_pass_enabled,
};
use transcript_delivery::TranscriptDeliveryTagger;

/// Live overlay: ms of audio held before the first interim emit.
const LIVE_PROFILE_BUFFER_DELAY_MS: u64 = 280;
/// Live overlay typing animation speed in characters per second.
const LIVE_PROFILE_TYPING_CPS: f32 = 90.0;
/// Cap words emitted per live interim chunk (smooth typing feel).
const LIVE_PROFILE_EMIT_WORDS_MAX: u64 = 2;
/// Seconds between interim emissions when the overlay is visible.
const LIVE_PROFILE_INTERIM_SEC: f32 = 1.2;
/// Longer interim interval when no overlay is watching partials.
const NO_OVERLAY_PROFILE_INTERIM_SEC: f32 = 8.0;
/// At most one level sample may wait behind the controller worker. The capture
/// thread never constructs IPC events or timestamps and never accumulates a
/// backlog when the bridge/UI is slower than CoreAudio.
const AUDIO_LEVEL_QUEUE_CAPACITY: usize = 1;

fn formatter_revision_level(
    requested: Option<FormattingPolicy>,
    settings: &RuntimeSettingsSnapshot,
) -> Result<(FormattingPolicy, &'static str)> {
    let configured = settings.formatting_policy();
    let selected = requested.unwrap_or(configured);
    Ok((
        selected,
        if requested.is_some() {
            "request"
        } else {
            "settings"
        },
    ))
}

/// Seal one capture take's formatting at admission. A take formats only with
/// the automatic policy (Off, Correction or Smart): Max is the Agent and runs
/// only for an explicit overlay or history Format action, so no capture can
/// schedule a Max readiness or execution request, live or at Stop. The
/// controller generation and the persisted selection stay untouched; only this
/// take's snapshot is derived, from the same captured inputs.
fn automatic_take_settings(settings: Arc<RuntimeSettingsSnapshot>) -> Arc<RuntimeSettingsSnapshot> {
    let automatic = settings.automatic_formatting_policy();
    if settings.formatting_policy() == automatic {
        settings
    } else {
        Arc::new(settings.with_formatting_level(automatic))
    }
}

/// What the production formatter did with a reopened archive transcript.
/// Mirrors the reducer's formatter refusals without a reducer revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchivedFormatOutcome {
    Applied(String),
    Failed,
    Unavailable,
    Unchanged,
}

impl ArchivedFormatOutcome {
    /// Only the formatter's status decides. An applied result equal to the
    /// source is still one accepted attempt and becomes its own version; a
    /// provider echo on failure reports `Failed`, never `Applied`.
    fn from_result(
        _source: &str,
        result: codescribe_core::llm::ai_formatting::AiFormatResult,
    ) -> Self {
        use codescribe_core::llm::ai_formatting::AiFormatStatus;
        match result.status {
            AiFormatStatus::Applied if !result.text.trim().is_empty() => Self::Applied(result.text),
            AiFormatStatus::Applied | AiFormatStatus::AiNoop => Self::Unchanged,
            AiFormatStatus::Failed => Self::Failed,
            AiFormatStatus::Skipped => Self::Unavailable,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Applied(_) => "applied",
            Self::Failed => "failed",
            Self::Unavailable => "unavailable",
            Self::Unchanged => "unchanged",
        }
    }
}

/// The formatter's verdict for one archived transcript and, when it applied,
/// the revision the history owner accepted for exactly that archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivedFormatReceipt {
    pub outcome: ArchivedFormatOutcome,
    pub revision: Option<codescribe_core::state::history::ArchiveRevision>,
}

#[cfg(test)]
mod archived_format_outcome_tests {
    use super::ArchivedFormatOutcome;
    use codescribe_core::llm::ai_formatting::{AiFormatResult, AiFormatStatus};

    fn result(text: &str, status: AiFormatStatus) -> AiFormatResult {
        AiFormatResult {
            text: text.to_string(),
            reasoning_text: None,
            status,
        }
    }

    /// A failed provider hands back the cleaned input; that echo must never
    /// read as an applied archive format.
    #[test]
    fn every_applied_attempt_reaches_the_archive_canvas() {
        let source = "ala ma kota";
        assert_eq!(
            ArchivedFormatOutcome::from_result(
                source,
                result("Ala ma kota.", AiFormatStatus::Applied)
            ),
            ArchivedFormatOutcome::Applied("Ala ma kota.".to_string())
        );
        assert_eq!(
            ArchivedFormatOutcome::from_result(source, result(source, AiFormatStatus::Applied)),
            ArchivedFormatOutcome::Applied(source.to_string())
        );
        assert_eq!(
            ArchivedFormatOutcome::from_result(source, result(source, AiFormatStatus::AiNoop)),
            ArchivedFormatOutcome::Unchanged
        );
        assert_eq!(
            ArchivedFormatOutcome::from_result(source, result(source, AiFormatStatus::Failed)),
            ArchivedFormatOutcome::Failed
        );
        assert_eq!(
            ArchivedFormatOutcome::from_result(source, result(source, AiFormatStatus::Skipped)),
            ArchivedFormatOutcome::Unavailable
        );
    }
}

fn log_formatter_revision(
    receipt: &UserRevisionCommit,
    level: FormattingPolicy,
    source: &str,
    runtime_settings: &RuntimeSettingsSnapshot,
) {
    let prompt = runtime_settings
        .ai_execution()
        .formatter()
        .formatting_prompt();
    info!(
        session_id = %receipt.session_id,
        source_revision = receipt.source_revision,
        revision = receipt.revision,
        provenance_receipt = %receipt.provenance_receipt,
        format_level = level.as_str(),
        source,
        prompt_source = prompt.map(|p| p.source().as_str()).unwrap_or("none"),
        prompt_sha256 = prompt.map(|p| p.composed_sha256()).unwrap_or("none"),
        settings_sha256 = runtime_settings.digest().as_str(),
        "formatter revision committed"
    );
}

/// Publish the live-transcription tuning for the session that is about to start
/// and report whether the overlay is enabled.
///
/// The knobs cross into the core pipeline as process env vars, which is why
/// this runs at every session start rather than once at boot: user settings can
/// change between recordings. A session with no overlay to feed uses a much
/// longer interim window — nobody is watching the partials, so paying for
/// frequent interim emissions would be waste.
fn apply_runtime_transcription_profile(
    config: &Config,
    settings: &UserSettings,
    assistive: bool,
) -> bool {
    let overlay_enabled = config.transcription_overlay_enabled;

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

/// Holds a shared flag `true` for a scope and clears it on drop — including on
/// an early `return` out of a start path, which is exactly where a hand-written
/// reset gets forgotten.
struct AtomicFlagGuard {
    flag: Arc<AtomicBool>,
}

impl AtomicFlagGuard {
    /// Raise the flag; it falls when the returned guard is dropped.
    fn new(flag: Arc<AtomicBool>) -> Self {
        flag.store(true, Ordering::SeqCst);
        Self { flag }
    }
}

impl Drop for AtomicFlagGuard {
    /// Lower the shared AtomicFlagGuard flag when the scope ends.
    fn drop(&mut self) {
        self.flag.store(false, Ordering::SeqCst);
    }
}

/// Why a delayed hold start unwound before it became an active recording.
/// Each variant is the truthful cause the terminal Bus line reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HoldStartAbort {
    /// Acoustic admission refused before the microphone opened.
    AdmissionRefused,
    /// No recorder instance, or it could not reach a clean pre-start state.
    RecorderUnavailable,
    /// `start_event_session` failed, including after the one stale-lock retry.
    RecorderStartFailed,
    /// A key-up or reschedule bumped the hold generation before `RecHold`.
    Superseded,
}

impl HoldStartAbort {
    /// The typed reason written on the Bus terminal line for this abort.
    fn bus_reason(self) -> TranscriptSessionEndReason {
        match self {
            Self::Superseded => TranscriptSessionEndReason::StartSuperseded,
            Self::AdmissionRefused | Self::RecorderUnavailable | Self::RecorderStartFailed => {
                TranscriptSessionEndReason::StartFailed
            }
        }
    }
}

/// The controller-owned session slots a delayed hold start fills before the
/// take is active. The spawned task has no `&self`; every early exit after
/// its start guard unwinds through exactly these handles.
struct HoldStartSession {
    session_id: Arc<RwLock<Option<String>>>,
    active_transcript_bus: Arc<RwLock<Option<Arc<TranscriptBus>>>>,
    active_presentation: Arc<RwLock<Option<Arc<PresentationEmitter>>>>,
    active_stop_receipt: Arc<RwLock<Arc<std::sync::Mutex<LateFinalReceipt>>>>,
    assistive_context: Arc<RwLock<Option<AssistiveContext>>>,
    pre_overlay_frontmost_app: Arc<RwLock<Option<String>>>,
    event_broadcast: broadcast::Sender<IpcEvent>,
    /// Ctrl-hold literal contract: the emitter must not shape (Light+) the
    /// terminal document for this take. Read at sink build time, not at stop.
    force_raw_mode: Arc<RwLock<bool>>,
    delivery_tagger: Arc<TranscriptDeliveryTagger>,
    composer_delivery_payload: Arc<RwLock<Option<(String, String)>>>,
}

/// The recorder-facing fanout plus the retained reducer authority behind it.
/// Naming this boundary keeps structural inspection exact while both handles
/// continue to refer to one `PresentationEmitter` instance.
struct RecordingEventPipeline {
    stop_receipt: Arc<std::sync::Mutex<LateFinalReceipt>>,
    event_sink: Arc<dyn codescribe_core::pipeline::contracts::EventSink>,
    presentation: Arc<PresentationEmitter>,
}

struct RecordingEventSinkOptions {
    preview_deltas_enabled: bool,
    sentence_pause_sec: f32,
    /// Per-take track switches from the frozen runtime snapshot.
    seal_ledger_enabled: bool,
    apple_live_enabled: bool,
    /// Speech edges of this take, for the watch that reads them (assistive).
    speech_observer: Option<crate::presentation::emitter::SpeechActivityObserver>,
}

/// Safe filename fragment for a controller session id. Rejects path
/// traversal; UUIDs and the bus-demux lease alphabet pass.
fn valid_session_audio_id(session_id: &str) -> Option<&str> {
    let ok = (8..=80).contains(&session_id.len())
        && session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    ok.then_some(session_id)
}

/// Toggle stop rewrites the live slot to `{uuid}:stopping` so a second start
/// cannot collide. That suffix is not a wav name — Lab lanes 2/3 (candle HQ
/// and cloud :8444) judge the file, not a second mic. Strip it before retain.
fn retainable_session_id(session_id: Option<&str>) -> Option<&str> {
    let raw = session_id?;
    valid_session_audio_id(raw.strip_suffix(":stopping").unwrap_or(raw))
}

/// Canonical wav for one Bus take: `~/.codescribe/sessions/<session_id>.wav`.
/// Bus-demux assigns this path to attached followers. `last_session.wav` is
/// only a latest-take alias for overlay / `codescribe transcribe last`.
fn session_audio_path(root: &std::path::Path, session_id: &str) -> Option<std::path::PathBuf> {
    valid_session_audio_id(session_id).map(|id| root.join("sessions").join(format!("{id}.wav")))
}

/// Freeze storage policy and acquire its read/processing fence before capture.
async fn begin_audio_capture(
    id: &str,
    policy: codescribe_core::config::AudioRetention,
) -> Result<()> {
    let root = Config::config_dir();
    let id = id.to_owned();
    tokio::task::spawn_blocking(move || {
        codescribe_core::state::history::audio_retention::begin_capture(&root, &id, policy)
    })
    .await
    .context("audio capture lease worker failed")?
}

fn finish_audio_capture(id: Option<&str>) {
    if let Some(id) = retainable_session_id(id) {
        codescribe_core::state::history::audio_retention::finish_capture(&Config::config_dir(), id);
    }
}

/// A dictation document and its initiating capture have distinct namespaces.
/// Failed admission may have acquired the capture lease before a Bus existed.
/// A surviving shared source keeps that lease until its last participant stops.
fn finish_document_audio_capture(document_id: Option<&str>, live_capture: Option<&str>) {
    let Some(document_id) = retainable_session_id(document_id) else {
        return;
    };
    if Some(document_id) != live_capture {
        finish_audio_capture(Some(document_id));
    }
    let capture = StreamingRecorder::dictation_capture_session_id(document_id);
    if Some(capture.as_str()) != live_capture {
        finish_audio_capture(Some(&capture));
    }
}

/// Observe the original take ledger after its final refinement completes.
///
/// The handle belongs to the recorder that just stopped. A missing handle, a
/// zero epoch, or a session that disagrees with the retention id is reported
/// as unavailable evidence. Nothing here mints a receipt.
fn observe_take_ledger(
    identity: (Option<&str>, u64),
    handle: Option<
        Arc<std::sync::Mutex<codescribe_core::pipeline::acoustic_ledger::AcousticLedger>>,
    >,
    retention_id: Option<&str>,
) -> codescribe_core::pipeline::take_truth::TakeTruth {
    use codescribe_core::pipeline::take_truth::TakeTruth;
    let (recorder_session, raw_epoch) = identity;
    let epoch = (raw_epoch != 0).then_some(raw_epoch);
    let session_id = match (recorder_session, retention_id) {
        (Some(session), None) => Some(session.to_string()),
        (Some(session), Some(_)) if retainable_session_id(retention_id) == Some(session) => {
            Some(session.to_string())
        }
        (Some(session), Some(retention_id)) => {
            warn!(
                recorder_session = session,
                retention_id,
                "take observer session disagreed with retention id; evidence unavailable"
            );
            return TakeTruth::unavailable_evidence(None, None);
        }
        (None, _) => None,
    };
    let Some(handle) = handle else {
        warn!(
            session_id = session_id.as_deref().unwrap_or(""),
            epoch = epoch.unwrap_or(0),
            "take observer ledger handle missing; evidence unavailable"
        );
        return TakeTruth::unavailable_evidence(session_id, epoch);
    };
    let Some(session_id) = session_id else {
        return TakeTruth::unavailable_evidence(None, epoch);
    };
    let Some(epoch) = epoch else {
        return TakeTruth::unavailable_evidence(Some(session_id), None);
    };
    let ledger = handle.lock().unwrap_or_else(|error| error.into_inner());
    TakeTruth::observe_ledger(&ledger, &session_id, epoch)
}

/// Keep full audio independently of transcript outcome on a blocking worker.
/// Returns the archived transcript when the take persisted spoken text: the
/// file whose revision chain becomes the durable history of this take.
async fn retain_session_audio(
    session_id: Option<&str>,
    path: &std::path::Path,
    transcript: codescribe_core::state::SessionTranscriptArchive<'_>,
    observer: &codescribe_core::pipeline::take_truth::TakeTruth,
) -> Option<std::path::PathBuf> {
    let lease = retainable_session_id(session_id).and_then(|id| {
        codescribe_core::state::history::audio_retention::capture(&Config::config_dir(), id)
    });
    retain_session_audio_with_lease(session_id, path, transcript, lease, observer).await
}

async fn retain_session_audio_with_lease(
    session_id: Option<&str>,
    path: &std::path::Path,
    transcript: codescribe_core::state::SessionTranscriptArchive<'_>,
    lease: Option<Arc<codescribe_core::state::history::audio_retention::CaptureLease>>,
    observer: &codescribe_core::pipeline::take_truth::TakeTruth,
) -> Option<std::path::PathBuf> {
    use codescribe_core::state::SessionTranscriptArchive;
    let session_id = session_id.map(str::to_owned);
    let path = path.to_path_buf();
    let observer = observer.clone();
    let (text, unavailable) = match transcript {
        SessionTranscriptArchive::Committed(text) => (Some(text.to_string()), false),
        SessionTranscriptArchive::NoSpeech => (None, false),
        SessionTranscriptArchive::Unavailable(reason) => (Some(reason.to_string()), true),
    };
    let result = tokio::task::spawn_blocking(move || {
        let transcript = if unavailable {
            SessionTranscriptArchive::Unavailable(text.as_deref().unwrap_or_default())
        } else if let Some(text) = text.as_deref() {
            SessionTranscriptArchive::Committed(text)
        } else {
            SessionTranscriptArchive::NoSpeech
        };
        let mut archived_transcript = None;
        let retained = retain_owned_session_audio_linked(
            session_id.as_deref(),
            &path,
            transcript,
            &observer,
            lease.as_deref(),
            &mut archived_transcript,
        );
        (retained, archived_transcript)
    })
    .await;
    match result {
        Ok((Ok(()), archived_transcript)) => archived_transcript,
        Ok((Err(error), archived_transcript)) => {
            tracing::error!("{error:#}");
            archived_transcript
        }
        Err(error) => {
            tracing::error!(%error, "audio retention worker failed; source preserved");
            None
        }
    }
}

fn retain_owned_session_audio(
    session_id: Option<&str>,
    path: &std::path::Path,
    transcript: codescribe_core::state::SessionTranscriptArchive<'_>,
    observer: &codescribe_core::pipeline::take_truth::TakeTruth,
    lease: Option<&codescribe_core::state::history::audio_retention::CaptureLease>,
) -> Result<()> {
    retain_owned_session_audio_linked(session_id, path, transcript, observer, lease, &mut None)
}

/// [`retain_owned_session_audio`], also naming the archived transcript when
/// the daily archive persisted spoken text beside the audio.
fn retain_owned_session_audio_linked(
    session_id: Option<&str>,
    path: &std::path::Path,
    transcript: codescribe_core::state::SessionTranscriptArchive<'_>,
    observer: &codescribe_core::pipeline::take_truth::TakeTruth,
    lease: Option<&codescribe_core::state::history::audio_retention::CaptureLease>,
    archived_transcript: &mut Option<std::path::PathBuf>,
) -> Result<()> {
    let spoken = matches!(
        transcript,
        codescribe_core::state::SessionTranscriptArchive::Committed(text) if !text.trim().is_empty()
    );
    let root = Config::config_dir();
    let mut daily_audio = None;
    let mut daily_card = None;
    let mut stable_card = None;
    let result = retain_session_audio_at(
        session_id,
        path,
        transcript,
        observer,
        &root,
        &mut stable_card,
        |source, text| match codescribe_core::state::archive_session_take_from_file_with_truth(
            source,
            text,
            Some(observer),
        ) {
            Some(take) => {
                daily_card = take.observer;
                daily_audio = Some(take.audio.clone());
                Some(take.audio)
            }
            None => None,
        },
    );
    if spoken {
        *archived_transcript = daily_audio
            .as_ref()
            .map(|audio| audio.with_extension("txt"))
            .filter(|transcript| transcript.is_file());
    }
    if let Some(lease) = lease {
        if result.is_err() {
            lease.protect_retry();
        }
        let mut owned = vec![path.to_path_buf()];
        if let Some(id) = retainable_session_id(session_id)
            && let Some(session) = session_audio_path(&root, id)
        {
            owned.push(session);
        }
        if let Some(daily) = daily_audio {
            owned.push(daily);
        }
        if let Some(card) = stable_card {
            owned.push(card);
        }
        if let Some(card) = daily_card {
            owned.push(card);
        }
        if let Err(error) = lease.record(&owned, false) {
            lease.protect_retry();
            return Err(error.context("audio ownership receipt refused; source preserved"));
        }
    }
    result
}

/// Same archive/copy path for ordinary and failed takes. Attempt every owned
/// destination even if one fails, report failure, and never remove the source.
/// Directory and daily-bag operation are injectable without changing HOME or
/// invoking the history transcoder in controller tests.
fn retain_session_audio_at(
    session_id: Option<&str>,
    path: &std::path::Path,
    transcript: codescribe_core::state::SessionTranscriptArchive<'_>,
    observer: &codescribe_core::pipeline::take_truth::TakeTruth,
    root: &std::path::Path,
    stable_card: &mut Option<std::path::PathBuf>,
    archive: impl FnOnce(
        &mut std::fs::File,
        codescribe_core::state::SessionTranscriptArchive<'_>,
    ) -> Option<std::path::PathBuf>,
) -> Result<()> {
    let has_speech = matches!(transcript, codescribe_core::state::SessionTranscriptArchive::Committed(text) if !text.trim().is_empty());
    let id = retainable_session_id(session_id)
        .ok_or_else(|| anyhow::anyhow!("audio retention refused: missing or unsafe session id"))?;
    // Freeze the source object before invoking the daily history owner.
    // O_NONBLOCK prevents a FIFO from hanging before fstat.
    let (source_parent, source_name) = open_retention_parent(path)
        .with_context(|| format!("audio retention source {} refused", path.display()))?;
    let mut source = open_retention_entry(
        &source_parent,
        &source_name,
        libc::O_RDONLY | libc::O_NONBLOCK,
    )?;
    anyhow::ensure!(
        source.metadata()?.is_file(),
        "audio retention source is not a regular file"
    );

    // Pin destinations before the callback. Session identity and the latest
    // spoken take remain independent when a quiet take follows speech.
    let root_directory = (|| -> Result<std::fs::File> {
        let (parent, name) = open_retention_parent(root)?;
        retention_subdirectory(&parent, &name)
    })();
    let sessions_directory = root_directory
        .as_ref()
        .map_err(|error| anyhow::anyhow!("{error:#}"))
        .and_then(|directory| retention_subdirectory(directory, c"sessions"));
    let mut failures = Vec::new();
    if archive(&mut source, transcript).is_none() {
        failures.push("daily audio archive failed".to_string());
    }
    let session_path = session_audio_path(root, id)
        .ok_or_else(|| anyhow::anyhow!("audio retention refused: unsafe session id"))?;
    let session_name = std::ffi::CString::new(format!("{id}.wav"))?;
    let linked = sessions_directory
        .as_ref()
        .map_err(|error| anyhow::anyhow!("{error:#}"))
        .and_then(|directory| {
            publish_retained_link(
                &source_parent,
                &source_name,
                &mut source,
                directory,
                session_name.as_c_str(),
            )
        });
    let session_retained = linked.is_ok();
    match linked {
        Ok(()) => info!(
            "session audio retained as link for {}",
            session_path.display()
        ),
        Err(error) => failures.push(format!("{}: {error:#}", session_path.display())),
    }
    let card_published = if session_retained {
        match sessions_directory
            .as_ref()
            .map_err(|error| anyhow::anyhow!("{error:#}"))
            .and_then(|directory| {
                codescribe_core::pipeline::take_truth::write_truth_sidecar_at(
                    directory,
                    &session_path,
                    observer,
                )
            }) {
            Ok(card) => {
                info!("session observer retained as {}", card.display());
                *stable_card = Some(card);
                true
            }
            Err(error) => {
                warn!(
                    "session observer was not written for {} (audio preserved): {error:#}",
                    session_path.display()
                );
                false
            }
        }
    } else {
        warn!(
            "session observer was not written for {} because session audio was not retained; source preserved",
            session_path.display()
        );
        false
    };
    if has_speech && session_retained {
        let alias = root.join("last_session.wav");
        match root_directory
            .as_ref()
            .map_err(|error| anyhow::anyhow!("{error:#}"))
            .and_then(|directory| publish_latest_capture_aliases(directory, id, card_published))
        {
            Ok(()) => info!("last spoken take alias updated for {}", alias.display()),
            Err(error) => failures.push(format!("{}: {error:#}", alias.display())),
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "audio retention failed (source {} preserved): {}",
            path.display(),
            failures.join("; ")
        ))
    }
}

fn publish_session_alias(directory: &std::fs::File, id: &str) -> Result<()> {
    use std::os::fd::AsRawFd;
    let target = std::ffi::CString::new(format!("sessions/{id}.wav"))?;
    let temporary = std::ffi::CString::new(format!(".retain-{}.tmp", Uuid::new_v4()))?;
    // SAFETY: all names and the held directory descriptor survive both calls.
    if unsafe { libc::symlinkat(target.as_ptr(), directory.as_raw_fd(), temporary.as_ptr()) } < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    if unsafe {
        libc::renameat(
            directory.as_raw_fd(),
            temporary.as_ptr(),
            directory.as_raw_fd(),
            c"last_session.wav".as_ptr(),
        )
    } < 0
    {
        let error = std::io::Error::last_os_error();
        unsafe { libc::unlinkat(directory.as_raw_fd(), temporary.as_ptr(), 0) };
        return Err(error.into());
    }
    Ok(())
}

enum TruthAliasKind {
    Absent,
    Symlink,
    Regular,
    Special,
}

/// Publish the spoken WAV alias and its observer alias for one capture.
///
/// The mutex orders the two names. It does not make the two renames one
/// transaction: a truth-alias failure leaves the WAV alias in place.
fn publish_latest_capture_aliases(
    directory: &std::fs::File,
    id: &str,
    card_published: bool,
) -> Result<()> {
    static ALIAS_PUBLICATION: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _publication = ALIAS_PUBLICATION
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    publish_session_alias(directory, id)?;
    publish_truth_alias(directory, id, card_published).map_err(|error| {
        error.context(
            "latest truth alias was not published for this capture; wav alias already names it",
        )
    })
}

fn publish_truth_alias(directory: &std::fs::File, id: &str, card_published: bool) -> Result<()> {
    use std::os::fd::AsRawFd;
    let alias = c"last_session.wav.truth.json";
    match truth_alias_kind(directory, alias)? {
        TruthAliasKind::Special => {
            anyhow::bail!("latest truth alias is not a regular file or symlink");
        }
        TruthAliasKind::Regular => preserve_regular_truth_alias(directory, alias)?,
        TruthAliasKind::Symlink => {
            if unsafe { libc::unlinkat(directory.as_raw_fd(), alias.as_ptr(), 0) } < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        TruthAliasKind::Absent => {}
    }
    if !card_published {
        info!("latest truth alias cleared; this capture has no observer card");
        return Ok(());
    }
    let target = std::ffi::CString::new(format!("sessions/{id}.wav.truth.json"))?;
    let temporary = std::ffi::CString::new(format!(".retain-{}.tmp", Uuid::new_v4()))?;
    if unsafe { libc::symlinkat(target.as_ptr(), directory.as_raw_fd(), temporary.as_ptr()) } < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    if unsafe {
        libc::renameat(
            directory.as_raw_fd(),
            temporary.as_ptr(),
            directory.as_raw_fd(),
            alias.as_ptr(),
        )
    } < 0
    {
        let error = std::io::Error::last_os_error();
        unsafe { libc::unlinkat(directory.as_raw_fd(), temporary.as_ptr(), 0) };
        return Err(error.into());
    }
    Ok(())
}

fn truth_alias_kind(directory: &std::fs::File, name: &std::ffi::CStr) -> Result<TruthAliasKind> {
    use std::os::fd::AsRawFd;
    let mut info: libc::stat = unsafe { std::mem::zeroed() };
    let rc = unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            name.as_ptr(),
            &mut info,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if rc < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            return Ok(TruthAliasKind::Absent);
        }
        return Err(error.into());
    }
    match info.st_mode & libc::S_IFMT {
        libc::S_IFLNK => Ok(TruthAliasKind::Symlink),
        libc::S_IFREG => Ok(TruthAliasKind::Regular),
        _ => Ok(TruthAliasKind::Special),
    }
}

/// Move a regular previous card aside. Expiry does not own that name.
fn preserve_regular_truth_alias(directory: &std::fs::File, alias: &std::ffi::CStr) -> Result<()> {
    use std::os::fd::AsRawFd;
    let stale = std::ffi::CString::new(format!(
        "last_session.wav.truth.{}.stale.json",
        Uuid::new_v4()
    ))?;
    if unsafe {
        libc::renameat(
            directory.as_raw_fd(),
            alias.as_ptr(),
            directory.as_raw_fd(),
            stale.as_ptr(),
        )
    } < 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    info!(
        "preserved previous latest truth card as {}",
        stale.to_string_lossy()
    );
    Ok(())
}

/// Link the verified source inode through a temporary destination name, then
/// publish atomically. If the volume refuses links, copy from the pinned file
/// descriptor and emit an explicit receipt for the extra physical bytes.
fn publish_retained_link(
    source_directory: &std::fs::File,
    source_name: &std::ffi::CStr,
    source: &mut std::fs::File,
    directory: &std::fs::File,
    name: &std::ffi::CStr,
) -> Result<()> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::MetadataExt;

    let temporary = std::ffi::CString::new(format!(".retain-{}.tmp", Uuid::new_v4()))?;
    // SAFETY: held parent descriptors and C strings survive the call.
    let linked = unsafe {
        libc::linkat(
            source_directory.as_raw_fd(),
            source_name.as_ptr(),
            directory.as_raw_fd(),
            temporary.as_ptr(),
            0,
        )
    } == 0;
    if linked {
        let candidate = open_retention_entry(directory, &temporary, libc::O_RDONLY)?;
        let expected = source.metadata()?;
        let actual = candidate.metadata()?;
        if expected.dev() == actual.dev() && expected.ino() == actual.ino() {
            // SAFETY: this renames only the freshly linked entry in the held directory.
            if unsafe {
                libc::renameat(
                    directory.as_raw_fd(),
                    temporary.as_ptr(),
                    directory.as_raw_fd(),
                    name.as_ptr(),
                )
            } == 0
            {
                return Ok(());
            }
        }
        // SAFETY: only our fresh temporary name is removed.
        unsafe { libc::unlinkat(directory.as_raw_fd(), temporary.as_ptr(), 0) };
    }
    warn!(destination = %name.to_string_lossy(), "audio hardlink unavailable; copying pinned source");
    publish_retained_audio(source, directory, name)
}

/// Resolve only the trusted parent (including platform aliases such as /var),
/// then walk its absolute components with held directory descriptors. The leaf
/// is never canonicalized. A changed ancestor symlink fails closed during the
/// walk; later renames cannot redirect operations on the held descriptors.
/// Existing ancestors are required; only the root leaf and sessions are created.
fn open_retention_parent(path: &std::path::Path) -> Result<(std::fs::File, std::ffi::CString)> {
    use std::os::unix::ffi::OsStrExt;
    use std::path::Component;

    anyhow::ensure!(
        !path
            .components()
            .any(|part| matches!(part, Component::ParentDir)),
        "audio retention refuses parent traversal"
    );
    let leaf = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("audio retention requires a leaf"))?;
    let name = std::ffi::CString::new(leaf.as_bytes())?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let resolved = parent.canonicalize()?;
    let mut directory = std::fs::File::open("/")?;
    for component in resolved.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(part) => {
                let part = std::ffi::CString::new(part.as_bytes())?;
                directory =
                    open_retention_entry(&directory, &part, libc::O_RDONLY | libc::O_DIRECTORY)?;
            }
            _ => anyhow::bail!("audio retention parent is not absolute and normalized"),
        }
    }
    Ok((directory, name))
}

/// Open exactly one directory-relative entry, never a symlink or a path walk.
fn open_retention_entry(
    directory: &std::fs::File,
    name: &std::ffi::CStr,
    flags: libc::c_int,
) -> Result<std::fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd};

    anyhow::ensure!(
        !name.to_bytes().is_empty()
            && !name.to_bytes().contains(&b'/')
            && name.to_bytes() != b"."
            && name.to_bytes() != b"..",
        "audio retention requires one safe component"
    );
    // SAFETY: the directory and NUL-terminated name live through openat. A
    // successful descriptor has exactly one File owner; all errors close none.
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(unsafe { std::fs::File::from_raw_fd(fd) })
}

/// mkdirat cannot follow the leaf; openat rejects an existing link or special
/// file even if it was substituted between creation and opening.
fn retention_subdirectory(
    directory: &std::fs::File,
    name: &std::ffi::CStr,
) -> Result<std::fs::File> {
    use std::os::fd::AsRawFd;

    // SAFETY: caller supplies a single component and both arguments stay live.
    if unsafe { libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o700) } < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(error.into());
        }
    }
    open_retention_entry(directory, name, libc::O_RDONLY | libc::O_DIRECTORY)
}

/// Copy to an exclusively created inode, then replace only the destination
/// entry. No destination is opened for writing, so symlinks and hardlinks cannot
/// truncate their referents, even when the destination names the source itself.
fn publish_retained_audio(
    source: &mut std::fs::File,
    directory: &std::fs::File,
    name: &std::ffi::CStr,
) -> Result<()> {
    use std::io::{Seek, SeekFrom};
    use std::os::fd::AsRawFd;

    let temporary = std::ffi::CString::new(format!(".retain-{}.tmp", Uuid::new_v4()))?;
    let mut output = open_retention_entry(
        directory,
        &temporary,
        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
    )?;
    let result = (|| -> Result<()> {
        source.seek(SeekFrom::Start(0))?;
        std::io::copy(source, &mut output)?;
        output.sync_all()?;
        // SAFETY: both names and the held directory survive the call. renameat
        // replaces the leaf entry, never follows it, and stays on this directory.
        if unsafe {
            libc::renameat(
                directory.as_raw_fd(),
                temporary.as_ptr(),
                directory.as_raw_fd(),
                name.as_ptr(),
            )
        } < 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    })();
    if result.is_err() {
        // SAFETY: unlink only the temporary entry relative to the held directory.
        if unsafe { libc::unlinkat(directory.as_raw_fd(), temporary.as_ptr(), 0) } < 0 {
            return result.context(format!(
                "retention temporary cleanup failed: {}",
                std::io::Error::last_os_error()
            ));
        }
    }
    result
}

/// Consume the producer error while the terminal caller still owns its take.
/// Both the controller id and the frozen recorder epoch must agree before any
/// archive side effect. Context keeps the typed error and its original cause.
fn recover_capture_stop_failure(
    mut error: anyhow::Error,
    session_id: Option<&str>,
    capture_identity: (Option<&str>, u64),
    retain: impl FnOnce(&str, &std::path::Path) -> Result<()>,
) -> anyhow::Error {
    let Some(failure) = error.downcast_ref::<CaptureStopFailure>() else {
        return error;
    };
    let id = retainable_session_id(session_id);
    if id.is_none()
        || id != failure.session_id.as_deref()
        || capture_identity.0 != failure.session_id.as_deref()
        || capture_identity.1 == 0
        || capture_identity.1 != failure.capture_epoch
    {
        return error.context("capture recovery refused: session/epoch identity mismatch");
    }
    let archive_failure = failure
        .cause
        .downcast_ref::<codescribe_core::audio::recorder::CaptureArchiveError>();
    let recovered = match archive_failure {
        Some(archive) => match archive.recover_complete_archive() {
            Ok(recovered) => Some(recovered),
            Err(recovery_error) => {
                return error.context(format!(
                    "CRITICAL: complete audio recovery failed: {recovery_error:#}; original PCM evidence remains protected"
                ));
            }
        },
        None => None,
    };
    let audio_path = recovered
        .as_ref()
        .map(|recovered| recovered.path().to_path_buf())
        .or_else(|| failure.audio_path.clone());
    if let Some(path) = audio_path {
        if let Err(archive_error) = retain(id.expect("identity checked"), &path) {
            let message = format!(
                "CRITICAL: recovery retention failed for {}: {archive_error:#}; original processing error: {error:#}",
                path.display(),
            );
            return error.context(message);
        }
        if let Some(recovered) = recovered {
            if let Some(archive) = error
                .downcast_ref::<CaptureStopFailure>()
                .and_then(|failure| {
                    failure
                        .cause
                        .downcast_ref::<codescribe_core::audio::recorder::CaptureArchiveError>()
                })
                && let Err(ack) = archive.acknowledge_recovery(&recovered)
            {
                return error.context(format!("recovered archive ownership refused: {ack:#}"));
            }
            if let Some(failure) = error.downcast_mut::<CaptureStopFailure>() {
                failure.audio_path = Some(path);
            }
            // Only a successful retention consumer may return this core proof.
            // The caller acknowledges it after any blocking worker has joined.
            return error.context(recovered);
        }
    }
    error
}

/// The archive a refused terminal take deserves.
///
/// The ledger refused the SEAL, not the words: `committed_text` is the
/// reducer-committed document at refusal time, and archiving it keeps the take
/// readable in history instead of a `_failed` audio bag with no text while the
/// overlay was showing a full formatted document (operator take 2026-09-24
/// 08:23). No seal is claimed — the daily bag writes it as `Raw`. An empty
/// document keeps the diagnostic-only retention.
/// Archived transcript of the latest refused take that kept its committed
/// words, keyed by session. The stop classifier that archives it has no
/// controller; the controller that hands those words to the user takes it
/// and links the take's live history to it. One slot: a newer refused take
/// replaces an unclaimed older one.
static REFUSED_TAKE_ARCHIVE: std::sync::Mutex<Option<(String, std::path::PathBuf)>> =
    std::sync::Mutex::new(None);

fn take_refused_take_archive(session_id: &str) -> Option<std::path::PathBuf> {
    let mut slot = REFUSED_TAKE_ARCHIVE
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    match slot.take() {
        Some((session, path)) if session == session_id => Some(path),
        other => {
            *slot = other;
            None
        }
    }
}

fn refused_take_archive(
    refusal: &TerminalSealRefused,
) -> codescribe_core::state::SessionTranscriptArchive<'_> {
    if refusal.committed_text.trim().is_empty() {
        codescribe_core::state::SessionTranscriptArchive::Unavailable(
            refusal.finality.reason().as_str(),
        )
    } else {
        codescribe_core::state::SessionTranscriptArchive::Committed(&refusal.committed_text)
    }
}

// One stop-instant budget covers every live lane, including an empty take.
// Refinement, archive and formatter work never own this deadline. The stop
// snapshot already holds every heard word through the Apple mirror, so the
// live final only anchors them; a longer wait buys nothing and reads as a hang.
const STOP_FINAL_BOUND: Duration = Duration::from_secs(2);

/// A next-start request interrupts only the live-final wait, never capture.
/// The global weak slot lets the event tap wake a stop even while its serial
/// receiver is still awaiting that stop. Registration owns the signal lifetime.
#[derive(Default)]
struct StopPasteWaitControl {
    preempted: AtomicBool,
    wake: tokio::sync::Notify,
}

static STOP_PASTE_WAIT: std::sync::Mutex<Option<std::sync::Weak<StopPasteWaitControl>>> =
    std::sync::Mutex::new(None);

pub(crate) fn preempt_stop_paste_for_next_take() -> bool {
    #[cfg(test)]
    let _lane = crate::test_env::StopPastePreemptionTestLane::acquire();
    let slot = STOP_PASTE_WAIT
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let Some(control) = slot.as_ref().and_then(std::sync::Weak::upgrade) else {
        return false;
    };
    control.preempted.store(true, Ordering::SeqCst);
    control.wake.notify_one();
    true
}

struct StopPasteWaitRegistration(Arc<StopPasteWaitControl>);

impl StopPasteWaitRegistration {
    fn new() -> Self {
        let control = Arc::new(StopPasteWaitControl::default());
        *STOP_PASTE_WAIT
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(Arc::downgrade(&control));
        Self(control)
    }

    fn close(&self) -> bool {
        let mut slot = STOP_PASTE_WAIT
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if slot
            .as_ref()
            .is_some_and(|current| current.ptr_eq(&Arc::downgrade(&self.0)))
        {
            *slot = None;
        }
        self.0.preempted.load(Ordering::SeqCst)
    }
}

impl Drop for StopPasteWaitRegistration {
    fn drop(&mut self) {
        self.close();
    }
}

impl StopPasteWaitControl {
    async fn requested(&self) {
        if !self.preempted.load(Ordering::SeqCst) {
            self.wake.notified().await;
        }
    }
}

/// Receipt-only join of stop settlement and the first post-bound live final.
/// Owned by the take's event sink, including when that sink drains after retirement.
#[derive(Default)]
struct LateFinalReceipt {
    take_id: Option<String>,
    stopped_at: Option<tokio::time::Instant>,
    pasted: Option<String>,
    final_arrival: Option<(String, u128)>,
    closed: bool,
}

impl LateFinalReceipt {
    fn begin(&mut self, take_id: Option<&str>, stopped_at: tokio::time::Instant) {
        self.take_id = take_id.map(str::to_owned);
        self.stopped_at = Some(stopped_at);
    }

    fn settle(&mut self, take_id: Option<&str>, timed_out: bool, pasted: &str) {
        if self.closed || self.take_id.as_deref() != take_id {
            return;
        }
        if !timed_out {
            self.closed = true;
            self.final_arrival = None;
            return;
        }
        self.pasted = Some(pasted.to_owned());
        self.emit_if_ready();
    }

    fn observe(&mut self, event: &EngineEvent) {
        if self.closed || self.final_arrival.is_some() {
            return;
        }
        let EngineEvent::UtteranceFinal { text, .. } = event else {
            return;
        };
        let Some(stopped_at) = self.stopped_at else {
            return;
        };
        let elapsed = stopped_at.elapsed();
        if elapsed < STOP_FINAL_BOUND {
            return;
        }
        self.final_arrival = Some((text.clone(), elapsed.as_millis()));
        self.emit_if_ready();
    }

    fn emit_if_ready(&mut self) {
        let (Some(pasted), Some((final_text, elapsed)), Some(take_id)) =
            (&self.pasted, &self.final_arrival, &self.take_id)
        else {
            return;
        };
        let missing = late_final_missing_words(pasted, final_text);
        info!(
            take_id = %take_id,
            late_final_words_after_bound = missing,
            late_final_after_bound_ms = *elapsed,
            "stop canvas late final receipt"
        );
        self.closed = true;
        self.pasted = None;
        self.final_arrival = None;
    }
}

/// Count unmatched final words using an ordered, multiplicity-preserving match.
/// Use the same outward marker stripping as paste; case and punctuation remain
/// significant. LCS avoids turning one insertion into a cascade of mismatches.
fn late_final_missing_words(pasted: &str, final_text: &str) -> usize {
    let pasted = context_bucket::strip_markers_for_delivery(pasted);
    let final_text = context_bucket::strip_markers_for_delivery(final_text);
    let words: Vec<_> = final_text.split_whitespace().collect();
    let mut matched = vec![0_usize; words.len() + 1];
    for pasted_word in pasted.split_whitespace() {
        let mut diagonal = 0;
        for (index, final_word) in words.iter().enumerate() {
            let previous = matched[index + 1];
            matched[index + 1] = if pasted_word == *final_word {
                diagonal + 1
            } else {
                matched[index + 1].max(matched[index])
            };
            diagonal = previous;
        }
    }
    words.len() - matched[words.len()]
}

struct TakeExternalEventSink {
    stop_receipt: Arc<std::sync::Mutex<LateFinalReceipt>>,
    presentation: Arc<PresentationEmitter>,
    sink: Arc<dyn codescribe_core::pipeline::contracts::EventSink>,
}

impl codescribe_core::pipeline::contracts::EventSink for TakeExternalEventSink {
    fn consultation_destinations(&self) -> usize {
        self.presentation.consultation_destinations()
    }

    fn on_consultation_completed(
        &self,
        completed: &codescribe_core::agent::consultation::ConsultationGroupAnswer,
    ) -> Result<()> {
        self.presentation.on_consultation_completed(completed)
    }

    fn on_capture_opened(&self, session_id: &str, capture_epoch: u64) {
        self.presentation
            .on_capture_opened(session_id, capture_epoch);
        self.sink.on_capture_opened(session_id, capture_epoch);
    }

    fn close_participation(&self, session_id: &str, capture_epoch: u64) -> Result<()> {
        self.presentation
            .close_participation(session_id, capture_epoch)
    }

    fn wait_presentation_published<'a>(
        &'a self,
        session_id: &'a str,
        capture_epoch: u64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        self.presentation
            .wait_presentation_published(session_id, capture_epoch)
    }

    fn on_event(&self, event: &EngineEvent) {
        // The same take sink routes events and completion to one reducer/FIFO.
        // External observers still see each event after reducer admission.
        self.presentation.on_event(event);
        self.stop_receipt
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .observe(event);
        self.presentation
            .with_active_presentation(|| self.sink.on_event(event));
    }
}

struct StopCanvasRequest {
    stopped_at: std::time::Instant,
    painted_at_stop: Option<VisibleCanvasSnapshot>,
    target: clipboard::StopPasteTarget,
    preemption: StopPasteWaitRegistration,
}

struct StopCanvasDelivery {
    delivery: Result<Option<TranscriptDelivery>>,
    preempted: bool,
}

struct StopCanvasWait {
    snapshot: Option<VisibleCanvasSnapshot>,
    stop_final_wait_ms: u128,
    stop_final_timeout: bool,
    live_finals_admitted: bool,
    painted_at_stop: Option<VisibleCanvasSnapshot>,
    preempted: bool,
    armed_order: bool,
}

impl StopCanvasWait {
    fn permits_first_delivery(&self, take_id: Option<&str>) -> bool {
        !self.preempted
            && self.live_finals_admitted
            && self.snapshot.as_ref().is_some_and(|canvas| {
                Some(canvas.session_id.as_str()) == take_id
                    && canvas.has_committed_document
                    && canvas.preview_only_words == 0
                    && self.painted_at_stop.as_ref().is_none_or(|before| {
                        before.session_id == canvas.session_id
                            && before.capture_epoch == canvas.capture_epoch
                            && before.revision <= canvas.revision
                            && !requires_terminal_accounting(&before.missing_words_from(canvas))
                    })
            })
    }
}

/// Phrase closure and volatile paint transitions are lifecycle evidence, not
/// proof that the chosen committed projection accounts for their PCM. Leave
/// those takes to the owned terminal drain rather than claiming an early paste.
/// Relabel/shape receipts may replace wording without any lexical word floor.
fn requires_terminal_accounting(
    missing: &[crate::presentation::emitter::MissingVisibleWord],
) -> bool {
    missing.iter().any(|word| {
        !word.reason.split("; ").all(|reason| {
            reason.starts_with("covered_by_committed occurrence=")
                || reason.starts_with("relabeled_in_place occurrence=")
                || reason.starts_with("reshaped_in_place occurrence=")
        })
    })
}

async fn await_live_finals_for_delivery(
    finals: impl std::future::Future<Output = bool>,
    snapshot: impl Fn() -> Option<VisibleCanvasSnapshot>,
    stopped_at: tokio::time::Instant,
    painted_at_stop: Option<VisibleCanvasSnapshot>,
    armed_order: bool,
    preemption: Option<&StopPasteWaitControl>,
) -> StopCanvasWait {
    let preempt = async {
        match preemption {
            Some(control) => control.requested().await,
            None => std::future::pending::<()>().await,
        }
    };
    let (settled, preempted) = tokio::select! {
        biased;
        () = preempt => (None, true),
        result = tokio::time::timeout_at(stopped_at + STOP_FINAL_BOUND, finals) =>
            (Some(result), false),
    };
    StopCanvasWait {
        snapshot: snapshot(),
        stop_final_wait_ms: stopped_at.elapsed().as_millis(),
        stop_final_timeout: preempted || matches!(settled, Some(Err(_))),
        live_finals_admitted: matches!(settled, Some(Ok(true))),
        painted_at_stop,
        preempted,
        armed_order,
    }
}

/// Empty retention is just as settled as an accepted paste. Only the composer
/// defers delivery to the terminal path; the terminal result cannot reopen it.
async fn deliver_terminal_unless_settled<F, Fut>(
    settled: Option<TranscriptDelivery>,
    deliver: F,
) -> Result<TranscriptDelivery>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<TranscriptDelivery>>,
{
    match settled {
        Some(delivery) => Ok(delivery),
        None => deliver().await,
    }
}

/// Settle the original closed take and classify the outcome.
///
/// A ledger refusal of the terminal transcript ([`TerminalSealRefused`]) is a
/// legitimate take outcome, not a recorder failure: the capture stopped and the
/// take WAV is already on disk. That audio is retained under the Bus uuid here,
/// before the refusal is returned, so the take survives for Retranscribe and the
/// error the user sees names the refused seal rather than the mic. Any other
/// processing error carries producer-owned recovery evidence when available.
///
/// Incident 2026-09-02 02:17 UTC (`~/.codescribe/logs/codescribe.log`): a quiet
/// take (-57.9 dB) ended with `terminal_seal_coverage_incomplete`; the toggle
/// stop propagated the refusal with `?` past its own state reset, the
/// controller stayed `Busy` for good, the Bus session never ended and every
/// later Finish press was ignored until the app was restarted.
async fn stop_closed_take_for_terminal(
    mut take: ClosedTake,
    session_id: Option<&str>,
) -> (
    Result<(String, Option<std::path::PathBuf>)>,
    codescribe_core::pipeline::take_truth::TakeTruth,
) {
    // This helper runs in an already tracked, retired take tail. A timeout
    // is an incomplete publication attempt, never permission to drop its sink.
    let mut pending_reported = false;
    loop {
        let (stopped, observer, pending) =
            try_stop_closed_take_for_terminal(take, session_id).await;
        let Some(original) = pending else {
            return (stopped, observer);
        };
        if !pending_reported {
            warn!(session_id, error = ?stopped.as_ref().err(),
                "closed take publication pending; original owner retained for retry");
            pending_reported = true;
        }
        take = original;
        // Permanent refusal stays visibly unsettled. Backoff prevents a
        // disconnected publisher from creating a busy retry loop.
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// One foreground attempt. An incomplete publication transfers its original
/// owner back to the controller, before any Bus end or history classification.
async fn try_stop_closed_take_for_terminal(
    mut take: ClosedTake,
    session_id: Option<&str>,
) -> (
    Result<(String, Option<std::path::PathBuf>)>,
    codescribe_core::pipeline::take_truth::TakeTruth,
    Option<ClosedTake>,
) {
    let (session, epoch) = take.capture_identity();
    let session = session.map(str::to_owned);
    let ledger = take.acoustic_ledger_handle();
    let stopped = take.finish().await;
    let retention_id = if take.is_shared_document() {
        session.as_deref()
    } else {
        session_id
    };
    let observer = observe_take_ledger((session.as_deref(), epoch), ledger, retention_id);
    if !take.is_settled() {
        return (stopped, observer, Some(take));
    }
    if let Some((capture, path)) = take.take_shared_capture_archive() {
        retain_session_audio(
            Some(&capture),
            &path,
            codescribe_core::state::SessionTranscriptArchive::Unavailable(
                "shared capture; transcript documents belong to participation intervals",
            ),
            &observer,
        )
        .await;
    }
    let released_capture = take.released_shared_capture().map(str::to_owned);
    let classified = classify_terminal_stop(
        stopped,
        retention_id,
        session.clone(),
        epoch,
        observer.clone(),
    )
    .await;
    finish_audio_capture(released_capture.as_deref());
    (classified, observer, None)
}

/// Retain one document through the existing history owner. A shared interval
/// without an exact WAV still owns text; it never claims the whole capture's
/// audio. The capture archive is retained separately under its physical id.
async fn retain_stopped_document(
    session_id: Option<&str>,
    capture_session_id: Option<&str>,
    text: &str,
    audio_path: Option<&std::path::Path>,
    shared_document: bool,
    observer: &codescribe_core::pipeline::take_truth::TakeTruth,
) -> Option<std::path::PathBuf> {
    if let Some(path) = audio_path {
        let lease = retainable_session_id(capture_session_id.or(session_id)).and_then(|id| {
            codescribe_core::state::history::audio_retention::capture(&Config::config_dir(), id)
        });
        let archived = retain_session_audio_with_lease(
            session_id,
            path,
            codescribe_core::state::SessionTranscriptArchive::from_committed(text),
            lease,
            observer,
        )
        .await;
        if shared_document {
            finish_audio_capture(capture_session_id);
        }
        return archived;
    }
    if !shared_document || text.trim().is_empty() {
        return None;
    }
    let text = text.to_string();
    match tokio::task::spawn_blocking(move || {
        codescribe_core::state::history::save_entry(&text).path
    })
    .await
    {
        Ok(path) if path.is_file() => Some(path),
        Ok(path) => {
            warn!(?session_id, path = %path.display(), "shared document history was not saved");
            None
        }
        Err(error) => {
            warn!(?session_id, %error, "shared document history worker failed");
            None
        }
    }
}

async fn classify_terminal_stop(
    stopped: Result<(String, Option<std::path::PathBuf>)>,
    session_id: Option<&str>,
    capture_session: Option<String>,
    capture_epoch: u64,
    observer: codescribe_core::pipeline::take_truth::TakeTruth,
) -> Result<(String, Option<std::path::PathBuf>)> {
    match stopped {
        Ok(stopped) => Ok(stopped),
        Err(err) => match err.downcast::<TerminalSealRefused>() {
            Ok(refusal) => {
                warn!(
                    session_id = ?session_id,
                    reason = refusal.finality.reason().as_str(),
                    coverage = ?refusal.finality.coverage(),
                    "terminal transcript refused after a successful capture stop; retaining take audio"
                );
                match refusal.audio_path.as_deref() {
                    Some(path) => {
                        let archived = retain_session_audio(
                            session_id,
                            path,
                            refused_take_archive(&refusal),
                            &observer,
                        )
                        .await;
                        if let (Some(id), Some(archived)) = (session_id, archived) {
                            *REFUSED_TAKE_ARCHIVE
                                .lock()
                                .unwrap_or_else(|error| error.into_inner()) =
                                Some((id.to_string(), archived));
                        }
                    }
                    None => warn!("refused take has no audio path to retain"),
                }
                Err(anyhow::Error::new(refusal))
            }
            Err(err) => {
                if err.downcast_ref::<CaptureStopFailure>().is_some() {
                    let session_id = session_id.map(str::to_owned);
                    let lease = retainable_session_id(session_id.as_deref()).and_then(|id| {
                        codescribe_core::state::history::audio_retention::capture(
                            &Config::config_dir(),
                            id,
                        )
                    });
                    if let Some(lease) = lease.as_ref() {
                        lease.protect_retry();
                    }
                    let recovered = tokio::task::spawn_blocking(move || {
                        recover_capture_stop_failure(
                            err,
                            session_id.as_deref(),
                            (capture_session.as_deref(), capture_epoch),
                            |id, path| {
                                retain_owned_session_audio(
                                    Some(id),
                                    path,
                                    codescribe_core::state::SessionTranscriptArchive::Unavailable(
                                        "capture processing failed; committed text unavailable",
                                    ),
                                    &observer,
                                    lease.as_deref(),
                                )
                            },
                        )
                    })
                    .await;
                    let recovered = match recovered {
                        Ok(error) => error,
                        Err(error) => anyhow::anyhow!("audio recovery worker failed: {error}"),
                    };
                    Err(recovered)
                } else {
                    Err(err.context("Failed to stop recorder"))
                }
            }
        },
    }
}

/// Exactly-once gate for stop-path delivery. Returns `true` when this take may
/// deliver and records it. A take without an id cannot be deduplicated and
/// always passes; the toggle path's `:stopping` suffix is not part of identity.
/// Marker the toggle stop appends to the live session slot so a second start
/// cannot mistake a take that is winding down for a take that is still open.
const STOPPING_SUFFIX: &str = ":stopping";

/// What a stop that names its capture actually did.
///
/// Typed because every one of these is a state the caller must act on
/// differently, and because "the stop returned Ok" is not the same claim as
/// "the take this gesture opened is the take that stopped".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureStopOutcome {
    /// The stop path ran for the intended capture.
    Stopped,
    /// A different capture owns the microphone; it was left running and no
    /// new take was started in its place.
    ForeignCapture,
    /// Nothing is capturing. Nothing stopped, nothing started.
    NoLiveCapture,
    /// The named capture is already inside its own stop path.
    AlreadyStopping,
    /// The controller owns the request, but terminal settlement is still owed.
    Pending,
    /// No operation was admitted. The caller keeps its handle and may retry.
    AdmissionUnavailable,
}

/// Produced inside the start's serial section, never inferred from a later
/// shared-slot read. Adapted from Claude's isolated 0f47c1201 admission work.
#[derive(Debug, PartialEq, Eq)]
enum CaptureAdmission {
    Admitted(String),
    NotAdmitted,
}

type CaptureSettlementResult = std::result::Result<CaptureStopOutcome, String>;

/// One slot, including the last completed result. No per-take task graveyard.
/// The task owns the controller until settlement; dropping a caller only drops
/// its watch receiver. Replacing this slot is allowed only after the task exits.
struct CaptureSettlement {
    capture_id: String,
    result: watch::Receiver<Option<CaptureSettlementResult>>,
    closed: watch::Receiver<bool>,
    task: JoinHandle<()>,
}

enum CaptureStopAdmission {
    Settling(
        watch::Receiver<Option<CaptureSettlementResult>>,
        watch::Receiver<bool>,
    ),
    Immediate(CaptureStopOutcome),
}

#[cfg(test)]
#[derive(Debug, PartialEq, Eq)]
enum CaptureSettlementStage {
    Registered,
    Admitted,
    Resetting,
}

/// A one-turn take is opened by the Agent composer and by nothing else — that
/// surface is the only caller that sets [`CaptureTurnIntent::SingleTurn`]. Its
/// destination is therefore the composer draft, not a system paste sink.
const fn take_delivers_to_composer(capture_turn: CaptureTurnIntent) -> bool {
    matches!(capture_turn, CaptureTurnIntent::SingleTurn)
}

fn claim_take_delivery(delivered: &mut Option<String>, take_id: Option<&str>) -> bool {
    let Some(take_id) = take_id
        .map(|id| id.trim().trim_end_matches(":stopping"))
        .filter(|id| !id.is_empty())
    else {
        return true;
    };
    if delivered.as_deref() == Some(take_id) {
        return false;
    }
    *delivered = Some(take_id.to_string());
    true
}

#[cfg(test)]
mod take_delivery_tests {
    use super::claim_take_delivery;

    #[test]
    fn a_take_delivers_once_and_a_new_take_delivers_again() {
        let mut delivered = None;
        assert!(claim_take_delivery(&mut delivered, Some("take-a")));
        assert!(!claim_take_delivery(&mut delivered, Some("take-a")));
        assert!(!claim_take_delivery(
            &mut delivered,
            Some("take-a:stopping")
        ));
        assert!(claim_take_delivery(&mut delivered, Some("take-b")));
        assert_eq!(delivered.as_deref(), Some("take-b"));
    }

    #[test]
    fn a_take_without_identity_cannot_be_deduplicated() {
        let mut delivered = Some("take-a".to_string());
        assert!(claim_take_delivery(&mut delivered, None));
        assert!(claim_take_delivery(&mut delivered, Some("  ")));
        assert_eq!(delivered.as_deref(), Some("take-a"));
    }
}

#[cfg(test)]
mod session_audio_id_tests {
    use super::{retainable_session_id, session_audio_path, valid_session_audio_id};

    #[test]
    fn uuid_session_ids_are_assigned_under_sessions_not_last_session() {
        let id = "fa3fd371-db9b-4bd5-8fc2-3a5940fa62a3";
        assert_eq!(valid_session_audio_id(id), Some(id));
        let path = session_audio_path(std::path::Path::new("unused-root"), id).expect("path");
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("fa3fd371-db9b-4bd5-8fc2-3a5940fa62a3.wav")
        );
        assert_eq!(
            path.parent()
                .and_then(|parent| parent.file_name())
                .and_then(|name| name.to_str()),
            Some("sessions")
        );
    }

    #[test]
    fn traversal_and_short_ids_are_refused() {
        assert!(valid_session_audio_id("../etc").is_none());
        assert!(valid_session_audio_id("short").is_none());
        assert!(valid_session_audio_id("").is_none());
        assert!(session_audio_path(std::path::Path::new("unused-root"), "../etc").is_none());
    }

    #[test]
    fn toggle_stopping_suffix_still_retains_the_bus_uuid_wav() {
        let id = "c06528af-f156-4f05-a6e0-8f282d8b4a07";
        let stopping = format!("{id}:stopping");
        assert!(
            valid_session_audio_id(&stopping).is_none(),
            "colon is not a wav alphabet character"
        );
        assert_eq!(retainable_session_id(Some(&stopping)), Some(id));
        let path = session_audio_path(std::path::Path::new("unused-root"), id).expect("path");
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("c06528af-f156-4f05-a6e0-8f282d8b4a07.wav")
        );
    }
}

/// What one stop-and-process pass produced: the delivery decision plus the
/// per-phase wall clock that the stop-path budget line reports.
#[derive(Debug, Clone, Default)]
struct ProcessRecordingOutcome {
    no_speech_reason: Option<String>,
    commit_trigger: Option<String>,
    transcript_present: bool,
    /// Capture settled, but acoustic completeness was refused. Never a seal.
    refusal: Option<TerminalSealRefused>,
}

/// A selected sink failed after capture settled; the words remain recoverable.
#[derive(Debug)]
struct StopDeliveryFailure {
    cause: anyhow::Error,
    refusal: Option<TerminalSealRefused>,
}

impl std::fmt::Display for StopDeliveryFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Transcript retained; destination handoff failed: {:#}",
            self.cause
        )
    }
}

impl std::error::Error for StopDeliveryFailure {}

/// One Max owner event with the identity needed to attribute it to a turn.
#[derive(Debug, Clone)]
pub struct MaxConsultationEvent {
    pub consultation_id: String,
    pub turn_id: String,
    pub event: codescribe_core::agent::AgentUiEvent,
}

/// Recording controller managing state machine and lifecycle
pub struct RecordingController {
    /// The one mutable controller generation handle. Each take clones this Arc
    /// once and every config, user-settings, LLM and recorder fact comes from it.
    /// The Arc inside is replaced only when idle so an active take keeps its
    /// generation even if Settings writes a later snapshot.
    runtime_settings: RwLock<Arc<RuntimeSettingsSnapshot>>,
    /// Persisted intent changed while a take owned the current generation.
    /// This is invalidation only; the loader remains the settings authority.
    runtime_settings_refresh_pending: AtomicBool,

    /// Current state
    state: Arc<RwLock<State>>,

    /// Audio recorder instance
    recorder: Arc<Mutex<Option<StreamingRecorder>>>,

    /// Whether AI assistive mode is enabled for the current session.
    ///
    /// This is true for:
    /// - Hold modes: Chat (Shift) / Selection (Cmd)
    /// - Assistive toggle (started from the menu bar or the overlay)
    assistive_mode: Arc<RwLock<bool>>,
    /// Current hold intent (Raw/Chat/Selection) for the active session.
    hold_mode: Arc<RwLock<HoldMode>>,

    /// Whether to force RAW mode (Ctrl Hold without Shift = always raw, ignores AI toggle)
    /// Toggle mode (Double Option) keeps this false and respects the FORMATTING_LEVEL setting.
    force_raw_mode: Arc<RwLock<bool>>,
    /// Whether to force AI formatting for the current session (e.g., left double Option)
    force_ai_mode: Arc<RwLock<bool>>,

    /// Current session ID for tracking
    session_id: Arc<RwLock<Option<String>>>,
    /// The one observer bus for the active recording. Presentation may publish
    /// mutable drafts through it, but only the stop controller publishes the
    /// immutable product seal after every automatic stage completes.
    active_transcript_bus: Arc<RwLock<Option<Arc<TranscriptBus>>>>,
    /// Retained after microphone teardown so a terminal overlay edit can enter
    /// the exact reducer/ledger pair that authored the visible projection.
    /// Replaced atomically when the next take installs its own authority.
    active_presentation: Arc<RwLock<Option<Arc<PresentationEmitter>>>>,
    active_stop_receipt: Arc<RwLock<Arc<std::sync::Mutex<LateFinalReceipt>>>>,

    /// Max conversation survives capture teardown; microphone lifetime is not
    /// conversation lifetime. No chat selection or OS focus changes this slot.
    max_consultation: Mutex<Option<Arc<crate::agent::max_consultation::MaxConsultation>>>,
    /// The shared approval mechanism, scoped to this controller's Max owner.
    max_approvals: Arc<codescribe_core::agent::ApprovalBroker>,
    /// Live Max owner events (voice and composer turns alike) for any surface
    /// that renders a consultation turn while it runs. Lagging subscribers
    /// lose deltas, never history: the owner persists through the gateway.
    max_consultation_events: broadcast::Sender<MaxConsultationEvent>,

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
    shutdown_requested: Arc<AtomicBool>,
    /// Registration never suspends while holding this lock. Contention refuses
    /// admission immediately; no cleanup task is spawned without being tracked.
    capture_settlement: std::sync::Mutex<Option<CaptureSettlement>>,
    closed_capture_tails: std::sync::Mutex<Vec<JoinHandle<()>>>,
    /// The retained take's archived transcript, mirrored from the live
    /// document history so a reopened take keeps every version.
    live_archive: Arc<std::sync::Mutex<Option<live_archive::LiveArchiveMirror>>>,
    #[cfg(test)]
    settlement_observer: std::sync::Mutex<Option<mpsc::UnboundedSender<CaptureSettlementStage>>>,
    /// Instance-owned stand-in for the live acoustic admission probe.
    ///
    /// `None` keeps the historical unit-test default: start does not touch TCC.
    /// `Some(Ok(()))` continues as a granted admission without a live probe.
    /// `Some(Err(blocker))` takes the same warning-or-refusal match as production.
    /// This is not a process-global hook.
    #[cfg(test)]
    test_start_admission: std::sync::Mutex<Option<Result<(), admission::AdmissionBlocker>>>,

    /// Where the stop path sent this take's committed document.
    ///
    /// Written once per take by `deliver_stop_transcript`, reset at every
    /// start, read once by `end_transcript_bus` so the terminal lifecycle line
    /// states a destination instead of leaving an observer to infer one from a
    /// label. It records what the controller *attempted*; only the receiving
    /// surface can turn `ComposerPending` into an admitted delivery.
    delivery_disposition: Arc<RwLock<TranscriptDelivery>>,
    /// Clean reducer text is wrapped only after delivery routing. This passive
    /// observer retains the active session's real engine quality metadata.
    delivery_tagger: Arc<TranscriptDeliveryTagger>,
    /// Delivery-only composer bytes, keyed by the take that produced them.
    /// The terminal projection carries this separately from `rendered_text`.
    composer_delivery_payload: Arc<RwLock<Option<(String, String)>>>,

    /// Flag set by VAD (silence detection) when recording should auto-stop
    vad_triggered: Arc<AtomicBool>,

    /// Assistive hands-off loop active (the Assistive toggle)
    assistive_loop_active: Arc<AtomicBool>,

    /// Toggle session: track whether we've already appended user/assistant text
    toggle_user_has_text: Arc<AtomicBool>,
    toggle_assistant_has_text: Arc<AtomicBool>,

    /// Best-effort selected-text/app context captured for assistive sessions.
    ///
    /// Must be captured BEFORE showing any overlay window, because overlays
    /// may steal focus and destroy the user's selection context.
    assistive_context: Arc<RwLock<Option<AssistiveContext>>>,
    /// Trigger-time context retained after recording cleanup until the overlay
    /// either auto-sends the untouched final transcript or the user explicitly
    /// sends an edited transcript.
    pending_assistive_context: Arc<RwLock<Option<AssistiveContext>>>,
    /// Combo-collected selections for the active dictation. The bucket survives
    /// recording cleanup and is consumed only at the agent-send seam.
    context_bucket: Arc<Mutex<ContextBucket>>,
    /// App that was frontmost when the user initiated a hold session, before
    /// Codescribe badge/overlay UI can become frontmost.
    pre_overlay_frontmost_app: Arc<RwLock<Option<String>>>,
    /// Take id whose stop-path delivery already fired. One recording delivers
    /// exactly once (Founder C02 2026-07-20): a second stop of the same take
    /// archives only.
    delivered_take: Arc<Mutex<Option<String>>>,
    /// Speech clock of the live hands-free assistive take. Thirty seconds
    /// without voice end the take and archive it (`assistive_silence`).
    assistive_silence: std::sync::Mutex<Option<assistive_silence::AssistiveSilenceWatch>>,

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

    /// Open Fn+digit channels. Outside the take state machine.
    ///
    /// Lock order: this mutex before the recorder mutex.
    agent_channels: Mutex<std::collections::HashMap<u8, agent_channel::OpenAgentChannel>>,

    /// Live members of `agent_channels`, read by every open channel bus as its
    /// `co_audience`. Replaced whole only while `agent_channels` is held.
    agent_channel_cohort: crate::presentation::transcript_bus::ChannelCohort,

    /// The ack watcher and channel silence cap already have a task.
    channel_guards_started: AtomicBool,
}

/// The shared handles one conversation audio loop moves into its task.
///
/// Named fields keep the set of handles crossing the thread boundary explicit;
/// the loop destructures it on entry so the body reads exactly as before.
struct ConversationLoopHandles {
    engine: Arc<Mutex<Option<ConversationEngine>>>,
    player: Arc<Mutex<Option<AudioPlayer>>>,
    recorder: Arc<Mutex<Option<StreamingRecorder>>>,
    stop_flag: Arc<AtomicBool>,
    generation_counter: Arc<AtomicU64>,
    state: Arc<RwLock<State>>,
    event_broadcast: broadcast::Sender<IpcEvent>,
}

/// Resources acquired before state assembly. Inert inputs carry no recorder,
/// model discovery or prewarm; they still use the actual controller lifecycle.
pub struct ControllerStartupResources {
    recorder: Option<StreamingRecorder>,
}

impl ControllerStartupResources {
    pub fn inert() -> Self {
        Self { recorder: None }
    }
}

impl RecordingController {
    /// One phrasing for "there is no recorder", logged and returned together so
    /// a caller cannot report the failure in a way the log does not corroborate.
    fn recorder_unavailable_error(context: &str) -> anyhow::Error {
        warn!("{context}: streaming recorder unavailable; voice capture is disabled");
        anyhow::anyhow!("{context}: streaming recorder unavailable")
    }

    /// Best-effort recorder construction at controller init. A missing audio
    /// device disables voice capture but must not prevent the app from starting,
    /// so the failure degrades to `None` plus a warning.
    fn init_streaming_recorder(context: &str) -> Option<StreamingRecorder> {
        crate::config::note_startup_acquisition("streaming recorder");
        match StreamingRecorder::new() {
            Ok(recorder) => Some(recorder),
            Err(error) => {
                warn!("{context}: failed to initialize streaming recorder: {error}");
                None
            }
        }
    }

    /// Mutable recorder out of a held lock guard, or the unavailable error.
    fn recorder_from_guard_mut<'a>(
        recorder_guard: &'a mut Option<StreamingRecorder>,
        context: &str,
    ) -> Result<&'a mut StreamingRecorder> {
        recorder_guard
            .as_mut()
            .ok_or_else(|| Self::recorder_unavailable_error(context))
    }

    /// Shared recorder out of a held lock guard, or the unavailable error.
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
        let snapshot = Config::load_startup_runtime_snapshot(true);
        Self::with_runtime_settings(snapshot, "RecordingController::new")
    }

    /// Create a new recording controller without populating secrets from Keychain.
    ///
    /// Used by the SwiftUI redesign dictation bridge: starting local recording must
    /// not ask for API-key access as an incidental side effect.
    pub fn new_without_keychain() -> Self {
        let snapshot = Config::load_startup_runtime_snapshot(false);
        Self::with_runtime_settings(snapshot, "RecordingController::new_without_keychain")
    }

    /// Shared constructor behind both public entry points.
    ///
    /// Outside tests this also kicks off a background STT prewarm. The product
    /// invariant it protects is that **recording readiness is not engine
    /// readiness**: capture must start the instant the user presses record, so
    /// the prewarm runs on its own thread and a failure is a warning, never a
    /// blocked recording.
    fn with_runtime_settings(
        runtime_settings: RuntimeSettingsSnapshot,
        recorder_context: &str,
    ) -> Self {
        let resources = Self::acquire_startup_resources(recorder_context, &runtime_settings);
        Self::from_startup_inputs(runtime_settings, resources, Config::config_dir())
    }

    fn acquire_startup_resources(
        recorder_context: &str,
        runtime_settings: &RuntimeSettingsSnapshot,
    ) -> ControllerStartupResources {
        crate::config::note_startup_acquisition("controller startup resources");
        let recorder = Self::init_streaming_recorder(recorder_context);

        if !cfg!(test) {
            crate::config::note_startup_acquisition("model discovery");
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

            // Best-effort BACKGROUND prewarm — never block recording readiness.
            //
            // Product invariant: recording readiness is NOT engine readiness.
            // Audio capture must start the moment the user presses record; the
            // selected live refiner warms from this same snapshot without
            // blocking the capture owner.
            // A failed prewarm is a warning, not an app or recording failure.
            // The idle-unload reaper (commit 2b8bb1f) may legitimately drop the
            // engine later and the next call reloads it — pinning it here would
            // undo that GPU/host-memory reclaim.
            //
            // A resident Whisper model says nothing about Apple readiness.
            // Prepare the primary and the selected in-process refiner, not
            // merely the router's primary engine.
            crate::config::note_startup_acquisition("STT prewarm thread");
            let runtime_settings = runtime_settings.clone();
            std::thread::Builder::new()
                .name("stt-prewarm".into())
                .spawn(move || {
                    if let Err(e) = crate::stt::prewarm_active_engine(&runtime_settings) {
                        warn!(
                            "STT background prewarm failed (will lazy-load on first use): {}",
                            e
                        );
                    }
                })
                .ok();
        }

        ControllerStartupResources { recorder }
    }

    /// One real state/lifecycle assembly, with an explicit context root.
    /// Callers that already acquired resources never fall back to host acquisition.
    pub fn from_startup_inputs(
        runtime_settings: RuntimeSettingsSnapshot,
        resources: ControllerStartupResources,
        data_root: impl AsRef<std::path::Path>,
    ) -> Self {
        let config = runtime_settings.values();
        if !cfg!(test) {
            codescribe_core::state::history::audio_retention::start_maintenance();
        }
        info!(
            "Initializing RecordingController (hold_delay={}ms, beep={}, language={:?})",
            config.hold_start_delay_ms, config.beep_on_start, config.whisper_language
        );
        let ControllerStartupResources { recorder } = resources;
        let runtime_settings = RwLock::new(Arc::new(runtime_settings));
        if recorder.is_none() {
            warn!("Recorder unavailable at controller init; voice capture is disabled");
        }
        let (event_broadcast, _) = broadcast::channel::<IpcEvent>(256);

        Self {
            runtime_settings,
            runtime_settings_refresh_pending: AtomicBool::new(false),
            state: Arc::new(RwLock::new(State::Idle)),
            recorder: Arc::new(Mutex::new(recorder)),
            assistive_mode: Arc::new(RwLock::new(false)),
            hold_mode: Arc::new(RwLock::new(HoldMode::Raw)),
            force_raw_mode: Arc::new(RwLock::new(false)),
            force_ai_mode: Arc::new(RwLock::new(false)),
            session_id: Arc::new(RwLock::new(None)),
            active_transcript_bus: Arc::new(RwLock::new(None)),
            active_presentation: Arc::new(RwLock::new(None)),
            active_stop_receipt: Arc::new(RwLock::new(Arc::new(std::sync::Mutex::new(
                LateFinalReceipt::default(),
            )))),
            max_consultation: Mutex::new(None),
            max_approvals: Arc::default(),
            max_consultation_events: broadcast::channel(256).0,
            hold_start_task: Arc::new(Mutex::new(None)),
            hold_start_generation: Arc::new(AtomicU64::new(0)),
            start_transition_in_flight: Arc::new(AtomicBool::new(false)),
            serial_lock: Arc::new(Mutex::new(())),
            shutdown_requested: Arc::new(AtomicBool::new(false)),
            capture_settlement: std::sync::Mutex::new(None),
            closed_capture_tails: std::sync::Mutex::new(Vec::new()),
            live_archive: Arc::new(std::sync::Mutex::new(None)),
            #[cfg(test)]
            settlement_observer: std::sync::Mutex::new(None),
            #[cfg(test)]
            test_start_admission: std::sync::Mutex::new(None),
            delivery_disposition: Arc::new(RwLock::new(TranscriptDelivery::Unattempted)),
            delivery_tagger: Arc::default(),
            composer_delivery_payload: Arc::new(RwLock::new(None)),
            vad_triggered: Arc::new(AtomicBool::new(false)),
            assistive_loop_active: Arc::new(AtomicBool::new(false)),
            toggle_user_has_text: Arc::new(AtomicBool::new(false)),
            toggle_assistant_has_text: Arc::new(AtomicBool::new(false)),
            assistive_context: Arc::new(RwLock::new(None)),
            pending_assistive_context: Arc::new(RwLock::new(None)),
            context_bucket: Arc::new(Mutex::new(ContextBucket::for_codescribe_data_dir(
                data_root,
            ))),
            pre_overlay_frontmost_app: Arc::new(RwLock::new(None)),
            delivered_take: Arc::new(Mutex::new(None)),
            assistive_silence: std::sync::Mutex::new(None),
            last_segment_audio_offset: Arc::new(AtomicUsize::new(0)),
            // Conversation mode (lazy init)
            conversation_engine: Arc::new(Mutex::new(None)),
            audio_player: Arc::new(Mutex::new(None)),
            conversation_stop_flag: Arc::new(AtomicBool::new(false)),
            conversation_generation: Arc::new(AtomicU64::new(0)),
            conversation_task: Arc::new(Mutex::new(None)),
            event_broadcast,
            agent_channels: Mutex::new(std::collections::HashMap::new()),
            agent_channel_cohort: Arc::default(),
            channel_guards_started: AtomicBool::new(false),
        }
    }

    /// Get current state
    pub async fn current_state(&self) -> State {
        *self.state.read().await
    }

    /// Resolve the controller's retained Max consultation without another
    /// recorder or a process-global conversation selection.
    ///
    /// Max exists only on a usable Agent lane. Without one this refuses with
    /// the Agent's actionable reason; it never falls back to the formatting
    /// lane or Apple. A retained consultation stays selected and resumes once
    /// the Agent is configured again.
    ///
    /// Only an explicit request reaches Max: an overlay/history Format action
    /// or an Agent-window turn derives a one-operation Max snapshot. A capture
    /// take carries the sealed automatic policy, so this returns `None` there.
    async fn selected_max_consultation(
        &self,
        settings: &RuntimeSettingsSnapshot,
    ) -> Result<Option<Arc<crate::agent::max_consultation::MaxConsultation>>> {
        if settings.formatting_policy() != codescribe_core::config::FormattingPolicy::Max {
            return Ok(None);
        }
        if let Some(reason) = crate::agent::max_unavailable_reason(settings) {
            anyhow::bail!("{reason}");
        }
        let mut selected = self.max_consultation.lock().await;
        if selected.is_none() {
            let gateway = codescribe_core::agent::ThreadDeliveryGateway::new()?;
            let consultation_id = gateway.selected_max_consultation_id()?;
            let approvals = Arc::clone(&self.max_approvals);
            let approval_handler: codescribe_core::agent::ToolApprovalHandler =
                Arc::new(move |request| approvals.begin(request));
            let events = self.max_consultation_events.clone();
            let consultation = crate::agent::max_consultation::MaxConsultation::start_deferred(
                consultation_id,
                settings,
                Box::new(|| Arc::new(crate::agent::tools::configured_registry())),
                Some(approval_handler),
                gateway,
                Arc::new(move |consultation, turn, event| {
                    if let codescribe_core::agent::AgentUiEvent::Error(error) = &event {
                        warn!(%error, "Max consultation failed");
                    }
                    // No receiver is a normal state (nothing renders the turn).
                    let _ = events.send(MaxConsultationEvent {
                        consultation_id: consultation.to_string(),
                        turn_id: turn.to_string(),
                        event,
                    });
                }),
                codescribe_core::config::agent_turn_lease_path(),
            );
            *selected = Some(Arc::new(consultation));
        }
        Ok(selected.clone())
    }

    /// Prepare the selected Max conversation before the first formatting turn.
    ///
    /// Max is manual-only (invoked explicitly from the Format menu or an Agent
    /// conversation turn), so startup readiness evaluates the automatic take
    /// policy (where a persisted `max` is clamped to `Smart`) and never touches
    /// `max_consultation` or `MaxConsultation::prepare` at launch.
    pub async fn prepare_max_consultation(
        &self,
    ) -> Result<Option<codescribe_core::agent::consultation::ConsultationPreparation>> {
        let settings = automatic_take_settings(self.runtime_settings_arc().await);
        if let Some(consultation) = self.selected_max_consultation(&settings).await? {
            return consultation.prepare(&settings).await.map(Some);
        }
        Ok(None)
    }

    /// Observe Max owner events as they happen. Subscribe before enqueueing a
    /// turn you intend to render; the owner never replays earlier deltas.
    pub fn subscribe_max_consultation_events(&self) -> broadcast::Receiver<MaxConsultationEvent> {
        self.max_consultation_events.subscribe()
    }

    /// The broker holding this controller's pending Max tool approvals. Exact
    /// (session, thread, call) matching inside the broker is the safety; it
    /// lets the Agent window answer a Max card without an async controller hop.
    pub fn max_approval_broker(&self) -> Arc<codescribe_core::agent::ApprovalBroker> {
        Arc::clone(&self.max_approvals)
    }

    /// Enter a typed Agent-window turn into the selected Max consultation: same
    /// owner, FIFO, Agent lane, prompt and approval broker as a spoken
    /// turn. The consultation is Max by identity, so the turn runs under Max
    /// regardless of the dictation formatting level currently selected. Only
    /// the selected consultation accepts new turns; an older one is history.
    ///
    /// `turn_id` is the caller's durable acceptance identity. The owner's
    /// journal refuses a repeat of it and every event of the turn carries it,
    /// so a renderer binds to exactly this turn and a replay never runs twice.
    pub async fn enqueue_max_consultation_text_turn(
        &self,
        thread_id: &str,
        turn_id: String,
        text: String,
        attachments: Vec<codescribe_core::agent::ImageAttachment>,
    ) -> Result<
        tokio::sync::oneshot::Receiver<
            Result<codescribe_core::agent::consultation::ConsultationAnswer>,
        >,
    > {
        let settings = self
            .runtime_settings_arc()
            .await
            .with_formatting_level(codescribe_core::config::FormattingPolicy::Max);
        // Max answers on the Agent lane, so the Agent model decides images.
        let lane = settings.llm_lanes().assistive();
        anyhow::ensure!(
            attachments.is_empty() || lane.supports_vision(lane.model()),
            "The selected Max model does not support images"
        );
        let consultation = self
            .selected_max_consultation(&settings)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Max consultation is not selected"))?;
        anyhow::ensure!(
            consultation.id() == thread_id,
            "This Max consultation is no longer active. Continue the current consultation, or start a new one in Settings → Creator."
        );
        consultation.enqueue(turn_id, text, attachments, &settings)
    }

    /// Subscribe to invalidations; the broker remains the pending-state owner.
    pub fn subscribe_max_approval_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.max_approvals.subscribe_changes()
    }

    /// Read pending Max cards without constructing a session or touching the mic.
    pub async fn pending_max_tool_approvals(
        &self,
    ) -> Vec<codescribe_core::agent::ToolApprovalRequest> {
        let selected = self.max_consultation.lock().await;
        selected
            .as_ref()
            .map(|consultation| self.max_approvals.pending_for_thread(consultation.id()))
            .unwrap_or_default()
    }

    /// Resolve only a currently pending call belonging to the selected Max owner.
    /// Exact-key matching remains inside the shared broker.
    pub async fn resolve_max_tool_approval(
        &self,
        session_id: &str,
        thread_id: &str,
        call_id: &str,
        approved: bool,
        remember: bool,
    ) -> bool {
        let selected = self.max_consultation.lock().await;
        if selected
            .as_ref()
            .is_none_or(|consultation| consultation.id() != thread_id)
        {
            return false;
        }
        self.max_approvals
            .resolve(session_id, thread_id, call_id, approved, remember)
    }

    /// Explicit conversation reset, preserving the prior thread and any
    /// unresolved effects. Never interrupt recording or accepted Agent work.
    pub async fn begin_new_max_consultation(&self) -> Result<String> {
        let _serial = self.serial_lock.lock().await;
        anyhow::ensure!(
            self.current_state().await == State::Idle,
            "cannot reset Max while recording or processing"
        );
        anyhow::ensure!(
            self.hold_start_task
                .lock()
                .await
                .as_ref()
                .is_none_or(|task| task.is_finished()),
            "cannot reset Max while a hold capture is scheduled"
        );
        let mut selected = self.max_consultation.lock().await;
        let gateway = codescribe_core::agent::ThreadDeliveryGateway::new()?;
        let expected = gateway.selected_max_consultation_id()?;
        if let Some(consultation) = selected.take()
            && let Err(error) = consultation.close_if_idle().await
        {
            *selected = Some(consultation);
            return Err(error);
        }
        gateway.begin_new_max_consultation(&expected)
    }

    /// Commit an overlay edit through the retained terminal reducer. The
    /// session/revision pair is checked in Rust; text never selects authority.
    pub async fn apply_user_revision_from_overlay(
        &self,
        session_id: String,
        source_revision: u64,
        rendered_text: String,
    ) -> Result<UserRevisionCommit> {
        self.apply_document_revision_from_overlay(
            session_id,
            source_revision,
            rendered_text,
            DocumentRevisionProvenance::UserEdit,
        )
        .await
    }

    pub async fn apply_retranscribe_revision_from_overlay(
        &self,
        session_id: String,
        source_revision: u64,
        rendered_text: String,
    ) -> Result<UserRevisionCommit> {
        self.apply_document_revision_from_overlay(
            session_id,
            source_revision,
            rendered_text,
            DocumentRevisionProvenance::Retranscribe,
        )
        .await
    }

    async fn apply_document_revision_from_overlay(
        &self,
        session_id: String,
        source_revision: u64,
        rendered_text: String,
        provenance: DocumentRevisionProvenance,
    ) -> Result<UserRevisionCommit> {
        if self.current_state().await != State::Idle {
            return Err(anyhow::anyhow!(
                "transcript revision refused while recording is active"
            ));
        }
        let presentation = self
            .active_presentation
            .read()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no terminal transcript revision authority"))?;
        let receipt = presentation
            .apply_user_revision(UserRevisionIntent {
                session_id,
                source_revision,
                rendered_text,
                provenance,
            })
            .map_err(anyhow::Error::new)?;
        self.sync_live_archive(&presentation, &receipt.session_id)
            .await;
        Ok(receipt)
    }

    /// Undo, Redo or a version pick on the retained take: the reducer
    /// re-selects an accepted step under the same session/revision CAS. No
    /// transcription or formatting runs again.
    pub async fn navigate_document_from_overlay(
        &self,
        session_id: String,
        source_revision: u64,
        step: usize,
    ) -> Result<UserRevisionCommit> {
        if self.current_state().await != State::Idle {
            return Err(anyhow::anyhow!(
                "transcript navigation refused while recording is active"
            ));
        }
        let presentation = self
            .active_presentation
            .read()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no terminal transcript revision authority"))?;
        let receipt = presentation
            .navigate_document(session_id, source_revision, step)
            .map_err(anyhow::Error::new)?;
        self.sync_live_archive(&presentation, &receipt.session_id)
            .await;
        Ok(receipt)
    }

    /// Why the versions of `session_id` are no longer saved to its archived
    /// transcript, when the mirror stopped: a code the overlay words itself.
    pub fn live_archive_refusal(&self, session_id: &str) -> Option<ArchiveRefusal> {
        self.live_archive
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .filter(|mirror| mirror.session_id() == session_id)
            .and_then(|mirror| mirror.diverged())
    }

    /// Accepted steps of the retained take with the selected one and the
    /// revision a navigation names. No retained take reads as no history.
    pub async fn document_versions(
        &self,
        session_id: &str,
    ) -> (u64, crate::presentation::emitter::DocumentTimeline) {
        match self.active_presentation.read().await.clone() {
            Some(presentation) => presentation.document_timeline(session_id),
            None => (0, Default::default()),
        }
    }

    /// Link the take just archived at Stop to its live history and write
    /// what the history already holds. A take without spoken text has no
    /// archived transcript and keeps no link; a new link replaces the last.
    async fn link_live_archive(
        &self,
        session_id: Option<&str>,
        transcript: Option<std::path::PathBuf>,
    ) {
        let (Some(session_id), Some(transcript)) = (session_id, transcript) else {
            return;
        };
        // The toggle stop marks the live slot `{uuid}:stopping`. The reducer
        // and the mirror know the take by its Bus uuid only.
        let session_id = session_id
            .strip_suffix(STOPPING_SUFFIX)
            .unwrap_or(session_id);
        let Some(presentation) = self.active_presentation.read().await.clone() else {
            warn!("archived take has no live document history to keep");
            return;
        };
        // The archived bytes are the version selected at Stop. A take with no
        // operation yet has no steps: the reducer anchors its committed
        // document as step 0 on the first operation, and that step is the
        // archived original. Mapped once, by position, never by text.
        let (_, timeline) = presentation.document_timeline(session_id);
        let archived_step = if timeline.steps().is_empty() {
            0
        } else {
            timeline.cursor()
        };
        *self
            .live_archive
            .lock()
            .unwrap_or_else(|error| error.into_inner()) =
            Some(live_archive::LiveArchiveMirror::link(
                session_id.to_string(),
                transcript,
                archived_step,
            ));
        self.sync_live_archive(&presentation, session_id).await;
    }

    /// Admit the live history of `session_id` to its archived transcript.
    /// Runs after every accepted operation and cursor move. The timeline is
    /// read under the mirror lock, so syncs apply in reducer order. A refusal
    /// stops the mirror and is reported; the live take is never undone.
    async fn sync_live_archive(&self, presentation: &Arc<PresentationEmitter>, session_id: &str) {
        let mirror = Arc::clone(&self.live_archive);
        let presentation = Arc::clone(presentation);
        let session_id = session_id.to_string();
        let synced = tokio::task::spawn_blocking(move || {
            let mut mirror = mirror.lock().unwrap_or_else(|error| error.into_inner());
            let Some(mirror) = mirror
                .as_mut()
                .filter(|mirror| mirror.session_id() == session_id)
            else {
                return Ok(());
            };
            let (_, timeline) = presentation.document_timeline(&session_id);
            mirror
                .sync(timeline.steps(), timeline.cursor())
                .with_context(|| {
                    format!(
                        "versions of this take are no longer saved to {}",
                        mirror.transcript().display()
                    )
                })
        })
        .await;
        match synced {
            Ok(Ok(())) => {}
            Ok(Err(error)) => error!("{error:#}"),
            Err(error) => error!(%error, "live history archive worker failed"),
        }
    }

    /// Format the exact retained terminal document through the production
    /// postprocess lane, then commit only an applied result through the same
    /// ledger CAS + projection corridor as an explicit user edit.
    pub async fn apply_formatter_revision_from_overlay(
        &self,
        session_id: String,
        source_revision: u64,
        requested_level: Option<FormattingPolicy>,
    ) -> Result<UserRevisionCommit> {
        if self.current_state().await != State::Idle {
            return Err(anyhow::anyhow!(
                "formatter revision refused while recording is active"
            ));
        }
        let presentation = self
            .active_presentation
            .read()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no terminal transcript revision authority"))?;
        let source = presentation
            .terminal_revision_source(&session_id, source_revision)
            .map_err(anyhow::Error::new)?;
        let runtime_settings = self.formatter_revision_settings(requested_level).await;
        let (format_level, level_source) =
            formatter_revision_level(requested_level, runtime_settings.as_ref())?;
        let language = runtime_settings.values().whisper_language;
        let consultation = self
            .selected_max_consultation(runtime_settings.as_ref())
            .await?;
        let turn_id = format!("{session_id}:revision:{source_revision}");
        let result = match &consultation {
            Some(consultation) => match consultation.prepare(runtime_settings.as_ref()).await {
                Ok(_) => {
                    format_text_with_status_for_policy(
                        &source,
                        language.whisper_hint(),
                        runtime_settings.as_ref(),
                        Some(codescribe_core::ai_formatting::FormattingConsultation {
                            agent: consultation.as_ref(),
                            turn_id: &turn_id,
                        }),
                    )
                    .await
                }
                Err(error) => {
                    warn!(%error, "Max consultation preparation failed on explicit Format");
                    codescribe_core::llm::ai_formatting::AiFormatResult {
                        text: source.clone(),
                        reasoning_text: None,
                        status: codescribe_core::llm::ai_formatting::AiFormatStatus::Failed,
                    }
                }
            },
            None => {
                format_text_with_status_for_policy(
                    &source,
                    language.whisper_hint(),
                    runtime_settings.as_ref(),
                    None,
                )
                .await
            }
        };
        let _serial_guard = self.serial_lock.lock().await;
        if self.current_state().await != State::Idle {
            return Err(anyhow::anyhow!(
                "formatter result refused because a recording became active"
            ));
        }
        let current_presentation = self
            .active_presentation
            .read()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("terminal transcript authority changed"))?;
        if !Arc::ptr_eq(&presentation, &current_presentation) {
            return Err(anyhow::anyhow!("terminal transcript authority changed"));
        }
        let receipt = presentation
            .apply_formatter_revision(session_id, source_revision, format_level, result)
            .map_err(anyhow::Error::new)?;
        log_formatter_revision(
            &receipt,
            format_level,
            level_source,
            runtime_settings.as_ref(),
        );
        self.sync_live_archive(&presentation, &receipt.session_id)
            .await;
        Ok(receipt)
    }

    /// Format one revision of an archived transcript through the same settings
    /// generation, level resolution, Max consultation and production formatter
    /// as [`Self::apply_formatter_revision_from_overlay`].
    ///
    /// The archive path and its revision are the compare-and-swap boundary,
    /// exactly as session and revision are for a live take: the source text is
    /// read here from the history owner, never accepted from Swift, and an
    /// applied result is committed to that archive's revision chain only. The
    /// reducer, the Bus and the latest take are not touched, so nothing is
    /// re-delivered and a later take can never receive this result.
    pub async fn format_archived_transcript(
        &self,
        archive_path: String,
        source_revision: u64,
        requested_level: Option<FormattingPolicy>,
    ) -> Result<ArchivedFormatReceipt> {
        use codescribe_core::state::history;
        anyhow::ensure!(
            self.current_state().await == State::Idle,
            "archived transcript formatting refused while recording is active"
        );
        let read_path = archive_path.clone();
        let document = tokio::task::spawn_blocking(move || {
            // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- history::admit_archived_transcript canonicalizes the path and admits only a .txt under transcriptions_base_dir() before any read.
            history::read_archived_document(std::path::Path::new(&read_path))
        })
        .await??;
        anyhow::ensure!(
            document.head_revision() == source_revision,
            "stale archived transcript revision: current {}, requested {}",
            document.head_revision(),
            source_revision
        );
        let source = document.head_text().to_string();
        anyhow::ensure!(
            !source.trim().is_empty(),
            "archived transcript has no text to format"
        );
        let runtime_settings = self.formatter_revision_settings(requested_level).await;
        let (format_level, level_source) =
            formatter_revision_level(requested_level, runtime_settings.as_ref())?;
        let language = runtime_settings.values().whisper_language;
        let consultation = self
            .selected_max_consultation(runtime_settings.as_ref())
            .await?;
        // The archive and its revision name the source, as session and revision
        // do for a live take; the uuid keeps a retry from reusing a Max turn.
        let turn_id = format!(
            "archive:{archive_path}:revision:{source_revision}:{}",
            Uuid::new_v4()
        );
        let result = match &consultation {
            Some(consultation) => match consultation.prepare(runtime_settings.as_ref()).await {
                Ok(_) => {
                    format_text_with_status_for_policy(
                        &source,
                        language.whisper_hint(),
                        runtime_settings.as_ref(),
                        Some(codescribe_core::ai_formatting::FormattingConsultation {
                            agent: consultation.as_ref(),
                            turn_id: &turn_id,
                        }),
                    )
                    .await
                }
                Err(error) => {
                    warn!(%error, "Max consultation preparation failed on archived Format");
                    codescribe_core::llm::ai_formatting::AiFormatResult {
                        text: source.clone(),
                        reasoning_text: None,
                        status: codescribe_core::llm::ai_formatting::AiFormatStatus::Failed,
                    }
                }
            },
            None => {
                format_text_with_status_for_policy(
                    &source,
                    language.whisper_hint(),
                    runtime_settings.as_ref(),
                    None,
                )
                .await
            }
        };
        let outcome = ArchivedFormatOutcome::from_result(&source, result);
        let revision = match &outcome {
            ArchivedFormatOutcome::Applied(text) => {
                let text = text.clone();
                let level = format_level.as_str().to_string();
                Some(
                    tokio::task::spawn_blocking(move || {
                        history::commit_archived_revision(
                            // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- history::admit_archived_transcript canonicalizes the path and admits only a .txt under transcriptions_base_dir() before any write.
                            std::path::Path::new(&archive_path),
                            source_revision,
                            &text,
                            history::ArchiveRevisionProvenance::Formatter,
                            Some(level),
                        )
                    })
                    .await??,
                )
            }
            _ => None,
        };
        let prompt = runtime_settings
            .ai_execution()
            .formatter()
            .formatting_prompt();
        info!(
            format_level = format_level.as_str(),
            source = level_source,
            outcome = outcome.label(),
            source_revision,
            revision = revision.as_ref().map(|revision| revision.revision),
            prompt_source = prompt.map(|p| p.source().as_str()).unwrap_or("none"),
            prompt_sha256 = prompt.map(|p| p.composed_sha256()).unwrap_or("none"),
            settings_sha256 = runtime_settings.digest().as_str(),
            "archived transcript formatting settled"
        );
        Ok(ArchivedFormatReceipt { outcome, revision })
    }

    async fn formatter_revision_settings(
        &self,
        requested_level: Option<FormattingPolicy>,
    ) -> Arc<RuntimeSettingsSnapshot> {
        if self.runtime_settings_refresh_pending.load(Ordering::SeqCst) {
            let _serial_guard = self.serial_lock.lock().await;
            if self.current_state().await == State::Idle {
                let _ = self.refresh_pending_runtime_settings_locked().await;
            }
        }
        let settings = self.runtime_settings_arc().await;
        match requested_level {
            Some(level) => Arc::new(settings.with_formatting_level(level)),
            None => settings,
        }
    }

    /// Forward one host sleep/wake boundary to the active recording session.
    ///
    /// This never creates a recorder or starts an engine. When capture is not
    /// active it is a normal no-op; otherwise the per-recording lifecycle
    /// channel wakes the session loop and degrades Layer 1 fail-closed.
    pub async fn note_sleep_wake(&self) -> bool {
        self.recorder
            .lock()
            .await
            .as_ref()
            .is_some_and(StreamingRecorder::note_sleep_wake)
    }

    /// Subscribe to the controller's IPC event stream. Each subscriber gets its
    /// own receiver; a slow consumer lags rather than stalling the producer.
    pub fn subscribe_events(&self) -> broadcast::Receiver<IpcEvent> {
        self.event_broadcast.subscribe()
    }

    /// Transition state and broadcast the change (see
    /// [`Self::set_state_with_broadcast`] for the invariants).
    async fn set_state(&self, new_state: State) {
        Self::set_state_with_broadcast(&self.state, &self.event_broadcast, new_state).await;
    }

    /// Flip the cursor badge to "processing" while a stop pipeline runs.
    async fn show_processing_badge_if_enabled(&self) {
        let hold_indicator = self.get_config().await.hold_indicator;
        publish_recording_indicator(BadgeMode::Processing, hold_indicator);
    }

    /// Character offset the live transcript has reached, used to anchor a
    /// context marker at the point in the dictation where the user pressed the
    /// combo. Zero outside an active recording, and zero when no recorder or
    /// buffer exists — an unanchored marker is better than a wrong anchor.
    async fn current_live_transcript_position(&self, state: State) -> usize {
        if !matches!(state, State::RecHold | State::RecToggle) {
            return 0;
        }
        let transcript_buffer = {
            let recorder = self.recorder.lock().await;
            recorder
                .as_ref()
                .map(StreamingRecorder::transcript_buffer_handle)
        };
        let Some(transcript_buffer) = transcript_buffer else {
            return 0;
        };
        transcript_buffer.lock().await.chars().count()
    }

    /// Capture selection + frontmost app for an assistive combo pressed mid
    /// session, and drop it into the context bucket as a marked item.
    ///
    /// Ordering is deliberate: the OS capture is launched first and the
    /// transcript position is read while it is already in flight, because
    /// focus and caret state can vanish the moment the combo changes the UI.
    /// A bucket failure degrades to the plain selection context rather than
    /// losing the capture entirely.
    async fn capture_assistive_combo_context(
        &self,
        state: State,
        prior_frontmost_app: Option<String>,
        allow_clipboard_image: bool,
    ) -> AssistiveContext {
        // Start selection capture first: focus/caret state may disappear as soon
        // as the combo changes the UI. Snapshot the live transcript position
        // concurrently while the OS capture is already in flight.
        let capture_task = tokio::task::spawn_blocking(move || {
            capture_assistive_context_with_image_with_prior_frontmost(prior_frontmost_app)
        });
        let position = self.current_live_transcript_position(state).await;
        let captured_payload = capture_task.await.unwrap_or_default();
        let fallback = captured_payload.context.clone();
        // The OS capture retains a selected image before clipboard restoration.
        // Only assistive combos may also read an image already on the clipboard.
        let clipboard_image = if allow_clipboard_image && captured_payload.image_png.is_none() {
            tokio::task::spawn_blocking(clipboard::get_image_png_best_effort)
                .await
                .unwrap_or(None)
        } else {
            None
        };
        let mut bucket = self.context_bucket.lock().await;
        let result = Self::record_captured_context(
            &mut bucket,
            position,
            allow_clipboard_image,
            || captured_payload,
            || clipboard_image,
        );
        drop(bucket);

        match result {
            Ok((context, events)) => {
                let event_sink = {
                    let recorder = self.recorder.lock().await;
                    recorder
                        .as_ref()
                        .and_then(StreamingRecorder::event_sink_handle)
                };
                if let Some(event_sink) = event_sink {
                    for event in &events {
                        event_sink.on_event(event);
                    }
                } else if !events.is_empty() {
                    warn!("Context markers captured without an active presentation reducer");
                }
                context
            }
            Err(error) => {
                warn!("Context bucket capture failed; retaining legacy selection context: {error}");
                fallback
            }
        }
    }

    /// Store only captures admitted by this route and return their reducer events.
    /// Source closures let tests prove the clipboard policy without a pasteboard.
    fn record_captured_context<C, P>(
        bucket: &mut ContextBucket,
        position: usize,
        allow_clipboard_image: bool,
        selection_source: C,
        clipboard_source: P,
    ) -> Result<(AssistiveContext, Vec<EngineEvent>)>
    where
        C: FnOnce() -> CapturedAssistiveContext,
        P: FnOnce() -> Option<Vec<u8>>,
    {
        let captured = selection_source();
        let mut context = captured.context;
        let image_png = captured.image_png.or_else(|| {
            if allow_clipboard_image {
                clipboard_source()
            } else {
                None
            }
        });
        let mut events = Vec::new();
        if let Some(selected_text) = context.selected_text.take()
            && let Some(marker) = bucket.add_selection(position, selected_text)?
        {
            events.push(EngineEvent::ContextMarker {
                position: marker.position,
                label: format!("{{{}}}", marker.label),
            });
        }
        if let Some(png) = image_png
            && let Some(marker) = bucket.add_image_png(&png)?
        {
            events.push(EngineEvent::ContextMarker {
                position,
                label: format!("{{{}}}", marker.label),
            });
        }
        Ok((context, events))
    }

    /// Attach the current OS selection as `{selection_N}` during an in-flight
    /// hold. Destination, overlay visibility, and Agent UI stay unchanged.
    pub async fn attach_hold_selection(&self) -> Result<()> {
        let current_state = self.current_state().await;
        let pending_hold = self.hold_start_task.lock().await.is_some();
        if !matches!(current_state, State::RecHold | State::RecToggle) && !pending_hold {
            debug!("attach_hold_selection ignored: no in-flight hold");
            return Ok(());
        }

        let prior_frontmost_app = self.pre_overlay_frontmost_app.read().await.clone();
        let _ctx = self
            .capture_assistive_combo_context(current_state, prior_frontmost_app, false)
            .await;
        Ok(())
    }

    /// Deliver the overlay's current transcript with the context captured at
    /// trigger time. Taking the context makes delivery one-shot.
    pub async fn deliver_pending_assistive_transcript(&self, transcript: String) -> Result<bool> {
        let runtime_settings = self.runtime_settings_arc().await;
        self.deliver_pending_assistive_transcript_with(
            transcript,
            move |wire, language, max_tokens, persona| {
                Box::pin(send_assistive_with_agent_runtime_lane(
                    runtime_settings,
                    wire,
                    language,
                    max_tokens,
                    persona,
                ))
                    as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
            },
        )
        .await
    }

    /// Production body with an injectable send adapter. The harness executes
    /// this exact instrumentation boundary with a fake adapter — moving the
    /// timer off the real send breaks the harness, not just a formatter test.
    pub(crate) async fn deliver_pending_assistive_transcript_with<F>(
        &self,
        transcript: String,
        send: F,
    ) -> Result<bool>
    where
        F: FnOnce(
            String,
            crate::config::Language,
            i32,
            bool,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>,
    {
        let delivery_started = std::time::Instant::now();
        if transcript.trim().is_empty() {
            info!(
                elapsed_secs = delivery_started.elapsed().as_secs_f64(),
                "assistive delivery skipped: empty transcript"
            );
            return Ok(false);
        }
        let to_agent = resolve_delivery_route(
            DeliveryIntent::OverlayToAgent,
            overlay_insert_facts(true, false),
        );
        info!(
            "{}",
            format_delivery_route_line(DeliveryIntent::OverlayToAgent, to_agent, None,)
        );
        // Dictation/formatting sessions never run the assistive pipeline branch
        // that arms `pending_assistive_context`, so the overlay's explicit
        // "To Agent" used to fail closed behind a live button (review P0-03).
        // The session trigger context (frontmost app, captured at every session
        // start) is truthful for an explicit send; taking it keeps delivery
        // one-shot either way.
        let _ = self.pending_assistive_context.write().await.take();
        let _ = self.assistive_context.write().await.take();
        let config = self.get_config().await;
        let delivery_text = self
            .delivery_tagger
            .render(&transcript, &config, Some("agent"));
        {
            let mut bucket = self.context_bucket.lock().await;
            match bucket.archive_and_reset("assistive-delivery") {
                Ok(Some(dir)) => info!("Context bucket archived: {}", dir.display()),
                Ok(None) => {}
                Err(err) => warn!("Context bucket archive failed (items kept): {err:#}"),
            }
        }
        send(
            delivery_text,
            config.whisper_language,
            config.ai_assistive_max_tokens,
            true,
        )
        .await;
        info!(
            elapsed_secs = delivery_started.elapsed().as_secs_f64(),
            "assistive delivery completed"
        );
        Ok(true)
    }

    /// Send a transcript reopened from history to Agent on an explicit click.
    ///
    /// The text is the archive's accepted revision, chosen by the overlay for
    /// one named archive. Unlike the live take's send this never takes
    /// `pending_assistive_context`, `assistive_context` or the context bucket:
    /// those belong to whichever take is live or pending now, and an archive
    /// send must leave them exactly as they were. The route resolution, the
    /// delivery tagger and the Agent runtime lane (current selected thread,
    /// configuration and permissions) are the same production seams.
    pub async fn deliver_archived_transcript_to_agent(&self, transcript: String) -> Result<bool> {
        let runtime_settings = self.runtime_settings_arc().await;
        self.deliver_archived_transcript_to_agent_with(
            transcript,
            move |wire, language, max_tokens, persona| {
                Box::pin(send_assistive_with_agent_runtime_lane(
                    runtime_settings,
                    wire,
                    language,
                    max_tokens,
                    persona,
                ))
                    as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
            },
        )
        .await
    }

    /// Archive-send body with an injectable send adapter, the twin of
    /// [`Self::deliver_pending_assistive_transcript_with`] minus every live
    /// context take.
    pub(crate) async fn deliver_archived_transcript_to_agent_with<F>(
        &self,
        transcript: String,
        send: F,
    ) -> Result<bool>
    where
        F: FnOnce(
            String,
            crate::config::Language,
            i32,
            bool,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>,
    {
        let delivery_started = std::time::Instant::now();
        if transcript.trim().is_empty() {
            info!("archived transcript delivery skipped: empty transcript");
            return Ok(false);
        }
        let to_agent = resolve_delivery_route(
            DeliveryIntent::OverlayToAgent,
            overlay_insert_facts(true, false),
        );
        info!(
            "{}",
            format_delivery_route_line(DeliveryIntent::OverlayToAgent, to_agent, None,)
        );
        let config = self.get_config().await;
        let delivery_text = self
            .delivery_tagger
            .render(&transcript, &config, Some("agent"));
        send(
            delivery_text,
            config.whisper_language,
            config.ai_assistive_max_tokens,
            true,
        )
        .await;
        info!(
            elapsed_secs = delivery_started.elapsed().as_secs_f64(),
            "archived transcript delivery completed"
        );
        Ok(true)
    }

    /// Swap the state and broadcast the transition, on `Arc` handles so spawned
    /// tasks can drive it without borrowing the controller.
    ///
    /// The write guard is released before the broadcast, and the event fires
    /// only on a real change. Any arrival at `Idle` tears down the cursor badge,
    /// which is what makes finalize, cancel, error and no-speech all end with a
    /// clean cursor instead of each path remembering to hide it.
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

    /// Refresh persisted intent now when idle, otherwise retain an invalidation
    /// for the next take. Load under the lifecycle lock so asynchronous Settings
    /// notifications cannot install an older, preloaded generation out of order.
    ///
    /// Scheduling, starts, stops and refresh all cross `serial_lock`. A live
    /// delayed-hold task counts as active ownership; a finished stale handle is
    /// consumed so it cannot block refresh forever.
    pub async fn refresh_runtime_settings_from_disk(&self) -> Result<bool> {
        let _serial_guard = self.serial_lock.lock().await;
        self.runtime_settings_refresh_pending
            .store(true, Ordering::SeqCst);

        {
            let mut hold_task = self.hold_start_task.lock().await;
            let finished = hold_task.as_ref().map(|task| task.is_finished());
            match finished {
                Some(false) => return Ok(false),
                Some(true) => {
                    let _ = hold_task.take();
                }
                None => {}
            }
        }

        if self.current_state().await != State::Idle {
            return Ok(false);
        }
        self.refresh_pending_runtime_settings_locked().await?;
        Ok(true)
    }

    /// The next take must consume persisted intent before selecting its Arc.
    /// Call only at an idle start boundary under `serial_lock`. A refused load
    /// keeps the invalidation set and refuses that start instead of silently
    /// recording with settings that the Founder has already changed.
    async fn refresh_pending_runtime_settings_locked(&self) -> Result<()> {
        if self.runtime_settings_refresh_pending.load(Ordering::SeqCst) {
            let refreshed = Config::load_runtime_snapshot_without_keychain()
                .context("pending_settings_snapshot_refresh_failed")?;
            self.install_runtime_settings_generation(refreshed).await;
            self.runtime_settings_refresh_pending
                .store(false, Ordering::SeqCst);
        }
        Ok(())
    }

    /// Install one already-sealed generation. Callers must own `serial_lock`;
    /// keeping the single write site here preserves the one-generation fence.
    ///
    /// Max consultation is manual-only (invoked explicitly via Format or Agent
    /// conversation turns), so installing a settings generation never prepares
    /// Max in the background when `max` is persisted on disk.
    async fn install_runtime_settings_generation(&self, runtime_settings: RuntimeSettingsSnapshot) {
        let settings = Arc::new(runtime_settings);
        *self.runtime_settings.write().await = Arc::clone(&settings);
    }

    /// Reload the just-persisted calibration into the controller before the
    /// calibration call returns. The capture path already owns `serial_lock`,
    /// so a Settings readiness probe cannot interleave on the stale Arc.
    async fn refresh_runtime_settings_after_calibration_locked(&self) -> Result<()> {
        let refreshed = Config::load_runtime_snapshot_without_keychain()
            .context("calibration_snapshot_refresh_failed")?;
        self.install_runtime_settings_generation(refreshed).await;
        Ok(())
    }

    /// Borrow the current settings generation as an Arc (may differ from an
    /// in-flight take that already cloned an older generation).
    pub async fn runtime_settings_arc(&self) -> Arc<RuntimeSettingsSnapshot> {
        Arc::clone(&*self.runtime_settings.read().await)
    }

    /// Stop consumes the generation bound to its capture, including when this
    /// document joined an older shared recognizer after Settings changed.
    async fn capture_runtime_settings(&self) -> Arc<RuntimeSettingsSnapshot> {
        let bound = self
            .recorder
            .lock()
            .await
            .as_ref()
            .and_then(StreamingRecorder::bound_runtime_settings);
        match bound {
            Some(settings) => automatic_take_settings(settings),
            None => automatic_take_settings(self.runtime_settings_arc().await),
        }
    }

    /// Snapshot of current controller configuration
    pub async fn get_config(&self) -> Config {
        self.runtime_settings_arc().await.values().clone()
    }

    /// Admission readiness of the next product recording, decided against
    /// this controller's current settings generation. Probes the device that
    /// would open and the seal lane; opens no stream, invents no floor.
    pub async fn admission_readiness(
        &self,
    ) -> Result<admission::AdmissionGrant, admission::AdmissionBlocker> {
        let snapshot = self.runtime_settings_arc().await;
        tokio::task::spawn_blocking(move || admission::evaluate_live_admission_arc(&snapshot))
            .await
            .unwrap_or_else(|join| {
                Err(admission::AdmissionBlocker::CaptureDeviceUnavailable {
                    reason: format!("admission probe panicked: {join}"),
                })
            })
    }

    /// Surface a refused start as typed presentation truth and settle transient
    /// recording chrome. The refusal happened before any microphone opened, so
    /// it deliberately does not invent a transcript projection or acoustic receipt.
    fn broadcast_admission_refusal(
        event_broadcast: &broadcast::Sender<IpcEvent>,
        session_id: Option<String>,
        blocker: &admission::AdmissionBlocker,
    ) {
        let status = PresentationStatusProjection::admission_refused(
            session_id,
            blocker.code(),
            format!("{} — {}", blocker.explanation(), blocker.action()),
        );
        Self::broadcast_presentation_status(event_broadcast, &status);
        // The explanation is the presentation above. Tray Error used to stick:
        // a refused start never leaves Idle, so the hotkey bridge sees no
        // StateChange and never restores Idle. Settle the transient chrome.
        // A live Listening/Thinking take is left alone.
        settle_failed_start_chrome();
    }

    /// Read the instance test seam. Production always returns `None` and the
    /// caller runs the live probe. Under `cfg(test)`, `None` means "do not
    /// touch TCC", which is the historical default for existing controller tests.
    fn installed_start_admission(&self) -> Option<Result<(), admission::AdmissionBlocker>> {
        #[cfg(test)]
        {
            return self
                .test_start_admission
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone();
        }
        #[cfg(not(test))]
        {
            None
        }
    }

    /// Install one admission verdict for this controller. The next toggle or
    /// hold start uses it instead of the live TCC probe. Call
    /// [`Self::clear_test_start_admission`] to restore the skip default.
    #[cfg(test)]
    pub fn install_test_start_admission(&self, verdict: Result<(), admission::AdmissionBlocker>) {
        *self
            .test_start_admission
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(verdict);
    }

    /// Restore the unit-test default: start does not call the live admission probe.
    #[cfg(test)]
    pub fn clear_test_start_admission(&self) {
        *self
            .test_start_admission
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
    }

    fn broadcast_capture_transcription_warning(
        event_broadcast: &broadcast::Sender<IpcEvent>,
        blocker: &admission::AdmissionBlocker,
    ) {
        warn!(%blocker, "transcription readiness unavailable; audio admission continues");
        let _ = event_broadcast.send(IpcEvent {
            timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            payload: IpcEventPayload::Engine(EngineEventWire::Warning {
                code: "capture_transcription_unavailable".into(),
                message: format!(
                    "Audio recording remains available; transcription cannot qualify: {} {}",
                    blocker.explanation(),
                    blocker.action(),
                ),
            }),
        });
    }

    /// Publish one non-transcript status over the same controller IPC stream as
    /// transcript projections. Serialization failure is logged and never
    /// re-routed through the engine warning channel.
    fn broadcast_presentation_status(
        event_broadcast: &broadcast::Sender<IpcEvent>,
        status: &PresentationStatusProjection,
    ) {
        let json = match serde_json::to_string(status) {
            Ok(json) => json,
            Err(error) => {
                error!(%error, "presentation status serialization failed");
                return;
            }
        };
        let _ = event_broadcast.send(IpcEvent {
            timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            payload: IpcEventPayload::PresentationStatus { json },
        });
    }

    /// Guided acoustic calibration: capture `duration` of the operator speaking
    /// through the exact recorder path a take would open, derive a device
    /// profile from the capture-level receipt (ITU-T P.56 margin), persist it
    /// beside `settings.json`, and report the measured figures. Levels and
    /// counts only — the temporary WAV the recorder writes is deleted and no
    /// audio is retained. Refuses unless the controller is idle.
    pub async fn capture_energy_calibration(
        &self,
        duration: Duration,
    ) -> Result<admission::EnergyCalibrationReport> {
        let result = self.capture_energy_calibration_inner(duration).await;
        let status = match &result {
            Ok(report) => PresentationStatusProjection::calibration_succeeded(
                report.version.clone(),
                format!(
                    "{} at {} Hz is ready for recording (profile {}).",
                    report.device_name, report.sample_rate, report.version
                ),
            ),
            Err(error) => PresentationStatusProjection::calibration_failed(error.to_string()),
        };
        Self::broadcast_presentation_status(&self.event_broadcast, &status);
        result
    }

    async fn capture_energy_calibration_inner(
        &self,
        duration: Duration,
    ) -> Result<admission::EnergyCalibrationReport> {
        self.refuse_exclusive_while_agent_channel_open("energy calibration")
            .await?;
        use codescribe_core::audio::capture_receipt::{CaptureLevelAccumulator, CapturePathMeta};
        use codescribe_core::config::energy_calibration::{
            EnergyCalibrationArtifact, EnergyCalibrationProfile, GuidedCalibrationRefused,
            SOURCE_GUIDED_CAPTURE, energy_calibration_path,
        };

        let _guard = self.serial_lock.lock().await;
        let current_state = *self.state.read().await;
        if current_state != State::Idle {
            anyhow::bail!("calibration_busy: cannot calibrate while state={current_state}");
        }
        let mut recorder_guard = self.recorder.lock().await;
        let recorder = Self::recorder_from_guard_mut(&mut recorder_guard, "Calibration")?;
        Self::ensure_recorder_ready_for_start(recorder, "Calibration preflight").await?;

        let accumulator = Arc::new(std::sync::Mutex::new(CaptureLevelAccumulator::new()));
        let sink = Arc::clone(&accumulator);
        recorder.recorder.config.auto_silence = false;
        recorder.recorder.set_callback(Box::new(move |data| {
            sink.lock()
                .unwrap_or_else(|error| error.into_inner())
                .push_samples(data);
        }));
        info!(
            duration_secs = duration.as_secs_f32(),
            "acoustic calibration capture starting"
        );
        recorder.recorder.start().await?;
        tokio::time::sleep(duration).await;
        let stopped = recorder.recorder.stop().await;
        Self::clear_recorder_callbacks(recorder);
        let temp_wav = stopped?;
        if let Some(path) = temp_wav
            && let Err(error) = std::fs::remove_file(&path)
        {
            warn!(path = %path.display(), %error, "calibration temp WAV could not be removed");
        }
        let meta = CapturePathMeta::from_open_path(
            recorder.recorder.actual_sample_rate(),
            recorder.recorder.last_native_channels(),
            recorder.recorder.last_input_device(),
        );
        drop(recorder_guard);

        let receipt = accumulator
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .finalize(meta);
        receipt.log();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or_default();
        let profile = EnergyCalibrationProfile::derive(&receipt, now_ms, SOURCE_GUIDED_CAPTURE)
            .map_err(|refusal| anyhow::Error::new(GuidedCalibrationRefused(refusal)))?;
        let path = energy_calibration_path();
        EnergyCalibrationArtifact::record_profile(&path, profile.clone(), now_ms)
            .map_err(|refusal| anyhow::anyhow!("calibration_store_failed: {refusal}"))?;
        // The caller's Settings refresh runs immediately after this method
        // returns. Replace the controller generation synchronously while the
        // calibration still owns `serial_lock`, so that probe cannot observe
        // the pre-calibration snapshot.
        self.refresh_runtime_settings_after_calibration_locked()
            .await?;
        info!(
            device = %profile.capture_path.device_name,
            version = %profile.version,
            existence_threshold_dbfs = profile.floors.existence_threshold_dbfs,
            path = %path.display(),
            "acoustic calibration profile stored"
        );
        Ok(admission::EnergyCalibrationReport {
            device_name: profile.capture_path.device_name.clone(),
            sample_rate: profile.capture_path.sample_rate,
            measured_seconds: receipt.active_speech_samples as f32
                / receipt.sample_rate.max(1) as f32,
            active_speech_median_dbfs: profile.measurement.active_speech_median_dbfs,
            noise_floor_dbfs: profile.measurement.noise_floor_dbfs,
            peak_dbfs: profile.measurement.peak_dbfs,
            existence_threshold_dbfs: profile.floors.existence_threshold_dbfs,
            version: profile.version,
            path,
        })
    }

    /// App name latched before the current overlay session took focus.
    ///
    /// This is a read-only snapshot for UI copy. Delivery continues to read the
    /// same field inside `paste_text_from_overlay`; exposing it does not alter the
    /// focus restoration or clipboard path.
    pub async fn paste_target_app_name(&self) -> Option<String> {
        self.pre_overlay_frontmost_app.read().await.clone()
    }

    /// Paste user-edited overlay text through the delivery throne, then restore
    /// the latched target and synthesize Cmd+V via clipboard.
    ///
    /// `resolve_delivery_route(OverlayInsert)` picks the destination. Overlay
    /// **caret** (Swift `defer_text_from_overlay`) arms Paste Here. Agent
    /// window, Alacritty, and every other latched caret get Cmd+V. Unconfirmed
    /// ambulances park Paste Here and leave the user's clipboard alone.
    pub async fn paste_text_from_overlay(&self, text: String) -> Result<OverlayPasteResult> {
        let trimmed = text.trim();
        let target_app = self.pre_overlay_frontmost_app.read().await.clone();
        if trimmed.is_empty() {
            return Ok(OverlayPasteResult::noop());
        }
        let config = self.get_config().await;
        let payload = self.delivery_tagger.render(trimmed, &config, None);

        self.execute_clipboard_paste(payload, target_app, "Overlay paste", PasteMode::Comfort)
            .await
    }

    /// Execute an already decided `ClipboardPaste`: activate the latched
    /// target, confirm it owns focus, borrow the clipboard for one Cmd+V.
    ///
    /// Destination selection stays in [`resolve_delivery_route`]; this is
    /// transport only. Focus counts as confirmed when the bounded wait saw the
    /// target frontmost **or** the target is observed frontmost afterwards
    /// (an accepted-but-unconfirmed activation of an app that already owned
    /// focus). A latched target that confirmed neither never yields to whoever
    /// is frontmost; only an Insert with no latch may follow the external
    /// frontmost app (`delivery_route::clipboard_paste_may_post`). An
    /// unconfirmed ambulance or a denied event tap parks Paste Here and leaves
    /// the user's pasteboard alone.
    async fn execute_clipboard_paste(
        &self,
        paste_text: String,
        target_app: Option<String>,
        context: &'static str,
        paste_mode: PasteMode,
    ) -> Result<OverlayPasteResult> {
        let config = self.get_config().await;
        let focus_confirmed = target_app
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .is_some_and(|name| {
                is_codescribe_app(name)
                    || (crate::os::selection::activate_app_by_name(name)
                        && crate::os::selection::wait_for_frontmost_app(
                            name,
                            Duration::from_millis(250),
                        ))
            });
        let target = clipboard::StopPasteTarget::capture();
        let frontmost = crate::os::selection::current_frontmost_app_name();
        let target_observed_frontmost = matches!(
            (target_app.as_deref(), frontmost.as_deref()),
            (Some(target), Some(front)) if front.trim().eq_ignore_ascii_case(target.trim())
        );
        let frontmost_is_external = frontmost
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .is_some_and(|name| !is_codescribe_app(name));
        debug!(
            target = ?target_app,
            frontmost = ?frontmost,
            focus_confirmed_by_wait = focus_confirmed,
            target_observed_frontmost,
            frontmost_is_external,
            "{context}: paste target activation"
        );
        // Throne law, shadowed on purpose so the corridor below reads exactly
        // as the contract states it: a latched target must have confirmed
        // focus or be observed frontmost; only an Insert with no latch may
        // follow the external frontmost app.
        let focus_confirmed = delivery_route::clipboard_paste_may_post(
            target_app.is_some(),
            focus_confirmed && target_observed_frontmost,
            target_observed_frontmost,
            frontmost_is_external,
        ) && frontmost
            .as_deref()
            .is_some_and(|name| target.matches_app_name(name));

        let preflight = clipboard::synthetic_paste_preflight();
        let mut facts = overlay_insert_facts(!paste_text.is_empty(), false);
        facts.paste_target = helpers::observe_paste_target(frontmost.as_deref());
        facts.executable_payload = looks_executable(&paste_text);
        let decision = resolve_delivery_route(DeliveryIntent::OverlayInsert, facts);
        info!(
            "{}",
            format_delivery_route_line(
                DeliveryIntent::OverlayInsert,
                decision,
                target_app.as_deref()
            )
        );

        let mut deferred_insert_shortcut = None;
        let mut deferred_insert_failure = None;
        let delivery = if decision.route == DeliveryRoute::ClipboardHold {
            self.arm_or_copy_deferred_payload(
                paste_text.clone(),
                &config,
                &mut deferred_insert_shortcut,
                &mut deferred_insert_failure,
            )?
        } else if focus_confirmed && preflight.can_post_events() {
            match clipboard::paste_to_stop_target(
                &paste_text,
                &target,
                paste_mode,
                config.restore_clipboard,
                config.restore_clipboard_delay_ms,
            ) {
                Ok(receipt) if receipt.delivery == clipboard::StopPasteDelivery::Pasted => {
                    OverlayPasteDelivery::PasteRequested
                }
                outcome => {
                    if let Err(error) = outcome {
                        warn!(%error, "{context}: paste failed; arming deferred insert");
                    }
                    self.arm_or_copy_deferred_payload(
                        paste_text.clone(),
                        &config,
                        &mut deferred_insert_shortcut,
                        &mut deferred_insert_failure,
                    )?
                }
            }
        } else {
            warn!(
                target_app = ?target_app,
                frontmost_app = ?frontmost,
                cg_post_event_access = preflight.cg_post_event_access,
                ax_trusted = preflight.ax_trusted,
                focus_confirmed,
                "{context}: could not execute the selected clipboard route; arming deferred insert"
            );
            self.arm_or_copy_deferred_payload(
                paste_text,
                &config,
                &mut deferred_insert_shortcut,
                &mut deferred_insert_failure,
            )?
        };

        Ok(OverlayPasteResult {
            delivery,
            target_app_name: target_app,
            frontmost_app_name: frontmost,
            deferred_insert_shortcut,
            deferred_insert_failure,
        })
    }

    /// Stop-path delivery: the committed live document goes where the session
    /// intent froze it at start. One `delivery_route:` line per take.
    ///
    /// `seal_refused` marks a degraded delivery: the ledger refused the
    /// terminal seal, history keeps its `failed` verdict, and the text is the
    /// same committed document the overlay already shows. Nothing here writes
    /// the ledger or invents a witness. `live_stream_session` and
    /// `commit_required` have no producer on this path today and are false by
    /// construction.
    async fn deliver_stop_transcript(
        &self,
        take_id: Option<&str>,
        text: &str,
        assistive: bool,
        force_ai: bool,
        capture_turn: CaptureTurnIntent,
        seal_refused: bool,
    ) -> Result<TranscriptDelivery> {
        let config = self.get_config().await;
        let paste_mode = config.paste_mode;
        self.deliver_stop_transcript_with_sink(
            take_id,
            text,
            (assistive, force_ai, capture_turn, seal_refused),
            &config,
            |route, text, target| async move {
                match route {
                    DeliveryRoute::DeferredInsert | DeliveryRoute::ClipboardHold => {
                        self.arm_overlay_text(&text, target, Some("Codescribe".to_string()))
                            .await
                    }
                    _ => {
                        self.execute_clipboard_paste(text, target, "Stop-path paste", paste_mode)
                            .await
                    }
                }
            },
        )
        .await
    }

    async fn begin_stop_paste(
        &self,
        take_id: Option<&str>,
        stopped_at: std::time::Instant,
    ) -> StopCanvasRequest {
        // Register before any await: the tap can interrupt even if its serial
        // receiver is waiting for this controller operation to finish.
        let preemption = StopPasteWaitRegistration::new();
        let target = clipboard::StopPasteTarget::capture();
        self.active_stop_receipt
            .read()
            .await
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .begin(take_id, tokio::time::Instant::from_std(stopped_at));
        let painted_at_stop = self
            .active_presentation
            .read()
            .await
            .as_ref()
            .filter(|emitter| take_id.is_some_and(|id| emitter.owns_document(id)))
            .and_then(|emitter| emitter.begin_stop_canvas());
        StopCanvasRequest {
            stopped_at,
            painted_at_stop,
            target,
            preemption,
        }
    }

    /// Settle the painted live finals before archive and terminal refinement drain.
    async fn deliver_frozen_canvas_at_stop(
        &self,
        recorder: &mut ClosedTake,
        take_id: Option<&str>,
        intent: (bool, bool, CaptureTurnIntent),
        stop: StopCanvasRequest,
        capture_closed_at: std::time::Instant,
        runtime_settings: &RuntimeSettingsSnapshot,
    ) -> StopCanvasDelivery {
        let (assistive, force_ai, capture_turn) = intent;
        let presentation = self.active_presentation.read().await.clone();
        let armed_order = recorder.seal_lane_armed();
        let mut wait = await_live_finals_for_delivery(
            async {
                let admitted = recorder.wait_live_finals_admitted().await;
                match presentation.as_ref() {
                    Some(emitter) => emitter.wait_paint_published().await && admitted,
                    None => admitted,
                }
            },
            || {
                presentation
                    .as_ref()
                    .filter(|emitter| take_id.is_some_and(|id| emitter.owns_document(id)))
                    .and_then(|emitter| emitter.finish_stop_canvas())
            },
            // The virtual capture grace precedes EOF. It must not consume the
            // recognizers' separate end-final budget after that fence.
            tokio::time::Instant::from_std(capture_closed_at),
            stop.painted_at_stop,
            armed_order,
            Some(&stop.preemption.0),
        )
        .await;
        // Closing and requesting share one lock. A next-start that races the
        // final's ready tick wins until settlement closes this registration.
        wait.preempted |= stop.preemption.close();
        wait.stop_final_timeout |= wait.preempted;
        if wait.preempted {
            wait.live_finals_admitted = false;
        }
        let config = runtime_settings.values().clone();
        self.stop_canvas_delivery(
                take_id,
                presentation.as_deref(),
                wait,
                stop.stopped_at,
                (assistive, capture_turn, runtime_settings),
                |text| async move {
                    let text = if !assistive {
                        self.select_paste_projection(take_id, runtime_settings, text)
                            .await
                    } else {
                        text
                    };
                    self.deliver_stop_transcript_with_sink(
                        take_id,
                        &text,
                        (assistive, force_ai, capture_turn, false),
                        &config,
                        |route, payload, target_app| async move {
                            if matches!(
                                route,
                                DeliveryRoute::ClipboardHold | DeliveryRoute::DeferredInsert
                            ) {
                                return self
                                    .arm_overlay_text(
                                        &payload,
                                        target_app,
                                        Some("Codescribe".into()),
                                    )
                                    .await;
                            }
                            let can_post = clipboard::synthetic_paste_preflight().can_post_events();
                            if !can_post {
                                return self
                                    .arm_overlay_text(
                                        &payload,
                                        target_app,
                                        crate::os::selection::current_frontmost_app_name(),
                                    )
                                    .await;
                            }
                            let outcome = clipboard::paste_to_stop_target(
                                &payload,
                                &stop.target,
                                config.paste_mode,
                                config.restore_clipboard,
                                config.restore_clipboard_delay_ms,
                            );
                            if !matches!(outcome, Ok(receipt) if receipt.delivery == clipboard::StopPasteDelivery::Pasted) {
                                if let Err(error) = outcome {
                                    warn!(%error, "Stop-path paste failed; arming deferred insert");
                                }
                                return self
                                    .arm_overlay_text(
                                        &payload,
                                        target_app,
                                        crate::os::selection::current_frontmost_app_name(),
                                    )
                                    .await;
                            }
                            let delivery = OverlayPasteDelivery::PasteRequested;
                            Ok(OverlayPasteResult {
                                delivery,
                                // Automatic stop uses the retained AX identity;
                                // the capture-start app label is not this target.
                                target_app_name: None,
                                frontmost_app_name:
                                    crate::os::selection::current_frontmost_app_name(),
                                deferred_insert_shortcut: None,
                                deferred_insert_failure: None,
                            })
                        },
                    )
                    .await
                },
            )
            .await
    }

    /// Decide first-Stop delivery before invoking the single transport boundary.
    /// Tests supply that boundary after the original ClosedTake completion wait.
    async fn stop_canvas_delivery<F, Fut>(
        &self,
        take_id: Option<&str>,
        presentation: Option<&PresentationEmitter>,
        wait: StopCanvasWait,
        stop_start: std::time::Instant,
        intent: (bool, CaptureTurnIntent, &RuntimeSettingsSnapshot),
        deliver: F,
    ) -> StopCanvasDelivery
    where
        F: FnOnce(String) -> Fut,
        Fut: std::future::Future<Output = Result<TranscriptDelivery>>,
    {
        let (assistive, capture_turn, runtime_settings) = intent;
        let preempted = wait.preempted;
        if take_delivers_to_composer(capture_turn) {
            return StopCanvasDelivery {
                delivery: Ok(None),
                preempted: false,
            };
        }
        if preempted {
            // A new take does not authorize pasting the old take's unfinished
            // paint. Its owned drain retains the audio and terminal document.
            self.record_delivery_disposition(TranscriptDelivery::Retained)
                .await;
            return StopCanvasDelivery {
                delivery: Ok(Some(TranscriptDelivery::Retained)),
                preempted: true,
            };
        }
        if !assistive
            && runtime_settings.formatting_policy() != FormattingPolicy::Off
            && presentation
                .as_ref()
                .is_some_and(|emitter| !emitter.literal_delivery())
        {
            // The painted prefix is not the terminal formatter source. Selecting
            // it here would permanently freeze the fallback before the last
            // occurrence results can settle. PCM is already closed; leave the
            // one delivery claim to the existing terminal path.
            return StopCanvasDelivery {
                delivery: Ok(None),
                preempted: false,
            };
        }
        let committed_canvas = wait
            .snapshot
            .as_ref()
            .is_some_and(|canvas| canvas.has_committed_document && canvas.preview_only_words == 0);
        let capture_id = presentation.and_then(|emitter| {
            emitter
                .capture_session_for_document(take_id.unwrap_or_default())
                .ok()
        });
        if !wait.permits_first_delivery(capture_id.as_deref()) {
            info!(
                take_id,
                stop_final_wait_ms = wait.stop_final_wait_ms,
                stop_final_timeout = wait.stop_final_timeout,
                live_finals_admitted = wait.live_finals_admitted,
                committed_canvas,
                "stop paste deferred until terminal text settlement"
            );
            // None leaves the single delivery claim for the existing terminal
            // path. A paint timeout or recognizer close cannot settle a prefix
            // and suppress the later, complete document.
            return StopCanvasDelivery {
                delivery: Ok(None),
                preempted: false,
            };
        }
        let delivery = self
            .settle_frozen_canvas_at_stop(take_id, presentation, wait, stop_start, deliver)
            .await
            .map(Some);
        StopCanvasDelivery {
            delivery,
            preempted,
        }
    }

    /// The microphone is already closed. Transfer preempted or publication-
    /// pending terminal work and its exact Bus into one tracked take-owned tail.
    /// It retains audio and Bus finality but has no authority over the next UI.
    async fn detach_preempted_stop(&self, take: ClosedTake, take_id: Option<&str>) -> Result<()> {
        let shared_document = take.is_shared_document();
        let capture_session = take.capture_identity().0.map(str::to_owned);
        if let Some(emitter) = self.active_presentation.read().await.as_ref() {
            emitter.retire_presentation();
        }
        let bus = self.active_transcript_bus.write().await.take();
        let take_id = take_id.map(str::to_owned);
        let retained_capture = retainable_session_id(capture_session.as_deref()).and_then(|id| {
            codescribe_core::state::history::audio_retention::capture(&Config::config_dir(), id)
        });
        let delivery = *self.delivery_disposition.read().await;
        let task = tokio::spawn(async move {
            let (stopped, observer) = stop_closed_take_for_terminal(take, take_id.as_deref()).await;
            let reason = match &stopped {
                Ok((text, path)) => {
                    if let Some(path) = path.as_deref() {
                        retain_session_audio_with_lease(
                            take_id.as_deref(),
                            path,
                            codescribe_core::state::SessionTranscriptArchive::from_committed(text),
                            retained_capture.clone(),
                            &observer,
                        )
                        .await;
                        if shared_document {
                            finish_audio_capture(capture_session.as_deref());
                        }
                    } else {
                        retain_stopped_document(
                            take_id.as_deref(),
                            capture_session.as_deref(),
                            text,
                            None,
                            shared_document,
                            &observer,
                        )
                        .await;
                    }
                    TranscriptSessionEndReason::Completed
                }
                Err(error) if error.is::<TerminalSealRefused>() => {
                    TranscriptSessionEndReason::CoverageRefused
                }
                Err(error) => {
                    warn!(take_id = ?take_id, %error, "retired take terminal drain failed");
                    TranscriptSessionEndReason::TranscriptionFailed
                }
            };
            let wav_exists = retainable_session_id(take_id.as_deref())
                .and_then(|id| session_audio_path(&Config::config_dir(), id))
                .is_some_and(|path| path.is_file());
            // Deliberately no UI broadcast: this terminal evidence belongs to
            // the retired take, not to the new capture's overlay.
            Self::end_transcript_bus(&RwLock::new(bus), reason, wav_exists, delivery, None, None)
                .await;
            drop(retained_capture);
        });
        let mut tails = self
            .closed_capture_tails
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        tails.retain(|task| !task.is_finished());
        tails.push(task);
        Ok(())
    }

    /// The sole stop settlement, with the destination effect injected for
    /// falsifiers. Preview text never becomes a Light+ document revision merely
    /// because it was visible at timeout.
    async fn settle_frozen_canvas_at_stop<F, Fut>(
        &self,
        take_id: Option<&str>,
        presentation: Option<&PresentationEmitter>,
        wait: StopCanvasWait,
        stop_start: std::time::Instant,
        deliver: F,
    ) -> Result<TranscriptDelivery>
    where
        F: FnOnce(String) -> Fut,
        Fut: std::future::Future<Output = Result<TranscriptDelivery>>,
    {
        let StopCanvasWait {
            snapshot,
            stop_final_wait_ms,
            stop_final_timeout,
            live_finals_admitted,
            painted_at_stop,
            preempted,
            armed_order,
        } = wait;
        let capture_id = presentation.and_then(|emitter| {
            emitter
                .capture_session_for_document(take_id.unwrap_or_default())
                .ok()
        });
        let snapshot =
            snapshot.filter(|canvas| Some(canvas.session_id.as_str()) == capture_id.as_deref());
        let painted_words_at_stop = painted_at_stop
            .as_ref()
            .map_or(0, |canvas| canvas.visible_words.len());
        if preempted {
            info!(take_id, "stop_paste_preempted_by_next_take");
        }
        let (snapshot, light_plus) = match snapshot {
            Some(snapshot) if !snapshot.text.trim().is_empty() => {
                let presentation = presentation
                    .ok_or_else(|| anyhow::anyhow!("stop canvas has no presentation owner"))?;
                // Label only after this single call. An accepted stop revision
                // replaces `revision` on the returned snapshot; the same snapshot
                // comes back when Light+ commits nothing. Preview and literal
                // stay the states the shaper already refused to rewrite.
                let offered_revision = snapshot.revision;
                let snapshot = presentation
                    .shape_frozen_canvas_at_stop(snapshot)
                    .map_err(|error| anyhow::anyhow!("Light+ stop revision refused: {error}"))?;
                let light_plus =
                    if snapshot.preview_only_words > 0 || !snapshot.has_committed_document {
                        "skipped_preview"
                    } else if presentation.literal_delivery() {
                        "literal"
                    } else if snapshot.revision != offered_revision {
                        "applied"
                    } else {
                        "unchanged"
                    };
                (Some(snapshot), light_plus)
            }
            snapshot => {
                warn!(
                    take_id,
                    stop_final_wait_ms,
                    stop_final_timeout,
                    qualified_occurrences = snapshot
                        .as_ref()
                        .map_or(0, |canvas| canvas.qualified_occurrences),
                    capture_epoch = snapshot.as_ref().map(|canvas| canvas.capture_epoch),
                    "stop_canvas_empty"
                );
                (snapshot, "literal")
            }
        };
        let canvas = snapshot.as_ref();
        let text = canvas.map_or("", |canvas| canvas.text.as_str());
        let preview_words = canvas.map_or(0, |canvas| canvas.preview_only_words);
        // Absent optional fields preserve the settled receipt for takes without
        // late Apple evidence. Read the same frozen canvas used by the sink.
        let late_apple_counts = canvas
            .map(VisibleCanvasSnapshot::late_apple_word_counts)
            .filter(|&(pasted, covered)| pasted != 0 || covered != 0);
        let paste_words = text.split_whitespace().count();
        let painted_words_at_snapshot = canvas.map_or(0, |canvas| canvas.visible_words.len());
        let missing_words = painted_at_stop
            .as_ref()
            .map_or_else(Vec::new, |before| match canvas {
                Some(after) => before.missing_words_from(after),
                None => before
                    .visible_words
                    .iter()
                    .map(|word| crate::presentation::emitter::MissingVisibleWord {
                        word: word.word.clone(),
                        reason: "unaccounted".into(),
                    })
                    .collect(),
            });
        let superseded_by_partial_words = missing_words
            .iter()
            .filter(|word| word.reason.starts_with("superseded_by_partial rev="))
            .count();
        let admitted_into_words = missing_words
            .iter()
            .filter(|word| word.reason.starts_with("admitted_into occurrence="))
            .count();
        let closed_by_final_words = missing_words
            .iter()
            .filter(|word| word.reason.starts_with("closed_by_final phrase="))
            .count();
        let retained_as_evidence_words = missing_words
            .iter()
            .filter(|word| word.reason.starts_with("retained_as_evidence occurrence="))
            .count();
        let untimed_final_phrase_arrivals = canvas.map(|canvas| &canvas.untimed_final_phrases);
        let untimed_final_phrases = untimed_final_phrase_arrivals.map_or(0, Vec::len);
        let moved_to_pending_words = missing_words
            .iter()
            .filter(|word| word.reason == "moved_to pending")
            .count();
        let moved_to_unmatched_words = missing_words
            .iter()
            .filter(|word| word.reason == "moved_to unmatched")
            .count();
        let untimed_final_words = canvas.map_or(0, |canvas| canvas.untimed_final_words).max(
            painted_at_stop
                .as_ref()
                .map_or(0, |canvas| canvas.untimed_final_words),
        );
        let covered_by_committed_words = missing_words
            .iter()
            .filter(|word| word.reason.starts_with("covered_by_committed occurrence="))
            .count();
        let relabeled_in_place_words = missing_words
            .iter()
            .filter(|word| {
                word.reason
                    .split("; ")
                    .any(|reason| reason.starts_with("relabeled_in_place occurrence="))
            })
            .count();
        let reshaped_in_place_words = missing_words
            .iter()
            .filter(|word| {
                word.reason
                    .split("; ")
                    .any(|reason| reason.starts_with("reshaped_in_place occurrence="))
            })
            .count();
        let unaccounted = missing_words
            .iter()
            .filter(|word| word.reason == "unaccounted")
            .count();
        if !missing_words.is_empty() {
            warn!(take_id, paste_words, painted_words_at_stop, painted_words_at_snapshot,
                missing_words = ?missing_words, unaccounted, defect = unaccounted > 0,
                "stop_paste_lost_visible_words");
            if unaccounted > 0 {
                error!(take_id, missing_words = ?missing_words, "stop_paste_visible_word_defect");
            }
        }
        // Settlement must precede the irreversible destination effect. A
        // closed preview alone cannot authorize it; only an accounted committed
        // projection from this capture may be handed off. The production caller
        // defers unresolved paint to terminal processing before reaching here.
        let accounted = !preempted
            && live_finals_admitted
            && canvas.is_some_and(|canvas| {
                canvas.has_committed_document
                    && canvas.preview_only_words == 0
                    && presentation
                        .and_then(PresentationEmitter::visible_canvas_snapshot)
                        .is_some_and(|current| {
                            current.session_id == canvas.session_id
                                && current.capture_epoch == canvas.capture_epoch
                                && current.revision == canvas.revision
                        })
            })
            && !requires_terminal_accounting(&missing_words);
        let delivery = if accounted && !text.trim().is_empty() {
            deliver(text.to_owned()).await?
        } else {
            if !text.trim().is_empty() || !missing_words.is_empty() {
                warn!(
                    take_id,
                    unaccounted,
                    closed_by_final_words,
                    "stop canvas retained without committed delivery accounting"
                );
                self.publish_stop_warning(
                    "stop_canvas_accounting_unsettled",
                    "Recording stopped. Text and audio are retained for review; this stop projection could not authorize delivery.".to_string(),
                );
            }
            TranscriptDelivery::Retained
        };
        self.record_delivery_disposition(delivery).await;
        info!(
            stop_to_delivery_ms = stop_start.elapsed().as_millis(),
            delivery = match delivery {
                TranscriptDelivery::SinkAccepted => "pasted",
                TranscriptDelivery::CopiedToClipboard => "copied",
                TranscriptDelivery::DeferredInsertArmed => "deferred_insert",
                TranscriptDelivery::Unattempted => "unattempted",
                TranscriptDelivery::ComposerPending => "composer_pending",
                TranscriptDelivery::Retained => "retained",
                TranscriptDelivery::EndedWithoutVoice => "ended_without_voice",
            },
            stop_final_wait_ms,
            stop_final_timeout,
            live_finals_admitted,
            painted_words_at_stop,
            painted_words_at_snapshot,
            superseded_by_partial_words,
            admitted_into_words,
            closed_by_final_words,
            retained_as_evidence_words,
            untimed_final_phrases,
            untimed_final_phrase_arrivals = ?untimed_final_phrase_arrivals,
            moved_to_pending_words,
            moved_to_unmatched_words,
            untimed_final_words,
            covered_by_committed_words,
            relabeled_in_place_words,
            reshaped_in_place_words,
            unaccounted,
            armed_order,
            light_plus,
            capture_epoch = canvas.map(|canvas| canvas.capture_epoch),
            reducer_revision = canvas.map(|canvas| canvas.revision),
            preview_only_words = preview_words,
            preview_words_in_paste = preview_words,
            late_apple_words_pasted = late_apple_counts.map(|(pasted, _)| pasted),
            late_apple_words_covered_by_slot = late_apple_counts.map(|(_, covered)| covered),
            paste_words,
            paste_periods = text.chars().filter(|ch| *ch == '.').count(),
            paste_commas = text.chars().filter(|ch| *ch == ',').count(),
            paste_qe = text.chars().filter(|ch| matches!(ch, '?' | '!')).count(),
            "stop canvas delivery settled"
        );
        self.active_stop_receipt
            .read()
            .await
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .settle(
                take_id,
                stop_final_timeout,
                if matches!(
                    delivery,
                    TranscriptDelivery::SinkAccepted
                        | TranscriptDelivery::CopiedToClipboard
                        | TranscriptDelivery::DeferredInsertArmed
                ) {
                    text
                } else {
                    ""
                },
            );
        Ok(delivery)
    }

    async fn deliver_stop_transcript_with_sink<F, Fut>(
        &self,
        take_id: Option<&str>,
        text: &str,
        intent: (bool, bool, CaptureTurnIntent, bool),
        config: &Config,
        sink: F,
    ) -> Result<TranscriptDelivery>
    where
        F: FnOnce(DeliveryRoute, String, Option<String>) -> Fut,
        Fut: std::future::Future<Output = Result<OverlayPasteResult>>,
    {
        let (assistive, force_ai, capture_turn, seal_refused) = intent;
        let trimmed = text.trim();
        if trimmed.is_empty() {
            self.record_delivery_disposition(TranscriptDelivery::Retained)
                .await;
            return Ok(TranscriptDelivery::Retained);
        }
        {
            let mut delivered = self.delivered_take.lock().await;
            if !claim_take_delivery(&mut delivered, take_id) {
                return Err(anyhow::anyhow!(
                    "stop-path handoff already attempted for this take"
                ));
            }
        }
        // An assistive take that heard no voice for 30 s is not a message.
        // Its text stays in the take archive and no destination is chosen:
        // no paste, no clipboard, no Agent (Founder decision 2026-10-26).
        if self.take_ended_by_silence(take_id) {
            self.record_delivery_disposition(TranscriptDelivery::EndedWithoutVoice)
                .await;
            info!(
                seal_refused,
                "delivery_route: skipped; the assistive take ended without voice"
            );
            return Ok(TranscriptDelivery::EndedWithoutVoice);
        }
        // A one-turn take has exactly one destination: the Agent composer draft
        // of the thread that owned the capture. It must not also post a
        // synthetic paste into whatever app happens to be frontmost — two
        // destinations for one take is how a transcript lands where nobody was
        // looking. The disposition is `ComposerPending` on purpose: the route
        // is decided here, admission is not, and only the receiver can say so.
        if take_delivers_to_composer(capture_turn) {
            let disposition = if trimmed.is_empty() {
                // An empty capture claims no delivery at all.
                TranscriptDelivery::Retained
            } else {
                TranscriptDelivery::ComposerPending
            };
            if disposition == TranscriptDelivery::ComposerPending {
                let payload = self.delivery_tagger.render(trimmed, config, Some("agent"));
                *self.composer_delivery_payload.write().await =
                    Some((take_id.unwrap_or_default().to_string(), payload));
            }
            self.record_delivery_disposition(disposition).await;
            info!(
                seal_refused,
                pending = !trimmed.is_empty(),
                "delivery_route: intent=agent_composer route=ComposerDraft"
            );
            return Ok(disposition);
        }
        let selected_refused = if seal_refused && !assistive {
            let settings = automatic_take_settings(self.runtime_settings_arc().await);
            Some(
                self.select_paste_projection_with_assembly(
                    take_id,
                    settings.as_ref(),
                    trimmed.to_string(),
                    crate::presentation::emitter::ProjectionAssembly::RefusedPerOccurrence,
                )
                .await,
            )
        } else {
            None
        };
        let trimmed = selected_refused.as_deref().unwrap_or(trimmed).trim();
        let notes_save_only = config.quick_notes_enabled && config.quick_notes_save_only;
        let intent = delivery_intent_from_session(assistive, force_ai, notes_save_only);
        let latched_target = self.pre_overlay_frontmost_app.read().await.clone();
        // Only an armed Orient paste needs the caret observed; the probe can
        // hold that paste, never redirect it.
        let paste_target = if matches!(
            intent,
            DeliveryIntent::OrientDictation | DeliveryIntent::OrientFormat
        ) && config.paste_mode != PasteMode::Off
        {
            helpers::observe_paste_target(latched_target.as_deref())
        } else {
            PasteTarget::UNOBSERVED
        };
        let decision = resolve_delivery_route(
            intent,
            DeliveryFacts {
                has_text: !trimmed.is_empty(),
                no_speech: false,
                paste_mode: config.paste_mode,
                paste_target,
                executable_payload: looks_executable(trimmed),
                overlay_enabled: config.transcription_overlay_enabled,
                live_stream_session: false,
                commit_required: false,
                latched_target_is_self: false,
            },
        );
        info!(
            seal_refused,
            paste_target = ?paste_target,
            "{}",
            format_delivery_route_line(intent, decision, latched_target.as_deref())
        );
        if !matches!(
            decision.route,
            DeliveryRoute::ClipboardPaste
                | DeliveryRoute::DeferredInsert
                | DeliveryRoute::ClipboardHold
        ) {
            // No sink was selected. The committed text is still readable in the
            // overlay and the session archive, so this is retained, not lost.
            self.record_delivery_disposition(TranscriptDelivery::Retained)
                .await;
            return Ok(TranscriptDelivery::Retained);
        }
        let mode = if assistive {
            "assistive"
        } else if force_ai {
            "format"
        } else {
            "dictation"
        };
        let payload = self.delivery_tagger.render(trimmed, config, Some(mode));
        let receipt_payload = payload.clone();
        let outcome = sink(decision.route, payload, latched_target).await;
        if let (Some(notice), Ok(result)) = (paste_hold_notice(decision), outcome.as_ref())
            && result.delivery == OverlayPasteDelivery::DeferredInsertArmed
        {
            let guidance = result.deferred_insert_shortcut.as_deref().map_or_else(
                || "Choose a Deferred Paste shortcut in Settings.".to_string(),
                |shortcut| format!("Press {shortcut} in the target input to insert it."),
            );
            helpers::announce_paste_hold(&format!("{notice} {guidance}"));
        }
        let result = self.finish_stop_delivery(outcome, seal_refused).await;
        if let Some(presentation) = self.active_presentation.read().await.as_ref() {
            presentation.record_projection_delivery(
                &receipt_payload,
                result
                    .as_ref()
                    .copied()
                    .unwrap_or(TranscriptDelivery::Retained),
            );
        }
        result
    }

    /// The real transport result enters here; tests may inject this boundary
    /// without opening a clipboard, microphone, or a second delivery owner.
    async fn finish_stop_delivery(
        &self,
        outcome: Result<OverlayPasteResult>,
        seal_refused: bool,
    ) -> Result<TranscriptDelivery> {
        match outcome {
            Ok(result) => {
                // A declined payload or missing permission is not acceptance.
                let disposition = match result.delivery {
                    OverlayPasteDelivery::PasteRequested => TranscriptDelivery::SinkAccepted,
                    OverlayPasteDelivery::CopiedToClipboard => {
                        TranscriptDelivery::CopiedToClipboard
                    }
                    OverlayPasteDelivery::DeferredInsertArmed => {
                        TranscriptDelivery::DeferredInsertArmed
                    }
                    OverlayPasteDelivery::Noop
                    | OverlayPasteDelivery::AccessibilityPermissionNeeded => {
                        TranscriptDelivery::Retained
                    }
                };
                self.record_delivery_disposition(disposition).await;
                info!(
                    delivery = ?result.delivery,
                    target = ?result.target_app_name,
                    frontmost = ?result.frontmost_app_name,
                    shortcut = ?result.deferred_insert_shortcut,
                    failure = ?result.deferred_insert_failure,
                    seal_refused,
                    ?disposition,
                    "stop-path delivery finished"
                );
                if disposition == TranscriptDelivery::Retained {
                    return Err(anyhow::Error::new(StopDeliveryFailure {
                        cause: anyhow::anyhow!("selected sink declined the transcript"),
                        refusal: None,
                    }));
                }
                Ok(disposition)
            }
            Err(err) => {
                // A failed sink keeps the text recoverable; it never becomes an
                // accepted delivery just because the attempt returned.
                self.record_delivery_disposition(TranscriptDelivery::Retained)
                    .await;
                warn!(seal_refused, "stop-path delivery failed: {err:#}");
                Err(anyhow::Error::new(StopDeliveryFailure {
                    cause: err,
                    refusal: None,
                }))
            }
        }
    }

    /// One refusal decision for hold and toggle, after the recorder's terminal
    /// tail and WAV retention. Only the typed producer refusal grants access
    /// to committed words. The injected boundary is destination handoff, not
    /// an alternate transcription or lifecycle implementation.
    async fn process_terminal_stop_error<F, Fut>(
        &self,
        error: anyhow::Error,
        deliver: F,
    ) -> Result<ProcessRecordingOutcome>
    where
        F: FnOnce(String) -> Fut,
        Fut: std::future::Future<Output = Result<TranscriptDelivery>>,
    {
        let refusal = error.downcast::<TerminalSealRefused>()?;
        let take_id = self.session_id.read().await.clone();
        // The ledger owns refusal identity/reason. Complete measured coverage
        // can coexist with a missing issued seal; never reinterpret coverage.
        if retainable_session_id(take_id.as_deref()) != Some(refusal.finality.session_id()) {
            return Err(anyhow::anyhow!(
                "terminal refusal does not match the active capture"
            ));
        }
        if refusal.committed_text.trim().is_empty() {
            return Err(anyhow::Error::new(refusal));
        }
        let bus = self.active_transcript_bus.read().await.clone();
        if bus.as_ref().is_none_or(|bus| {
            !bus.matches_refused_document(&refusal.finality, &refusal.committed_text)
        }) {
            return Err(anyhow::anyhow!(
                "terminal refusal has no matching authenticated Bus document"
            ));
        }
        // The retained words are this take's document: the user can revise
        // them like any take, so their versions keep the same durable history.
        let archived = take_refused_take_archive(refusal.finality.session_id());
        self.link_live_archive(take_id.as_deref(), archived).await;
        match deliver(refusal.committed_text.clone()).await {
            Ok(_) => Ok(ProcessRecordingOutcome {
                transcript_present: true,
                refusal: Some(refusal),
                ..ProcessRecordingOutcome::default()
            }),
            Err(cause) => Err(anyhow::Error::new(StopDeliveryFailure {
                cause,
                refusal: Some(refusal),
            })),
        }
    }

    /// Record what this take's stop path attempted. One writer, one read at the
    /// terminal lifecycle line; nothing else in the controller interprets it.
    async fn record_delivery_disposition(&self, disposition: TranscriptDelivery) {
        *self.delivery_disposition.write().await = disposition;
    }

    /// Degrade path when a synthetic paste is not safe to post: park the payload
    /// in the process-local Paste Here slot. Never writes the system pasteboard.
    ///
    /// The out-params carry back what the UI must tell the user — which
    /// shortcut is now armed, or why the chord is not bound. The transcript
    /// still sits in-process either way; the user's clipboard stays put.
    fn arm_or_copy_deferred_payload(
        &self,
        payload: String,
        config: &Config,
        shortcut_label: &mut Option<String>,
        registration_failure: &mut Option<String>,
    ) -> Result<OverlayPasteDelivery> {
        if !clipboard::arm_deferred_insert(payload) {
            return Ok(OverlayPasteDelivery::Noop);
        }
        let collision =
            shortcut_registry::deferred_insert_shortcut_conflict(config.deferred_insert_shortcut);
        if !config.deferred_insert_shortcut.is_enabled() {
            *registration_failure = Some("Paste Here shortcut is disabled".to_string());
        } else if !hotkeys::is_global_hotkey_manager_active() {
            *registration_failure = Some("Paste Here hotkey registration failed".to_string());
        } else if let Some(reason) = collision {
            *registration_failure = Some(reason);
        } else {
            *shortcut_label = Some(config.deferred_insert_shortcut.label().to_string());
        }
        Ok(OverlayPasteDelivery::DeferredInsertArmed)
    }

    /// Arm tagged overlay text for Paste Here. Shared by the throne's
    /// `DeferredInsert` verdict and by the explicit defer click.
    async fn arm_overlay_text(
        &self,
        trimmed: &str,
        target_app: Option<String>,
        frontmost_app_name: Option<String>,
    ) -> Result<OverlayPasteResult> {
        let config = self.get_config().await;
        let payload = trimmed.to_string();
        let mut deferred_insert_shortcut = None;
        let mut deferred_insert_failure = None;
        let delivery = self.arm_or_copy_deferred_payload(
            payload,
            &config,
            &mut deferred_insert_shortcut,
            &mut deferred_insert_failure,
        )?;
        Ok(OverlayPasteResult {
            delivery,
            target_app_name: target_app,
            frontmost_app_name,
            deferred_insert_shortcut,
            deferred_insert_failure,
        })
    }

    /// The one paste-target capture for a take that carries no assistive
    /// context: the app frontmost at take START, with the previous latch as
    /// the fallback when Codescribe itself is frontmost. Hold and toggle
    /// starts both come through here so the latch has a single producer.
    async fn capture_paste_target_context(&self) -> AssistiveContext {
        let prior = self.pre_overlay_frontmost_app.read().await.clone();
        tokio::task::spawn_blocking(move || capture_frontmost_app_only_with_prior_frontmost(prior))
            .await
            .unwrap_or_default()
    }

    /// The one latch writer at take start: store the captured frontmost app
    /// as the paste target and the whole context for the assistive prompt.
    /// Returns whether a target was latched.
    async fn latch_trigger_context(&self, trigger_context: AssistiveContext) -> bool {
        let has_latched_target = trigger_context.frontmost_app.is_some();
        *self.pre_overlay_frontmost_app.write().await = trigger_context.frontmost_app.clone();
        *self.assistive_context.write().await = Some(trigger_context);
        has_latched_target
    }

    /// Arm the edited overlay transcript without attempting target activation.
    /// Used when the caret is known to still be inside Codescribe.
    pub async fn defer_text_from_overlay(&self, text: String) -> Result<OverlayPasteResult> {
        let trimmed = text.trim();
        let target_app = self.pre_overlay_frontmost_app.read().await.clone();
        let intent = DeliveryIntent::OverlayInsert;
        // This entry exists because Swift already knows the caret is inside
        // Codescribe. That is a latched-self fact, not a focus-at-click fact.
        let decision =
            resolve_delivery_route(intent, overlay_insert_facts(!trimmed.is_empty(), true));
        info!(
            "{}",
            format_delivery_route_line(intent, decision, target_app.as_deref())
        );
        if trimmed.is_empty() || decision.route == DeliveryRoute::ArchiveOnly {
            return Ok(OverlayPasteResult::noop());
        }
        let config = self.get_config().await;
        let payload = self.delivery_tagger.render(trimmed, &config, None);
        self.arm_overlay_text(&payload, target_app, Some("Codescribe".to_string()))
            .await
    }

    /// Explicit overlay Copy: write the tagged transcript to the system
    /// pasteboard. This is the only automatic-adjacent verb allowed to replace
    /// the user's clipboard. Insert / stop-path refuse must not call this.
    pub async fn copy_text_from_overlay(&self, text: String) -> Result<()> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        let config = self.get_config().await;
        let payload = self.delivery_tagger.render(trimmed, &config, None);
        clipboard::set_clipboard(&payload).context("Failed to copy overlay text")?;
        Ok(())
    }

    /// Cancel any pending delayed hold-start task
    async fn cancel_pending_hold_start(&self) -> u64 {
        let generation = self.hold_start_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let mut task_guard = self.hold_start_task.lock().await;
        let pending_start_invalidated = match task_guard.take() {
            Some(task) if task.is_finished() => {
                debug!("Cleared finished hold-start task (generation={generation})");
                false
            }
            Some(_) => {
                debug!("Invalidated pending hold-start task (generation={generation})");
                true
            }
            None => false,
        };
        // The pre-overlay app is the take's delivery target, not the start
        // task's scratch. Only a start that never became a take (hold key
        // released before the delay elapsed) loses it here; a finished start
        // means the take is live or sealed and the overlay Insert click that
        // follows still needs it. Hold release runs through this path via
        // `finish_recording`, which is exactly where the target used to vanish
        // (`target_app=None frontmost_app=Some("vc-terminal")` in the log).
        if pending_start_invalidated {
            *self.pre_overlay_frontmost_app.write().await = None;
        }
        generation
    }

    /// Detach every sink and callback from the recorder.
    ///
    /// Run on each stop and before each start: a callback left over from a
    /// finished session would route the next session's audio and deltas into
    /// the previous session's overlay.
    fn clear_recorder_callbacks(recorder: &mut StreamingRecorder) {
        recorder.set_utterance_callback(None);
        recorder.set_utterance_silence_sec(None);
        // A one-turn composer take must never widen into the next hands-free
        // one. This runs before every start and after every stop, so the
        // override cannot outlive the take that asked for it.
        recorder.set_capture_turn_intent(CaptureTurnIntent::HandsFree);
        recorder.set_event_sink(None);
        recorder.set_live_formatting_agent(None);
        // Level metering belongs to the physical capture, not this take. A
        // channel may still own the stream while dictation starts or stops;
        // keep its one tap until the last capture subscriber releases it.
        if !recorder.recorder.is_active() || !recorder.has_capture_subscribers() {
            recorder.set_level_callback(None);
        }
    }

    /// Bring the recorder to a clean pre-start state: force-stop a stream left
    /// active by a previous session, then clear its callbacks. Refusing to
    /// start here would strand the user behind a session they cannot see.
    ///
    /// Admission requires "no active dictation take", never "no capture": an
    /// active stream that capture subscribers legitimately own (a take or the
    /// agent channel) is not stale and must not be force-stopped here. Only a
    /// stream no subscriber owns is leftover state.
    async fn ensure_recorder_ready_for_start(
        recorder: &mut StreamingRecorder,
        context: &str,
    ) -> Result<()> {
        if recorder.recorder.is_active() && !recorder.has_capture_subscribers() {
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
    /// This is the single source of truth for which fields constitute active
    /// recording state so the various reset entry points (start-failure,
    /// finished recording, toggle-stop, nuclear reset) can no longer drift
    /// apart in the subset of fields they clear (P3.1). The latched delivery
    /// target deliberately is not active recording state: after a terminal
    /// reset the overlay still needs it for an explicit Insert click. Failure
    /// and nuclear-reset callers clear that latch in their own tails.
    ///
    /// Ordering note (P2.2): every satellite flag is cleared before
    /// `set_state(State::Idle)` so cross-thread readers (e.g. the VAD monitor
    /// polling `current_state`) never observe Idle alongside stale flags.
    async fn reset_session_fields(&self, reason: TranscriptSessionEndReason) {
        *self.assistive_mode.write().await = false;
        *self.hold_mode.write().await = HoldMode::Raw;
        *self.force_raw_mode.write().await = false;
        *self.force_ai_mode.write().await = false;
        let session_id = self.session_id.write().await.take();
        let session_wav_exists = retainable_session_id(session_id.as_deref())
            .and_then(|id| session_audio_path(&Config::config_dir(), id))
            .is_some_and(|path| path.is_file());
        // Every path back to Idle ends the Bus session exactly once (text-free
        // lifecycle line), so an observer can tell "the take is over" apart
        // from "the take is live" even when zero occurrences sealed.
        // Read the disposition before the reset clears it: the terminal
        // lifecycle line is the one place a destination is stated.
        let delivery = *self.delivery_disposition.read().await;
        let delivery_text = self
            .composer_delivery_payload
            .write()
            .await
            .take()
            .and_then(|(payload_take, payload)| {
                let ended_take = retainable_session_id(session_id.as_deref()).unwrap_or_default();
                (payload_take.is_empty() || payload_take == ended_take).then_some(payload)
            });
        Self::end_transcript_bus(
            &self.active_transcript_bus,
            reason,
            session_wav_exists,
            delivery,
            delivery_text,
            Some(&self.event_broadcast),
        )
        .await;
        let live_capture = self
            .recorder
            .lock()
            .await
            .as_ref()
            .and_then(StreamingRecorder::shared_channel_view)
            .map(|view| view.capture_session_id);
        finish_document_audio_capture(session_id.as_deref(), live_capture.as_deref());
        *self.assistive_context.write().await = None;
        self.start_transition_in_flight
            .store(false, Ordering::SeqCst);
        self.assistive_loop_active.store(false, Ordering::SeqCst);
        self.toggle_user_has_text.store(false, Ordering::SeqCst);
        self.toggle_assistant_has_text
            .store(false, Ordering::SeqCst);
        // `state` becomes Idle only once the rest of the session state is consistent.
        self.set_state(State::Idle).await;
        let path = crate::presentation::transcript_bus::transcript_bus_path();
        tokio::task::spawn_blocking(move || {
            if let Err(error) =
                crate::presentation::transcript_bus_maintenance::compact_bus_if_enabled(
                    &path, "idle",
                )
            {
                warn!(%error, "idle bus compaction unavailable");
            }
        });
    }

    /// End the active Bus session exactly once with a typed reason. This is
    /// the only controller call site of `publish_ended`: the active-take reset,
    /// the pre-active hold unwind and a preempted take's detached tail all go
    /// through here, so no second lifecycle owner can drift on when the
    /// terminal line is written. An installed Bus that never started ends
    /// silently (Bus idempotency). A retired take passes no broadcast: its
    /// terminal line belongs to its own Bus, not to the next take's overlay.
    async fn end_transcript_bus(
        slot: &RwLock<Option<Arc<TranscriptBus>>>,
        reason: TranscriptSessionEndReason,
        session_wav_exists: bool,
        delivery: TranscriptDelivery,
        delivery_text: Option<String>,
        event_broadcast: Option<&broadcast::Sender<IpcEvent>>,
    ) {
        let ended_bus = slot.write().await.take();
        if let Some(bus) = ended_bus
            && let Some(event) = bus.publish_ended_with_delivery_text(
                reason,
                session_wav_exists,
                delivery,
                delivery_text,
            )
            && let Some(event_broadcast) = event_broadcast
        {
            Self::broadcast_transcript_projection(event_broadcast, &event);
        }
    }

    /// The one exit for a delayed hold start that fails or is superseded after
    /// its start guard and before `RecHold`. Stops a recorder that already
    /// opened, detaches its callbacks, writes exactly one terminal Bus line
    /// (none when the Bus never started), releases the session slots and hides
    /// the badge. `state` never left Idle on this path, so it is not touched.
    /// Idempotent: a second call finds every slot empty and the Bus ended.
    async fn unwind_hold_start(
        session: &HoldStartSession,
        rec: Option<&mut StreamingRecorder>,
        abort: HoldStartAbort,
    ) {
        warn!("Hold-start unwound before recording became active: {abort:?}");
        let mut live_capture = None;
        if let Some(rec) = rec {
            if rec.recorder.is_active()
                && let Err(stop_err) = rec.stop_and_discard_path().await
            {
                warn!("Hold-start unwind: stale-session stop failed: {stop_err}");
            }
            Self::clear_recorder_callbacks(rec);
            live_capture = rec
                .shared_channel_view()
                .map(|view| view.capture_session_id);
        }
        // A hold that never became a recording delivered nothing; the take is
        // not "failed delivery", it had no delivery attempt at all.
        Self::end_transcript_bus(
            &session.active_transcript_bus,
            abort.bus_reason(),
            false,
            TranscriptDelivery::Unattempted,
            None,
            Some(&session.event_broadcast),
        )
        .await;
        *session.active_presentation.write().await = None;
        let aborted_id = session.session_id.write().await.take();
        finish_document_audio_capture(aborted_id.as_deref(), live_capture.as_deref());
        *session.assistive_context.write().await = None;
        *session.pre_overlay_frontmost_app.write().await = None;
        set_assistive_session(false);
        settle_failed_start_chrome();
    }

    /// Unwind session state after a start that never produced a recording, so a
    /// failed start leaves nothing behind for the next hotkey press.
    async fn reset_session_after_start_failure(&self, context: &str) {
        warn!("{context}: resetting controller flags after failed start");
        *self.active_presentation.write().await = None;
        *self.pre_overlay_frontmost_app.write().await = None;
        self.reset_session_fields(TranscriptSessionEndReason::StartFailed)
            .await;
        set_assistive_session(false);
        settle_failed_start_chrome();
    }

    /// Unwind session state after a recording that completed. Telemetry is kept
    /// (unlike the start-failure path) — the finished session's stats are still
    /// being read by the result handler.
    async fn reset_finished_recording_state(&self, result: &Result<ProcessRecordingOutcome>) {
        #[cfg(test)]
        self.observe_capture_settlement(CaptureSettlementStage::Resetting);
        let reason = match result {
            Ok(outcome) if outcome.refusal.is_some() => TranscriptSessionEndReason::CoverageRefused,
            Ok(_) => TranscriptSessionEndReason::Completed,
            Err(error) if error.is::<TerminalSealRefused>() => {
                TranscriptSessionEndReason::CoverageRefusedEmpty
            }
            Err(error) if error.is::<StopDeliveryFailure>() => {
                TranscriptSessionEndReason::DeliveryFailed
            }
            Err(_) => TranscriptSessionEndReason::TranscriptionFailed,
        };
        self.reset_session_fields(reason).await;
        set_assistive_session(false);
    }

    /// Post-pipeline epilogue: log the commit decision on success, and surface a
    /// failure to the user instead of leaving it in the log.
    ///
    /// On success it also hands freed pages back to the OS while the app is
    /// idle, so a long session does not accumulate footprint across recordings.
    /// A failure is routed through the engine `Warning` channel, which the
    /// bridge turns into a visible error and a tray state — a silently failed
    /// transcription reads to the user as a lost recording.
    async fn handle_processed_recording_result(
        &self,
        assistive: bool,
        result: &Result<ProcessRecordingOutcome>,
    ) {
        match result {
            Ok(outcome) if outcome.refusal.is_some() => {
                self.publish_stop_warning(
                    "terminal_coverage_refused",
                    "Recording stopped. Available words are retained, but the transcript has no current terminal seal. Destination acceptance is reported separately.".to_string(),
                );
            }
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
                    let cfg = self.get_config().await;

                    if outcome.transcript_present
                        && cfg.transcription_overlay_enabled
                        && !(cfg.quick_notes_enabled && cfg.quick_notes_save_only)
                    {
                        let reason = outcome
                            .commit_trigger
                            .as_deref()
                            .unwrap_or("transcript_present");
                        info!("COMMIT decision: trigger={reason}");
                    } else if cfg.quick_notes_enabled && cfg.quick_notes_save_only {
                        info!("COMMIT decision: skipped (quick_notes_save_only)");
                    } else {
                        info!("COMMIT decision: skipped (delivery conditions not met)");
                    }
                }
            }
            Err(e) => {
                error!("Processing failed: {}", e);
                if let Some(failure) = e.downcast_ref::<StopDeliveryFailure>() {
                    let message = if failure.refusal.is_some() {
                        format!("The transcript has no current terminal seal. {failure}")
                    } else {
                        failure.to_string()
                    };
                    // Preserve the existing bridge's terminal-error allowlist.
                    // The Bus reason identifies delivery failure independently.
                    self.publish_stop_warning("transcription_failed", message);
                    return;
                }
                if e.is::<TerminalSealRefused>() {
                    self.publish_stop_warning(
                        "transcription_failed",
                        "Recording stopped without a current terminal seal or committed words. Retained audio may be used for recovery.".to_string(),
                    );
                    return;
                }
                // Surface the failure to the user instead of leaving it as a
                // log-only event. Reuse the existing engine `Warning` channel:
                // the bridge forwarder (forward_event_to_listener) turns it into
                // `listener.on_error(...)` + a tray Error state, so the SwiftUI
                // surface reflects the failed transcription.
                let _ = self.event_broadcast.send(IpcEvent {
                    timestamp: chrono::Utc::now()
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                    payload: IpcEventPayload::Engine(EngineEventWire::Warning {
                        code: "transcription_failed".to_string(),
                        message: format!("Transcription failed: {e}"),
                    }),
                });
            }
        }
    }

    fn publish_stop_warning(&self, code: &str, message: String) {
        let _ = self.event_broadcast.send(IpcEvent {
            timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            payload: IpcEventPayload::Engine(EngineEventWire::Warning {
                code: code.to_string(),
                message,
            }),
        });
    }

    /// Recognize the recorder's "already in progress" refusal, which is the one
    /// start failure worth a single force-stop-and-retry rather than an abort.
    fn is_already_in_progress_error(error: &anyhow::Error) -> bool {
        error
            .to_string()
            .contains("Recording is already in progress")
    }

    /// Reconcile a controller that believes it is `Idle` with a recorder that is
    /// still streaming, by force-stopping the orphaned stream.
    ///
    /// The in-flight start flag is checked twice — before and after taking the
    /// serial lock — because a session that is mid-start legitimately looks like
    /// "Idle plus an active recorder", and killing it there would turn recovery
    /// into the very bug it exists to fix.
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
        if recorder.has_non_take_subscriber() {
            return;
        }
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
        let ended_id = self.session_id.write().await.take();
        finish_document_audio_capture(ended_id.as_deref(), None);
        self.assistive_loop_active.store(false, Ordering::SeqCst);
        self.toggle_user_has_text.store(false, Ordering::SeqCst);
        self.toggle_assistant_has_text
            .store(false, Ordering::SeqCst);
        set_assistive_session(false);
        info!("RECOVERY decision: stale active stream cleared, controller remains IDLE");
    }

    /// Fan one pipeline event stream out to the three consumers a session needs:
    /// the presentation emitter (transcript assembly, optional preview deltas),
    /// the IPC broadcast, and session telemetry.
    ///
    /// Preview deltas are wired only when something is actually watching them;
    /// otherwise the delta sink is absent rather than emitting into the void.
    fn build_recording_event_sink(
        transcript_buffer: Arc<tokio::sync::Mutex<String>>,
        options: RecordingEventSinkOptions,
        event_broadcast: broadcast::Sender<IpcEvent>,
        transcript_bus: Option<Arc<TranscriptBus>>,
        acoustic_ledger: Option<
            Arc<std::sync::Mutex<codescribe_core::pipeline::acoustic_ledger::AcousticLedger>>,
        >,
        delivery_tagger: Arc<TranscriptDeliveryTagger>,
    ) -> RecordingEventPipeline {
        let delta_sink = options.preview_deltas_enabled.then(|| {
            Arc::new(helpers::RoutingDeltaSink)
                as Arc<dyn codescribe_core::pipeline::contracts::DeltaSink>
        });
        let projection_broadcast = event_broadcast.clone();
        let projection_callback = Arc::new(
            move |event: &crate::presentation::transcript_bus::TranscriptBusEvidenceEvent| {
                Self::broadcast_transcript_projection(&projection_broadcast, event);
            },
        );
        let cursor_token = crate::os::hold_badge::take_token();
        let compact_broadcast = event_broadcast.clone();
        let emitter = PresentationEmitter::new_with_authority(
            transcript_buffer,
            delta_sink,
            None,
            transcript_bus,
            acoustic_ledger,
            Some(projection_callback),
        )
        .with_apple_preview_observer(Arc::new(move |text| {
            crate::os::hold_badge::update_transcript(cursor_token, text);
        }))
        .with_cursor_observer(Arc::new(move |projection| {
            crate::os::hold_badge::update_voice_state(cursor_token, projection.voice);
            crate::os::tray_status::set_tray_voice_for_take(cursor_token, projection.voice);
            // The overlay retains the complete reducer projection; pointer
            // paint follows raw Apple partials through its separate observer.
            match serde_json::to_string(projection) {
                Ok(json) => {
                    let _ = compact_broadcast.send(IpcEvent {
                        timestamp: chrono::Utc::now()
                            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                        payload: IpcEventPayload::CompactProjection { json },
                    });
                }
                Err(error) => tracing::warn!(%error, "compact projection serialization failed"),
            }
        }))
        .with_sentence_pause_sec(options.sentence_pause_sec)
        .with_transcription_controls(options.seal_ledger_enabled, options.apple_live_enabled);
        let presentation = Arc::new(match options.speech_observer {
            Some(observer) => emitter.with_speech_activity_observer(observer),
            None => emitter,
        });
        let ipc_sink: Arc<dyn codescribe_core::pipeline::contracts::EventSink> =
            Arc::new(helpers::IpcBroadcastSink::new(event_broadcast));
        let delivery_tag_sink: Arc<dyn codescribe_core::pipeline::contracts::EventSink> =
            delivery_tagger;
        let stop_receipt = Arc::new(std::sync::Mutex::new(LateFinalReceipt::default()));
        let event_sink = Arc::new(TakeExternalEventSink {
            stop_receipt: Arc::clone(&stop_receipt),
            presentation: Arc::clone(&presentation),
            sink: Arc::new(codescribe_core::pipeline::sinks::FanoutEventSink::new(
                vec![ipc_sink, delivery_tag_sink],
            )),
        });
        RecordingEventPipeline {
            stop_receipt,
            event_sink,
            presentation,
        }
    }

    /// Send the sole typed transcript projection over the existing IPC event.
    /// Reducer revisions arrive through `PresentationEmitter`; the one terminal
    /// value arrives only after `TranscriptBus::publish_ended` has read the last
    /// committed book render.
    fn broadcast_transcript_projection(
        event_broadcast: &broadcast::Sender<IpcEvent>,
        event: &crate::presentation::transcript_bus::TranscriptBusEvidenceEvent,
    ) {
        match serde_json::to_string(event) {
            Ok(json) => {
                let _ = event_broadcast.send(IpcEvent {
                    timestamp: chrono::Utc::now()
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                    payload: IpcEventPayload::TranscriptProjection { json },
                });
            }
            Err(error) => {
                tracing::warn!(%error, "transcript projection serialization failed");
            }
        }
    }

    /// Feed the overlay's live level meter without doing allocation or timestamp
    /// formatting on CoreAudio's capture thread. The callback only attempts a
    /// bounded, non-blocking send; the controller worker constructs and
    /// broadcasts the typed IPC event. When the worker is behind, the new sample
    /// is dropped instead of growing a queue or delaying audio capture.
    fn configure_level_broadcast(
        recorder: &mut StreamingRecorder,
        event_broadcast: broadcast::Sender<IpcEvent>,
    ) {
        // The physical callback already captured its tap. Joining that stream
        // must neither replace the retained sender nor create another worker.
        if recorder.recorder.is_active() {
            return;
        }
        let (level_tx, mut level_rx) = mpsc::channel::<f32>(AUDIO_LEVEL_QUEUE_CAPACITY);
        recorder.set_level_callback(Some(Arc::new(move |rms| {
            let _ = level_tx.try_send(rms);
        })));

        tokio::spawn(async move {
            // Voice Lab feed: the Lab's bound lane watches Codescribe's own
            // capture instead of opening a second browser microphone. One
            // failed POST disarms the feed for this broadcast session — a
            // machine without a running Lab pays a single refused connection.
            // The slot is reserved atomically (armed → in-flight) before a
            // POST is spawned, so a stalled relay sees exactly one attempt:
            // batches that mature while a request is in flight are dropped,
            // never queued. `.no_proxy()` keeps the loopback payload off any
            // system HTTP proxy — this client talks to 127.0.0.1 or nobody.
            const LAB_FEED_DISARMED: u8 = 0;
            const LAB_FEED_ARMED: u8 = 1;
            const LAB_FEED_IN_FLIGHT: u8 = 2;
            let lab_state = Arc::new(AtomicU8::new(LAB_FEED_ARMED));
            let lab_client = reqwest::Client::builder()
                .no_proxy()
                .timeout(std::time::Duration::from_millis(400))
                .build()
                .ok();
            let mut lab_batcher = lab_feed::LabFeedBatcher::new(std::time::Instant::now());
            let lab_frames_url = lab_feed::lab_frames_url();
            while let Some(rms) = level_rx.recv().await {
                // Cleanup drops the callback sender. Do not drain a buffered
                // sample after that boundary: it belongs to the closed session
                // and must never animate a subsequently prepared overlay.
                if level_rx.is_closed() {
                    break;
                }
                let _ = event_broadcast.send(IpcEvent {
                    timestamp: chrono::Utc::now()
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                    payload: IpcEventPayload::AudioLevel { rms },
                });
                if let Some(client) = &lab_client
                    && let Some(batch) = lab_batcher.offer(rms, std::time::Instant::now())
                    && lab_state
                        .compare_exchange(
                            LAB_FEED_ARMED,
                            LAB_FEED_IN_FLIGHT,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok()
                {
                    let payload = lab_feed::frames_payload(&batch);
                    let client = client.clone();
                    let state = Arc::clone(&lab_state);
                    let url = lab_frames_url.clone();
                    tokio::spawn(async move {
                        let delivered = client
                            .post(url)
                            .json(&payload)
                            .send()
                            .await
                            .map(|resp| resp.status().is_success())
                            .unwrap_or(false);
                        state.store(
                            if delivered {
                                LAB_FEED_ARMED
                            } else {
                                LAB_FEED_DISARMED
                            },
                            Ordering::Release,
                        );
                    });
                }
            }
        });
    }

    /// Wire level metering and the event sink for a hold session. Hold has no
    /// utterance callback: text is finalized on key-up in `finish_recording`.
    fn configure_hold_event_sink(
        recorder: &mut StreamingRecorder,
        preview_deltas_enabled: bool,
        event_broadcast: broadcast::Sender<IpcEvent>,
        transcript_bus: Option<Arc<TranscriptBus>>,
        delivery_tagger: Arc<TranscriptDeliveryTagger>,
        speech_observer: Option<crate::presentation::emitter::SpeechActivityObserver>,
    ) -> RecordingEventPipeline {
        Self::configure_level_broadcast(recorder, event_broadcast.clone());
        let acoustic_ledger = recorder.acoustic_ledger_handle();
        let pipeline = Self::build_recording_event_sink(
            recorder.transcript_buffer_handle(),
            RecordingEventSinkOptions {
                preview_deltas_enabled,
                sentence_pause_sec: recorder.light_plus_sentence_pause_sec(),
                seal_ledger_enabled: recorder.seal_ledger_enabled(),
                apple_live_enabled: recorder.apple_live_enabled(),
                speech_observer,
            },
            event_broadcast,
            transcript_bus,
            acoustic_ledger,
            delivery_tagger,
        );
        recorder.set_event_sink(Some(Arc::clone(&pipeline.event_sink)));
        pipeline
    }

    /// Wire level metering and the event sink for a toggle / hands-off session,
    /// which is ONE continuous recorder session (ADR 2026-05-28 Faza 1).
    fn configure_toggle_event_sink(
        recorder: &mut StreamingRecorder,
        preview_deltas_enabled: bool,
        speech_observer: Option<crate::presentation::emitter::SpeechActivityObserver>,
        event_broadcast: broadcast::Sender<IpcEvent>,
        transcript_bus: Option<Arc<TranscriptBus>>,
        delivery_tagger: Arc<TranscriptDeliveryTagger>,
    ) -> RecordingEventPipeline {
        // Hands-off is ONE continuous recorder session (ADR 2026-05-28 Faza 1).
        // Normal hands-off uses cumulative SessionRendered deltas in the transcription overlay.
        //
        // Assistive hands-off is intentionally callback-driven: every finalized utterance
        // appends into the current chat user bubble, and VAD end commits that bubble to the
        // agent without stopping the recorder. Do not route assistive live preview deltas
        // into the same bubble, or previews and finals will duplicate. This sink does not
        // perform that routing: callers pass `preview_deltas_enabled`. A hands-free
        // assistive take also passes the observer its silence watch reads.
        Self::configure_hold_event_sink(
            recorder,
            preview_deltas_enabled,
            event_broadcast,
            transcript_bus,
            delivery_tagger,
            speech_observer,
        )
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
    /// - **Hold + assistive=false**: force RAW mode (ignores FORMATTING_LEVEL)
    /// - **Hold + assistive=true**: force Assistive mode (Shift pressed = AI augmentation)
    /// - **Toggle + force_ai=true**: force AI formatting (normal hands-off)
    /// - **Toggle + assistive=true**: force Assistive hands-off
    fn hotkey_stop_decision(&self, event: &HotkeyInput) -> Result<(State, bool)> {
        let state = {
            let Ok(state) = self.state.try_read() else {
                return Err(anyhow::anyhow!("hotkey admission unavailable"));
            };
            *state
        };
        let stop = matches!(
            (event.key_type, event.action, state),
            (
                HotkeyType::Hold,
                HotkeyAction::Up,
                State::RecHold | State::Busy
            ) | (
                HotkeyType::Toggle,
                HotkeyAction::Press,
                State::RecToggle | State::Busy
            )
        ) || (event.key_type == HotkeyType::Toggle
            && event.action == HotkeyAction::Press
            && event.force_raw
            && state == State::RecHold);
        Ok((state, stop))
    }

    /// Admit each native gesture in FIFO order. Stop returns only after its
    /// PCM feed is closed and capture locks are released; terminal work remains
    /// owned by the same settlement task. Explicit stop APIs await completion.
    pub async fn admit_hotkey_event(self: &Arc<Self>, event: HotkeyInput) -> Result<()> {
        let (state, stop) = self.hotkey_stop_decision(&event)?;
        info!("{}", event.decision_line(Some(state), stop));
        if !stop {
            return self.handle_hotkey_event(event).await;
        }
        match self.current_capture_stop() {
            CaptureStopAdmission::Immediate(CaptureStopOutcome::NoLiveCapture) => Ok(()),
            CaptureStopAdmission::Immediate(outcome) => {
                Err(anyhow::anyhow!("Stop admission unresolved: {outcome:?}"))
            }
            CaptureStopAdmission::Settling(result, closed) => {
                Self::await_admitted_stop(result, closed).await
            }
        }
    }

    /// Return at the capture-close receipt or at the settled result,
    /// whichever comes first. A settlement that never closes a capture (gate
    /// refusal, no live take, not recording) drops its receipt sender without
    /// sending; its result still arrives and is the outcome, not an error.
    async fn await_admitted_stop(
        mut result: watch::Receiver<Option<CaptureSettlementResult>>,
        mut closed: watch::Receiver<bool>,
    ) -> Result<()> {
        let mut receipt_pending = true;
        loop {
            if receipt_pending && *closed.borrow_and_update() {
                return Ok(());
            }
            if let Some(settled) = result.borrow_and_update().clone() {
                return match settled.map_err(anyhow::Error::msg)? {
                    CaptureStopOutcome::Stopped | CaptureStopOutcome::NoLiveCapture => Ok(()),
                    outcome => Err(anyhow::anyhow!("Stop admission unresolved: {outcome:?}")),
                };
            }
            if receipt_pending {
                tokio::select! {
                    receipt = closed.changed() => {
                        receipt_pending = receipt.is_ok();
                    }
                    change = result.changed() => {
                        change.map_err(|_| anyhow::anyhow!("capture settlement owner lost"))?;
                    }
                }
            } else {
                result
                    .changed()
                    .await
                    .map_err(|_| anyhow::anyhow!("capture settlement owner lost"))?;
            }
        }
    }

    pub async fn handle_hotkey_event(self: &Arc<Self>, event: HotkeyInput) -> Result<()> {
        let hold_generation = (event.key_type == HotkeyType::Hold
            && event.action == HotkeyAction::Down)
            .then(|| self.hold_start_generation.load(Ordering::SeqCst));
        // Stop gestures enter before mode updates: a RAW toggle during hold
        // must not rewrite the take's destination while asking to end it.
        let (_, stop_gesture) = self.hotkey_stop_decision(&event)?;
        if stop_gesture {
            return self.stop_recording_from_external_surface().await;
        }
        let next_start = matches!(
            (event.key_type, event.action),
            (HotkeyType::Hold, HotkeyAction::Down) | (HotkeyType::Toggle, HotkeyAction::Press)
        );
        if next_start && preempt_stop_paste_for_next_take() {
            // The old owner copies and retires before this gesture may open a
            // microphone or capture a new target. Its slow drain is detached.
            let result = self
                .capture_settlement
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_ref()
                .map(|operation| operation.result.clone());
            if let Some(result) = result {
                anyhow::ensure!(
                    matches!(
                        Self::await_capture_stop(result).await?,
                        CaptureStopOutcome::Stopped | CaptureStopOutcome::NoLiveCapture
                    ),
                    "preceding take has not retired its foreground"
                );
            }
        }
        let mut current_state = self.current_state().await;

        if current_state == State::Idle
            && event.key_type == HotkeyType::Hold
            && event.action == HotkeyAction::Up
        {
            return self.handle_hold_event(event, None).await;
        }

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

        if current_state == State::Idle
            && event.key_type == HotkeyType::Hold
            && matches!(event.action, HotkeyAction::Down)
        {
            // Leftovers from a previous session are archived, never destroyed
            // (operator law 2026-07-21: reproducible moment-of-truth in store).
            match self
                .context_bucket
                .lock()
                .await
                .archive_and_reset("session-start-discard")
            {
                Ok(Some(dir)) => info!("Context bucket archived: {}", dir.display()),
                Ok(None) => {}
                Err(err) => warn!("Context bucket archive failed (items kept): {err:#}"),
            }
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
                            let prior_frontmost_app =
                                self.pre_overlay_frontmost_app.read().await.clone();
                            let ctx = self
                                .capture_assistive_combo_context(
                                    current_state,
                                    prior_frontmost_app,
                                    true,
                                )
                                .await;
                            *self.assistive_context.write().await = Some(ctx);

                            if matches!(current_state, State::RecHold | State::RecToggle) {
                                publish_recording_indicator(
                                    BadgeMode::Assistive,
                                    self.get_config().await.hold_indicator,
                                );
                            }
                        }
                        HoldMode::Selection => {
                            *self.assistive_mode.write().await = true;
                            *self.force_raw_mode.write().await = false;
                            *self.force_ai_mode.write().await = false;
                            let prior_frontmost_app =
                                self.pre_overlay_frontmost_app.read().await.clone();
                            let ctx = self
                                .capture_assistive_combo_context(
                                    current_state,
                                    prior_frontmost_app,
                                    true,
                                )
                                .await;
                            *self.assistive_context.write().await = Some(ctx);

                            if matches!(current_state, State::RecHold | State::RecToggle) {
                                publish_recording_indicator(
                                    BadgeMode::Assistive,
                                    self.get_config().await.hold_indicator,
                                );
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
        if hold_generation.is_some_and(|generation| {
            self.hold_start_generation.load(Ordering::SeqCst) != generation
        }) {
            return Ok(());
        }
        match event.key_type {
            HotkeyType::Hold => self.handle_hold_event(event, hold_generation).await,
            HotkeyType::Toggle => self.handle_toggle_event(event).await,
            HotkeyType::Conversation => self.handle_conversation_event(event).await,
        }
    }

    /// Handle hold-type hotkey events
    async fn handle_hold_event(
        self: &Arc<Self>,
        event: HotkeyInput,
        hold_generation: Option<u64>,
    ) -> Result<()> {
        match event.action {
            HotkeyAction::Down => {
                let current_state = self.current_state().await;
                if current_state == State::Idle {
                    let Some(generation) = hold_generation else {
                        return Ok(());
                    };
                    self.schedule_hold_start(event.assistive, generation)
                        .await?;
                    // Fn down with a live OS selection attaches `{selection_1}`
                    // immediately. Mid-hold arm pulses add `{selection_2..n}`.
                    // Destination stays dictation — do not arm Chat/Agent.
                    if !event.assistive && matches!(event.hold_mode, HoldMode::Raw) {
                        self.attach_hold_selection().await?;
                    }
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
    async fn handle_toggle_event(self: &Arc<Self>, event: HotkeyInput) -> Result<()> {
        if event.action != HotkeyAction::Press {
            return Ok(());
        }

        let current_state = self.current_state().await;

        match current_state {
            State::Idle => {
                let admission = self
                    .start_toggle_recording(event.assistive, CaptureTurnIntent::HandsFree)
                    .await?;
                if let CaptureAdmission::Admitted(take) = admission {
                    self.watch_assistive_silence(take);
                }
            }
            State::RecToggle => {
                info!("Toggle pressed; entering stop flow (state=REC_TOGGLE)");
                self.assistive_loop_active.store(false, Ordering::SeqCst);
                self.stop_toggle_and_adjudicate().await?;
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
                self.stop_recording_from_external_surface().await?;
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

    /// True when no agent channel blocks a single-capture surface such as
    /// conversation mode or energy calibration.
    ///
    /// Lock order: this method acquires `agent_channels` only; callers that
    /// also need the recorder lock must acquire `agent_channels` first.
    async fn ensure_exclusive_capture_available(&self) -> Result<()> {
        let channels = self.agent_channels.lock().await;
        if channels.is_empty() {
            Ok(())
        } else {
            let digits: Vec<u8> = channels.keys().copied().collect();
            Err(anyhow::anyhow!(
                "capture admission refused: agent channels active on digits {digits:?}"
            ))
        }
    }

    /// Start conversation mode (full-duplex Moshi)
    ///
    /// Initializes ConversationEngine and AudioPlayer, then starts the audio
    /// processing loop that feeds mic input to Moshi and plays responses.
    async fn start_conversation_mode(&self) -> Result<()> {
        self.refuse_exclusive_while_agent_channel_open("conversation")
            .await?;
        let _serial = self.serial_lock.lock().await;
        if self.shutdown_requested.load(Ordering::SeqCst)
            || self.current_state().await != State::Idle
        {
            return Err(anyhow::anyhow!(
                "conversation capture admission unavailable"
            ));
        }
        self.ensure_exclusive_capture_available().await?;
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

        let handles = ConversationLoopHandles {
            engine,
            player,
            recorder,
            stop_flag,
            generation_counter: generation_arc,
            state,
            event_broadcast,
        };

        let task = tokio::spawn(async move {
            Self::conversation_audio_loop(handles, generation).await;
        });

        *self.conversation_task.lock().await = Some(task);

        Ok(())
    }

    /// The main conversation audio processing loop
    ///
    /// Runs in a background task: captures audio → ConversationEngine → speaker
    async fn conversation_audio_loop(handles: ConversationLoopHandles, my_generation: u64) {
        let ConversationLoopHandles {
            engine,
            player,
            recorder,
            stop_flag,
            generation_counter,
            state,
            event_broadcast,
        } = handles;
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
                    // Full cleanup on failure: state and badge
                    Self::set_state_with_broadcast(&state, &event_broadcast, State::Idle).await;
                    codescribe_core::memory::release_freed_heap();
                    return;
                }
            };
            rec.recorder.set_callback(Box::new(move |data: &[f32]| {
                let _ = tx_clone.try_send(data.to_vec());
            }));

            if let Err(e) = rec.recorder.start().await {
                error!("Failed to start recorder for conversation: {}", e);
                // Full cleanup on failure: state and badge
                Self::set_state_with_broadcast(&state, &event_broadcast, State::Idle).await;
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
                        /// Clears `playback_active` on exit (unwind path; not panic=abort).
                        struct PlaybackGuard(Arc<AtomicBool>);
                        impl Drop for PlaybackGuard {
                            /// Clear the playback-active flag when play scope ends.
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
    async fn schedule_hold_start(
        self: &Arc<Self>,
        assistive: bool,
        requested_generation: u64,
    ) -> Result<()> {
        // Scheduling selects the take generation. Refresh and every actual
        // start/stop transition cross this same boundary, while the spawned
        // task itself is never awaited under the guard.
        let _scheduling_guard = self.serial_lock.lock().await;

        if self.shutdown_requested.load(Ordering::SeqCst) {
            return Err(anyhow::anyhow!("capture admission closed for shutdown"));
        }
        if self.hold_start_generation.load(Ordering::SeqCst) != requested_generation {
            return Ok(());
        }
        // Cancel any existing delayed start before selecting the next Arc.
        let task_generation = self.cancel_pending_hold_start().await;
        if task_generation != requested_generation.wrapping_add(1) {
            return Ok(());
        }
        self.refresh_pending_runtime_settings_locked().await?;
        // The take seals the automatic policy: a capture never runs Max.
        let runtime_settings = automatic_take_settings(self.runtime_settings_arc().await);
        let config = runtime_settings.values().clone();

        // Hold mode never runs the assistive loop
        self.assistive_loop_active.store(false, Ordering::SeqCst);
        let configured_delay_ms = config.hold_start_delay_ms;
        let delay_ms = effective_hold_start_delay_ms(configured_delay_ms, assistive);
        let beep = config.beep_on_start;
        let sound_volume = config.sound_volume;

        let hold_mode = Arc::clone(&self.hold_mode);

        debug!(
            "Scheduling hold-start after {}ms delay (configured={}ms, assistive={}, hold_mode={:?})",
            delay_ms,
            configured_delay_ms,
            assistive,
            *hold_mode.read().await
        );

        *self.pending_assistive_context.write().await = None;
        let initial_hold_mode = *hold_mode.read().await;
        let trigger_context = if matches!(initial_hold_mode, HoldMode::Chat | HoldMode::Selection) {
            self.assistive_context
                .read()
                .await
                .clone()
                .unwrap_or_default()
        } else {
            self.capture_paste_target_context().await
        };
        let has_latched_target = self.latch_trigger_context(trigger_context).await;

        // Reset VAD flag for new session
        self.vad_triggered.store(false, Ordering::SeqCst);

        let state = Arc::clone(&self.state);
        let recorder = Arc::clone(&self.recorder);
        let delay = Duration::from_millis(delay_ms);
        let vad_flag = Arc::clone(&self.vad_triggered);
        let event_broadcast = self.event_broadcast.clone();
        let serial_lock = Arc::clone(&self.serial_lock);
        let hold_start_generation = Arc::clone(&self.hold_start_generation);
        let shutdown_requested = Arc::clone(&self.shutdown_requested);
        let start_transition_in_flight = Arc::clone(&self.start_transition_in_flight);
        let controller = Arc::clone(self);
        // Every exit after the start guard releases exactly these slots.
        let hold_session = HoldStartSession {
            session_id: Arc::clone(&self.session_id),
            active_transcript_bus: Arc::clone(&self.active_transcript_bus),
            active_presentation: Arc::clone(&self.active_presentation),
            active_stop_receipt: Arc::clone(&self.active_stop_receipt),
            assistive_context: Arc::clone(&self.assistive_context),
            pre_overlay_frontmost_app: Arc::clone(&self.pre_overlay_frontmost_app),
            event_broadcast: event_broadcast.clone(),
            force_raw_mode: Arc::clone(&self.force_raw_mode),
            delivery_tagger: Arc::clone(&self.delivery_tagger),
            composer_delivery_payload: Arc::clone(&self.composer_delivery_payload),
        };

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

            if shutdown_requested.load(Ordering::SeqCst) {
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

            // The caret sits in one of Codescribe's own message fields. That
            // field records through its own microphone; global dictation would
            // steal focus to the overlay and deliver elsewhere. Stay idle and
            // let the field show the person where its microphone is.
            if hold_start_redirects_to_composer(
                assistive,
                *hold_mode.read().await,
                hotkeys::composer_caret_owned(),
            ) {
                info!("Hold-start refused: a Codescribe composer owns the caret");
                let _ = event_broadcast.send(IpcEvent {
                    timestamp: chrono::Utc::now()
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                    payload: IpcEventPayload::ComposerDictationRedirected,
                });
                return;
            }

            let _start_guard = AtomicFlagGuard::new(Arc::clone(&start_transition_in_flight));

            // Generate session ID
            let new_session_id = Uuid::new_v4().to_string();
            let shared = if !assistive && matches!(*hold_mode.read().await, HoldMode::Raw) {
                recorder
                    .lock()
                    .await
                    .as_ref()
                    .and_then(StreamingRecorder::shared_channel_view)
            } else {
                None
            };
            let runtime_settings = shared
                .as_ref()
                .map(|view| Arc::clone(&view.runtime_settings))
                .unwrap_or(runtime_settings);
            let config = runtime_settings.values().clone();
            let language = config.whisper_language;
            let capture_session_id =
                if !assistive && matches!(*hold_mode.read().await, HoldMode::Raw) {
                    StreamingRecorder::dictation_capture_session_id(&new_session_id)
                } else {
                    new_session_id.clone()
                };
            if shared.is_none()
                && let Err(error) =
                    begin_audio_capture(&capture_session_id, config.audio_retention).await
            {
                error!(%error, "capture storage lease refused; microphone remains closed");
                return;
            }
            *hold_session.session_id.write().await = Some(new_session_id.clone());

            info!("Starting hold recording (session={})", new_session_id);

            let hold_mode = *hold_mode.read().await;
            let is_assistive = matches!(hold_mode, HoldMode::Chat | HoldMode::Selection);
            let overlay_enabled = apply_runtime_transcription_profile(
                &config,
                runtime_settings.user_settings(),
                is_assistive,
            );

            // Speech readiness never owns audio admission. The live session
            // starts behind capture; its failure cannot erase the requested take.
            // Only microphone/device blockers below refuse the physical capture.
            // The cursor badge is published after this verdict, so a refusal
            // never leaves a recording indicator up.
            let admission_verdict = match controller.installed_start_admission() {
                Some(Ok(())) => {
                    info!("acoustic admission granted for hold start (instance test seam)");
                    None
                }
                Some(Err(blocker)) => Some(Err(blocker)),
                None if cfg!(test) => None,
                None => {
                    // `.clone()` (not `Arc::clone`) on purpose: C15D counts one
                    // recorder binding per start body; this is the same Arc.
                    let admission_settings = runtime_settings.clone();
                    Some(
                        tokio::task::spawn_blocking(move || {
                            admission::evaluate_live_admission_arc(&admission_settings)
                        })
                        .await
                        .unwrap_or_else(|join| {
                            Err(admission::AdmissionBlocker::CaptureDeviceUnavailable {
                                reason: format!("admission probe panicked: {join}"),
                            })
                        }),
                    )
                }
            };
            if let Some(verdict) = admission_verdict {
                match verdict {
                    Ok(grant) => info!(
                        device = %grant.device_name,
                        sample_rate = grant.sample_rate,
                        calibration_version = ?grant.calibration_version,
                        "acoustic admission granted for hold start"
                    ),
                    Err(blocker)
                        if !matches!(
                            blocker,
                            admission::AdmissionBlocker::MicrophonePermissionUnavailable { .. }
                                | admission::AdmissionBlocker::CaptureDeviceUnavailable { .. }
                        ) =>
                    {
                        Self::broadcast_capture_transcription_warning(&event_broadcast, &blocker);
                    }
                    Err(blocker) => {
                        error!("Hold-start refused (acoustic admission): {blocker}");
                        Self::broadcast_admission_refusal(
                            &event_broadcast,
                            Some(new_session_id.clone()),
                            &blocker,
                        );
                        Self::unwind_hold_start(
                            &hold_session,
                            None,
                            HoldStartAbort::AdmissionRefused,
                        )
                        .await;
                        return;
                    }
                }
            }

            // Cursor-following recording badge (config-gated): red for hold dictation,
            // purple for assistive/agent. Works headless — no overlay needed.
            publish_recording_indicator(
                if is_assistive {
                    BadgeMode::Assistive
                } else {
                    BadgeMode::Hold
                },
                config.hold_indicator,
            );

            // Start the recorder (skip in tests: no CoreAudio device needed)
            // hang_sec is derived from hardcoded VAD defaults (single source of truth).
            let mut rec_guard = recorder.lock().await;
            let rec = match Self::recorder_from_guard_mut(&mut rec_guard, "Hold-start") {
                Ok(rec) => rec,
                Err(error) => {
                    error!("Hold-start aborted: {error}");
                    drop(rec_guard);
                    Self::unwind_hold_start(
                        &hold_session,
                        None,
                        HoldStartAbort::RecorderUnavailable,
                    )
                    .await;
                    return;
                }
            };
            if let Err(e) = Self::ensure_recorder_ready_for_start(rec, "Hold-start preflight").await
            {
                error!("Hold-start aborted: {e}");
                Self::unwind_hold_start(
                    &hold_session,
                    Some(rec),
                    HoldStartAbort::RecorderUnavailable,
                )
                .await;
                return;
            }
            // Hold-to-talk: the key-down is the source of truth. Don't auto-stop
            // the session mid-hold. Silence still closes an SFSpeech epoch so
            // Layer 1 can be fed — same knob as toggle (`TOGGLE_SILENCE_SEC`).
            rec.recorder.config.auto_silence = false;
            rec.set_utterance_silence_sec(Some(config.toggle_silence_sec));
            rec.recorder.set_on_vad_stop(move || {
                info!("VAD callback: setting vad_triggered flag");
                vad_flag.store(true, Ordering::SeqCst);
            });

            // Set session mode for delta routing BEFORE starting the pipeline,
            // so the very first deltas route to the correct overlay.
            set_assistive_session(is_assistive);
            // `hold_mode` is the value read above, not the lock.
            if !is_assistive && matches!(hold_mode, HoldMode::Raw) {
                rec.bind_dictation_authority(new_session_id.clone(), Arc::clone(&runtime_settings));
            } else {
                rec.bind_session_authority(new_session_id.clone(), Arc::clone(&runtime_settings));
            }
            let capture_session_id = rec
                .bound_capture_session_id()
                .unwrap_or(&new_session_id)
                .to_string();
            hold_session.delivery_tagger.begin(
                if is_assistive {
                    "assistive"
                } else {
                    "dictation"
                },
                config.whisper_language.as_str(),
            );
            *hold_session.composer_delivery_payload.write().await = None;
            let transcript_bus = TranscriptBus::open(TranscriptSession {
                session_id: new_session_id,
                mode: if is_assistive {
                    TranscriptMode::Assistive
                } else {
                    TranscriptMode::Dictation
                },
                has_latched_target,
                // The capture-time target predates the overlay caret. A true
                // self-canvas fact exists only at the explicit defer click.
                latched_target_is_self: false,
                audience: None,
                badge_only: false,
            })
            .map(|bus| Arc::new(bus.with_capture_session(capture_session_id)));
            // Install the Bus before the recorder starts: from here every exit
            // runs through `unwind_hold_start`, which ends whatever is in this
            // slot — silently while un-started, with one terminal line once
            // `publish_started` has been written.
            *hold_session.active_transcript_bus.write().await = transcript_bus.clone();

            // Runtime pipeline is always event-based. Hold mode has no utterance callback;
            // text is finalized on key-up in `finish_recording`.
            let pipeline = Self::configure_hold_event_sink(
                rec,
                is_assistive || overlay_enabled,
                event_broadcast.clone(),
                transcript_bus.clone(),
                Arc::clone(&hold_session.delivery_tagger),
                None,
            );
            pipeline
                .presentation
                .set_literal_delivery(*hold_session.force_raw_mode.read().await);
            *hold_session.active_stop_receipt.write().await = pipeline.stop_receipt;
            *hold_session.active_presentation.write().await = Some(pipeline.presentation);
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
                        let pipeline = Self::configure_hold_event_sink(
                            rec,
                            is_assistive || overlay_enabled,
                            event_broadcast.clone(),
                            transcript_bus.clone(),
                            Arc::clone(&hold_session.delivery_tagger),
                            None,
                        );
                        pipeline
                            .presentation
                            .set_literal_delivery(*hold_session.force_raw_mode.read().await);
                        *hold_session.active_stop_receipt.write().await = pipeline.stop_receipt;
                        *hold_session.active_presentation.write().await =
                            Some(pipeline.presentation);
                        let retry_result = rec.start_event_session(language_hint).await;
                        if let Err(retry_err) = retry_result {
                            error!("Failed to start recorder after recovery: {retry_err}");
                            Self::unwind_hold_start(
                                &hold_session,
                                Some(rec),
                                HoldStartAbort::RecorderStartFailed,
                            )
                            .await;
                            return;
                        }
                    } else {
                        error!("Failed to start recorder: {e}");
                        Self::unwind_hold_start(
                            &hold_session,
                            Some(rec),
                            HoldStartAbort::RecorderStartFailed,
                        )
                        .await;
                        return;
                    }
                }
            }

            if let Some(bus) = &transcript_bus {
                bus.publish_started();
            }

            // A key-up that landed while `state` was still Idle bumped the
            // generation without `serial_lock`; the take must not survive it.
            // The Bus already carries `session_started`, so this exit is the
            // one that writes its terminal line.
            if hold_start_generation.load(Ordering::SeqCst) != task_generation {
                warn!("Hold-start superseded after recorder start; stopping stale session");
                Self::unwind_hold_start(&hold_session, Some(rec), HoldStartAbort::Superseded).await;
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
        });

        *self.hold_start_task.lock().await = Some(task);
        Ok(())
    }

    /// Start recording in toggle mode (immediate, no delay)
    ///
    /// `capture_turn` is the per-take intent of the surface that opened the
    /// microphone. Hotkey, tray and overlay pass
    /// [`CaptureTurnIntent::HandsFree`] and keep their utterance-epoch
    /// contract; the Agent composer passes [`CaptureTurnIntent::SingleTurn`].
    async fn start_toggle_recording(
        &self,
        is_assistive: bool,
        capture_turn: CaptureTurnIntent,
    ) -> Result<CaptureAdmission> {
        preempt_stop_paste_for_next_take();
        // Acquire serial lock to prevent race conditions
        let _guard = self.serial_lock.lock().await;

        if self.shutdown_requested.load(Ordering::SeqCst) {
            return Ok(CaptureAdmission::NotAdmitted);
        }
        // Double-check state under lock
        let current_state = *self.state.read().await;
        if current_state != State::Idle {
            debug!(
                "start_toggle_recording: state already changed to {}",
                current_state
            );
            return Ok(CaptureAdmission::NotAdmitted);
        }
        // A new take inherits no destination from the previous one.
        *self.delivery_disposition.write().await = TranscriptDelivery::Unattempted;
        self.refresh_pending_runtime_settings_locked().await?;
        // The take seals the automatic policy: a capture never runs Max.
        let runtime_settings = automatic_take_settings(self.runtime_settings_arc().await);
        let shared = if !is_assistive && matches!(capture_turn, CaptureTurnIntent::HandsFree) {
            self.recorder
                .lock()
                .await
                .as_ref()
                .and_then(StreamingRecorder::shared_channel_view)
        } else {
            None
        };
        let runtime_settings = shared
            .as_ref()
            .map(|view| Arc::clone(&view.runtime_settings))
            .unwrap_or(runtime_settings);
        let config = runtime_settings.values();
        let _start_guard = AtomicFlagGuard::new(Arc::clone(&self.start_transition_in_flight));

        *self.pending_assistive_context.write().await = None;
        match self
            .context_bucket
            .lock()
            .await
            .archive_and_reset("session-start-discard")
        {
            Ok(Some(dir)) => info!("Context bucket archived: {}", dir.display()),
            Ok(None) => {}
            Err(err) => warn!("Context bucket archive failed (items kept): {err:#}"),
        }
        let trigger_context = if is_assistive {
            tokio::task::spawn_blocking(capture_assistive_context)
                .await
                .unwrap_or_default()
        } else {
            self.capture_paste_target_context().await
        };
        let has_latched_target = self.latch_trigger_context(trigger_context).await;

        // Generate session ID
        let new_session_id = Uuid::new_v4().to_string();
        let capture_session_id =
            if !is_assistive && matches!(capture_turn, CaptureTurnIntent::HandsFree) {
                StreamingRecorder::dictation_capture_session_id(&new_session_id)
            } else {
                new_session_id.clone()
            };
        if shared.is_none() {
            begin_audio_capture(&capture_session_id, config.audio_retention).await?;
        }
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

        let language = config.whisper_language;
        let toggle_silence_sec = config.toggle_silence_sec;
        let beep_enabled = config.beep_on_start;
        let sound_volume = config.sound_volume;
        let overlay_enabled = apply_runtime_transcription_profile(
            config,
            runtime_settings.user_settings(),
            is_assistive,
        );

        // Speech readiness never owns audio admission. Recognition starts
        // downstream of capture, with no Speech permission wait on this path.
        // Calibration/seal readiness is a warning; capture failures still refuse.
        // The cursor badge is published after this verdict.
        let admission_verdict = match self.installed_start_admission() {
            Some(Ok(())) => {
                info!("acoustic admission granted for toggle start (instance test seam)");
                None
            }
            Some(Err(blocker)) => Some(Err(blocker)),
            None if cfg!(test) => None,
            None => Some(self.admission_readiness().await),
        };
        if let Some(verdict) = admission_verdict {
            match verdict {
                Ok(grant) => info!(
                    device = %grant.device_name,
                    sample_rate = grant.sample_rate,
                    calibration_version = ?grant.calibration_version,
                    "acoustic admission granted for toggle start"
                ),
                Err(blocker)
                    if !matches!(
                        blocker,
                        admission::AdmissionBlocker::MicrophonePermissionUnavailable { .. }
                            | admission::AdmissionBlocker::CaptureDeviceUnavailable { .. }
                    ) =>
                {
                    Self::broadcast_capture_transcription_warning(&self.event_broadcast, &blocker);
                }
                Err(blocker) => {
                    error!("Toggle-start refused (acoustic admission): {blocker}");
                    Self::broadcast_admission_refusal(
                        &self.event_broadcast,
                        Some(new_session_id.clone()),
                        &blocker,
                    );
                    self.reset_session_after_start_failure("Toggle-start admission")
                        .await;
                    return Err(anyhow::anyhow!("{blocker}"));
                }
            }
        }

        // Cursor-following recording badge (config-gated): pulsing red for toggle /
        // hands-off, purple for assistive/agent.
        publish_recording_indicator(
            if is_assistive {
                BadgeMode::Assistive
            } else {
                BadgeMode::Toggle
            },
            config.hold_indicator,
        );

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
        // The intent owns the epoch question, not the mode flags: hands-free
        // keeps the configured threshold, a one-turn take asks for the legacy
        // single continuous stream. Silero, ledger qualification and Layer 1
        // tail repair are unaffected either way.
        recorder.set_utterance_silence_sec(capture_turn.utterance_silence_sec(toggle_silence_sec));
        recorder.set_capture_turn_intent(capture_turn);

        // Set session mode for delta routing BEFORE starting the pipeline,
        // so the very first deltas route to the correct overlay.
        set_assistive_session(is_assistive);
        if !is_assistive && matches!(capture_turn, CaptureTurnIntent::HandsFree) {
            recorder
                .bind_dictation_authority(new_session_id.clone(), Arc::clone(&runtime_settings));
        } else {
            recorder.bind_session_authority(new_session_id.clone(), Arc::clone(&runtime_settings));
        }
        let capture_session_id = recorder
            .bound_capture_session_id()
            .unwrap_or(&new_session_id)
            .to_string();
        self.delivery_tagger.begin(
            if is_assistive { "agent" } else { "dictation" },
            config.whisper_language.as_str(),
        );
        *self.composer_delivery_payload.write().await = None;
        let transcript_bus = TranscriptBus::open(TranscriptSession {
            session_id: new_session_id.clone(),
            mode: if is_assistive {
                TranscriptMode::Agent
            } else {
                TranscriptMode::Dictation
            },
            has_latched_target,
            // The capture-time target predates the overlay caret. A true
            // self-canvas fact exists only at the explicit defer click.
            latched_target_is_self: false,
            audience: None,
            badge_only: false,
        })
        .map(|bus| Arc::new(bus.with_capture_session(capture_session_id)));

        // A hands-free assistive take ends after its voiceless limit
        // (Settings › Built-in Agent); any other take clears the previous
        // take's watch.
        let speech_observer = self.arm_assistive_silence(
            &new_session_id,
            is_assistive && matches!(capture_turn, CaptureTurnIntent::HandsFree),
            config.agent_take_voiceless_end_sec,
        );
        // Runtime pipeline is always event-based.
        let pipeline = Self::configure_toggle_event_sink(
            recorder,
            overlay_enabled,
            speech_observer.clone(),
            self.event_broadcast.clone(),
            transcript_bus.clone(),
            Arc::clone(&self.delivery_tagger),
        );
        pipeline
            .presentation
            .set_literal_delivery(*self.force_raw_mode.read().await);
        *self.active_stop_receipt.write().await = pipeline.stop_receipt;
        *self.active_presentation.write().await = Some(pipeline.presentation);
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
                let pipeline = Self::configure_toggle_event_sink(
                    recorder,
                    overlay_enabled,
                    speech_observer.clone(),
                    self.event_broadcast.clone(),
                    transcript_bus.clone(),
                    Arc::clone(&self.delivery_tagger),
                );
                pipeline
                    .presentation
                    .set_literal_delivery(*self.force_raw_mode.read().await);
                *self.active_stop_receipt.write().await = pipeline.stop_receipt;
                *self.active_presentation.write().await = Some(pipeline.presentation);
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
        *self.active_transcript_bus.write().await = transcript_bus.clone();
        if let Some(bus) = &transcript_bus {
            bus.publish_started();
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

        Ok(CaptureAdmission::Admitted(new_session_id))
    }

    /// All toggle gestures join the capture-owned terminal operation.
    async fn stop_toggle_and_adjudicate(self: &Arc<Self>) -> Result<()> {
        self.stop_recording_from_external_surface().await
    }

    /// Terminal body called only with serial_lock held by the Stop owner.
    ///
    /// Phases are timed and logged at `info!` on purpose: the watchdog could say
    /// *that* a stop hung but never *where*, and `debug!` is filtered out in
    /// release, exactly where the hang was reported. The phase numbers in the
    /// log lines are the diagnostic contract.
    ///
    /// The session-id snapshot before the rename is a self-deadlock guard: under
    /// Rust 2024 a read guard held as an if-let scrutinee outlives the body and
    /// would block this same task's write.
    async fn stop_toggle_and_adjudicate_inner(
        &self,
        expected: Option<&str>,
        serial: OwnedMutexGuard<()>,
        closed: Option<watch::Sender<bool>>,
    ) -> Result<CaptureStopOutcome> {
        // Phase-timed instrumentation retained from the original failure.
        // STOP_TIMEOUT now bounds only callers, never this owned body.
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
        // The registered operation already owns serial_lock.
        info!(
            "stop_toggle_inner: PHASE 0 — serial_lock acquired in {:?}",
            stop_start.elapsed()
        );

        // Identity gate, inside the same `serial_lock` section as the stop it
        // authorizes. A take that replaced this one between the caller's query
        // and this call is refused here, and nothing is started in its place.
        match self.capture_gate(expected).await {
            CaptureStopOutcome::Stopped => {}
            refused => {
                info!(
                    ?refused,
                    "conditional stop refused: capture identity mismatch"
                );
                return Ok(refused);
            }
        }

        if *self.state.read().await != State::RecToggle {
            return Ok(CaptureStopOutcome::AlreadyStopping);
        }
        #[cfg(test)]
        self.observe_capture_settlement(CaptureSettlementStage::Admitted);

        info!("Stopping toggle recording with final-pass adjudication");

        let assistive = *self.assistive_mode.read().await;
        let force_ai = *self.force_ai_mode.read().await;

        // The same automatic seal as the take's start: Stop formatting and
        // projection selection never run Max for a capture.
        let runtime_settings = self.capture_runtime_settings().await;

        // Self-deadlock guard (Rust 2024): the read guard temporary from an
        // if-let chain scrutinee outlives the chain body. Inlining the read
        // would keep the guard alive across `.write().await`, blocking the
        // write on this same task's read guard → STOP_TIMEOUT hang reproduced in
        // ~/.codescribe/logs/codescribe.log 2026-05-14T00:16:23 (PHASE 1
        // never reached; watchdog forced recovery). Materialize the snapshot
        // first so the read guard drops at the semicolon.
        let session_id_snapshot = self.session_id.read().await.clone();
        if let Some(ref session_id) = session_id_snapshot {
            *self.session_id.write().await = Some(format!("{session_id}{STOPPING_SUFFIX}"));
        }

        let stop = self
            .begin_stop_paste(session_id_snapshot.as_deref(), stop_start)
            .await;
        self.set_state(State::Busy).await;
        self.show_processing_badge_if_enabled().await;

        // Phases 1–3 run inside one fallible block so that PHASE 4 — the state
        // reset, the terminal Bus line and the user-facing result — always
        // runs. A `?` here used to leave the controller `Busy` forever when the
        // ledger refused the terminal transcript (incident 2026-09-02).
        let mut rec_stop_secs = 0.0_f64;
        let mut phase3_secs = 0.0_f64;
        let result: Result<ProcessRecordingOutcome> = async {
            let phase1 = std::time::Instant::now();
            info!("stop_toggle_inner: PHASE 1 — locking recorder mutex");
            let mut recorder_guard = self.recorder.lock().await;
            info!(
                "stop_toggle_inner: PHASE 1 — recorder mutex acquired in {:?}",
                phase1.elapsed()
            );

            let recorder = Self::recorder_from_guard_mut(&mut recorder_guard, "Toggle-adjudicate")?;
            let serving_engine = recorder.streaming_engine_label();
            let capture_turn = recorder.capture_turn_intent();

            let phase2 = std::time::Instant::now();
            info!("stop_toggle_inner: PHASE 2 — closing capture before delivery");
            // The live slot is `{uuid}:stopping` here. File-lane identity is
            // the Bus uuid snapped before that rewrite.
            recorder.await_stop_pause(None, stop.stopped_at).await;
            let was_active = recorder.close_capture().await;
            let capture_closed_at = std::time::Instant::now();
            let mut take = recorder.detach_closed_take(was_active);
            let shared_document = take.is_shared_document();
            let capture_session = take.capture_identity().0.map(str::to_owned);
            Self::clear_recorder_callbacks(recorder);
            drop(recorder_guard);
            drop(serial);
            if let Some(closed) = closed { closed.send_replace(true); }
            let StopCanvasDelivery { delivery: initial_delivery, preempted } = self
                .deliver_frozen_canvas_at_stop(
                    &mut take,
                    session_id_snapshot.as_deref(),
                    (assistive, force_ai, capture_turn),
                    stop,
                    capture_closed_at,
                    runtime_settings.as_ref(),
                )
                .await;
            if preempted {
                self.detach_preempted_stop(take, session_id_snapshot.as_deref()).await?;
                initial_delivery?;
                return Ok(ProcessRecordingOutcome::default());
            }
            let (stopped, observer, pending) =
                try_stop_closed_take_for_terminal(take, session_id_snapshot.as_deref()).await;
            if let Some(take) = pending {
                // Move the exact old Bus out before foreground timeout/reset.
                // The tracked tail alone owns eventual publication and history.
                self.detach_preempted_stop(take, session_id_snapshot.as_deref()).await?;
            }
            rec_stop_secs = phase2.elapsed().as_secs_f64();
            // Read the take's own intent before the per-take state is cleared.
            // The recorder is the single owner of this fact; the stop path must
            // not re-derive it from the assistive flag or the active screen.
            // The session is over whichever way `stop()` went; the engine that
            // served it is the same on a clean stop and on a refused seal.
            Self::publish_live_serving_verdict(serving_engine);
            let (streaming_text, raw_audio_path_opt) = match stopped {
                Ok(stopped) => stopped,
                Err(err) => {
                    if let Err(delivery_error) = &initial_delivery {
                        warn!(%delivery_error, "initial delivery failed before terminal stop refusal");
                        return Err(err);
                    }
                    let settled = initial_delivery?;
                    return self
                        .process_terminal_stop_error(err, |text| async move {
                            deliver_terminal_unless_settled(settled, || async {
                                // Coverage describes the capture, not whether its
                                // owned terminal document may be formatted. Keep
                                // the same one-turn pass as the ordinary Stop.
                                let text = self
                                    .format_composer_turn_once(
                                        session_id_snapshot.as_deref(),
                                        runtime_settings.as_ref(),
                                        capture_turn,
                                        text,
                                    )
                                    .await;
                                self.deliver_stop_transcript(
                                    session_id_snapshot.as_deref(),
                                    &text,
                                    assistive,
                                    force_ai,
                                    capture_turn,
                                    true,
                                )
                                .await
                            })
                            .await
                        })
                        .await;
                }
            };
            info!(
                "stop_toggle_inner: PHASE 2 — recorder.stop() returned in {:?} (streaming_text={} chars, has_wav={})",
                phase2.elapsed(),
                streaming_text.len(),
                raw_audio_path_opt.is_some()
            );

            let phase3 = std::time::Instant::now();
            info!("stop_toggle_inner: PHASE 3 — reducer-owned transcript already delivered");
            // One composer gesture, one turn, one formatting pass. A hands-free
            // take returns unchanged here — it already formatted per occurrence
            // while it was live.
            let streaming_text = self
                .format_composer_turn_once(
                    session_id_snapshot.as_deref(),
                    runtime_settings.as_ref(),
                    capture_turn,
                    streaming_text,
                )
                .await;
            let streaming_text = if !take_delivers_to_composer(capture_turn) && !assistive {
                self.select_paste_projection(
                    session_id_snapshot.as_deref(),
                    runtime_settings.as_ref(),
                    streaming_text,
                )
                .await
            } else { streaming_text };
            let archived = retain_stopped_document(
                session_id_snapshot.as_deref(),
                capture_session.as_deref(),
                &streaming_text,
                raw_audio_path_opt.as_deref(),
                shared_document,
                &observer,
            )
            .await;
            self.link_live_archive(session_id_snapshot.as_deref(), archived)
                .await;
            let settled = initial_delivery?;
            deliver_terminal_unless_settled(settled, || async {
                self.deliver_stop_transcript(
                    session_id_snapshot.as_deref(),
                    &streaming_text,
                    assistive,
                    force_ai,
                    capture_turn,
                    false,
                )
                .await
            })
            .await?;
            phase3_secs = phase3.elapsed().as_secs_f64();
            info!(
                "stop_toggle_inner: PHASE 3 — reducer handoff completed in {:?}",
                phase3.elapsed()
            );
            Ok(ProcessRecordingOutcome {
                transcript_present: !streaming_text.trim().is_empty(),
                ..ProcessRecordingOutcome::default()
            })
        }
        .await;

        let phase4 = std::time::Instant::now();
        self.toggle_user_has_text.store(false, Ordering::SeqCst);
        self.toggle_assistant_has_text
            .store(false, Ordering::SeqCst);
        self.reset_finished_recording_state(&result).await;
        self.handle_processed_recording_result(assistive, &result)
            .await;
        let cleanup_secs = phase4.elapsed().as_secs_f64();
        let total_secs = stop_start.elapsed().as_secs_f64();
        info!(
            "stop_toggle_inner: PHASE 4 — cleanup + result handler completed in {:?} (total stop time: {:?}, cleanup={cleanup_secs:.3}s)",
            phase4.elapsed(),
            stop_start.elapsed()
        );
        info!(
            total_secs,
            rec_stop_secs, phase3_secs, cleanup_secs, "stop_toggle_inner: mechanical stop timing"
        );

        result.map(|_| CaptureStopOutcome::Stopped)
    }

    /// Publish the engine that served the take that just stopped as the
    /// Settings "Active STT" truth (`serving_status` owner). Called once per
    /// stop path, right after the recorder released the session, so a clean
    /// stop and a refused seal report the same fact: the engine the live
    /// session actually ran on. Never derived from the configured `stt_engine`.
    fn publish_live_serving_verdict(streaming_engine_label: &str) {
        let verdict = serving_status::LastServingVerdict::from_live_session(streaming_engine_label);
        info!(
            engine = %verdict.engine,
            routing_mode = %verdict.routing_mode,
            "serving verdict published"
        );
        serving_status::publish_last_serving(verdict);
    }

    /// Close future admission, including already scheduled hold starts. A
    /// start already inside serial_lock remains owned until it returns.
    pub fn request_capture_shutdown(&self) {
        self.shutdown_requested.store(true, Ordering::SeqCst);
    }

    /// Quiescence is checked under start serialization, never from recording
    /// telemetry. Keep this closed controller in the bridge slot until runtime
    /// teardown so a concurrent composer cannot lazily construct a fresh owner.
    pub fn capture_shutdown_settled(&self) -> bool {
        if !self.shutdown_requested.load(Ordering::SeqCst) {
            return false;
        }
        let Ok(_serial) = self.serial_lock.try_lock() else {
            return false;
        };
        self.capture_work_is_quiescent()
    }

    /// Reserve an idle restart under the same boundary as every capture start.
    /// Refusal leaves capture admission open. Acceptance closes it before the
    /// caller can request process termination, including future channel opens.
    pub fn reserve_idle_restart(&self) -> bool {
        let Ok(_serial) = self.serial_lock.try_lock() else {
            return false;
        };
        let Ok(channels) = self.agent_channels.try_lock() else {
            return false;
        };
        let Ok(hold) = self.hold_start_task.try_lock() else {
            return false;
        };
        if self.shutdown_requested.load(Ordering::SeqCst)
            || !channels.is_empty()
            || hold.as_ref().is_some_and(|task| !task.is_finished())
            || !self.capture_work_is_quiescent()
        {
            return false;
        }
        self.shutdown_requested.store(true, Ordering::SeqCst);
        true
    }

    fn capture_work_is_quiescent(&self) -> bool {
        let Ok(slot) = self.capture_settlement.try_lock() else {
            return false;
        };
        if slot
            .as_ref()
            .is_some_and(|op| !op.task.is_finished() || op.result.borrow().is_none())
        {
            return false;
        }
        let Ok(tails) = self.closed_capture_tails.try_lock() else {
            return false;
        };
        if tails.iter().any(|task| !task.is_finished()) {
            return false;
        }
        let Ok(state) = self.state.try_read() else {
            return false;
        };
        if *state != State::Idle {
            return false;
        }
        let Ok(identity) = self.session_id.try_read() else {
            return false;
        };
        if identity.is_some() {
            return false;
        }
        let Ok(recorder) = self.recorder.try_lock() else {
            return false;
        };
        if recorder
            .as_ref()
            .is_some_and(|rec| rec.recorder.is_active())
        {
            return false;
        }
        let Ok(conversation) = self.conversation_task.try_lock() else {
            return false;
        };
        conversation.as_ref().is_none_or(|task| task.is_finished())
    }

    /// Bind Stop to the published capture before waiting for start serialization.
    /// The retained terminal owner checks that identity under the serial lock;
    /// a delayed gesture cannot discover or stop a replacement after waiting.
    pub async fn stop_current_capture(self: &Arc<Self>) -> Result<CaptureStopOutcome> {
        match self.current_capture_stop() {
            CaptureStopAdmission::Immediate(outcome) => Ok(outcome),
            CaptureStopAdmission::Settling(result, _) => Self::await_capture_stop(result).await,
        }
    }

    fn current_capture_stop(self: &Arc<Self>) -> CaptureStopAdmission {
        let Ok(mut slot) = self.capture_settlement.try_lock() else {
            return CaptureStopAdmission::Immediate(CaptureStopOutcome::AdmissionUnavailable);
        };
        if let Some(operation) = slot
            .as_ref()
            .filter(|op| !op.task.is_finished() || op.result.borrow().is_none())
        {
            CaptureStopAdmission::Settling(operation.result.clone(), operation.closed.clone())
        } else {
            let Ok(state) = self.state.try_read() else {
                return CaptureStopAdmission::Immediate(CaptureStopOutcome::AdmissionUnavailable);
            };
            if *state == State::Idle {
                // Idle is only conclusive once any unpublished start has
                // released admission. A published take below already has
                // its own identity and can register Stop without this lock.
                let Ok(_serial) = self.serial_lock.try_lock() else {
                    return CaptureStopAdmission::Immediate(
                        CaptureStopOutcome::AdmissionUnavailable,
                    );
                };
                // No active terminal body to cancel. Invalidate a delayed
                // hold that has not crossed this same admission boundary.
                self.hold_start_generation.fetch_add(1, Ordering::SeqCst);
                return CaptureStopAdmission::Immediate(CaptureStopOutcome::NoLiveCapture);
            }
            if !matches!(*state, State::RecHold | State::RecToggle) {
                // Conversation has a separate loop owner, and Busy without
                // a settlement receipt is not proof of a released resource.
                return CaptureStopAdmission::Immediate(CaptureStopOutcome::AdmissionUnavailable);
            }
            let Ok(identity) = self.session_id.try_read() else {
                return CaptureStopAdmission::Immediate(CaptureStopOutcome::AdmissionUnavailable);
            };
            let Some(id) = identity.as_deref() else {
                return CaptureStopAdmission::Immediate(CaptureStopOutcome::AdmissionUnavailable);
            };
            let result = self.register_capture_stop(&mut slot, id);
            CaptureStopAdmission::Settling(
                result,
                slot.as_ref().expect("registered stop").closed.clone(),
            )
        }
    }

    /// Native dispatch may release its take-ownership claim once the owned
    /// stop has fenced PCM. Busy still vetoes new takes until foreground reset.
    pub fn stopped_capture_feed_closed(&self) -> bool {
        self.capture_settlement
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .is_some_and(|operation| *operation.closed.borrow())
    }

    /// Legacy void bridge surface must not turn Pending into successful Stop.
    pub async fn stop_recording_from_external_surface(self: &Arc<Self>) -> Result<()> {
        match self.stop_current_capture().await? {
            CaptureStopOutcome::Stopped | CaptureStopOutcome::NoLiveCapture => Ok(()),
            outcome => Err(anyhow::anyhow!("Stop remains unresolved: {outcome:?}")),
        }
    }

    /// Only the registered owner calls terminal processing. Routing and identity
    /// are decided in the same serialization section as the terminal effects.
    async fn stop_external_capture(
        &self,
        expected: Option<&str>,
        closed: watch::Sender<bool>,
    ) -> Result<CaptureStopOutcome> {
        let serial = Arc::clone(&self.serial_lock).lock_owned().await;
        match self.capture_gate(expected).await {
            CaptureStopOutcome::Stopped => {}
            refused => return Ok(refused),
        }
        let state = self.current_state().await;
        let assistive = *self.assistive_mode.read().await;
        let single_turn = if state == State::RecToggle {
            let recorder = self.recorder.lock().await;
            recorder
                .as_ref()
                .is_none_or(|rec| rec.capture_turn_intent() == CaptureTurnIntent::SingleTurn)
        } else {
            false
        };
        if single_turn
            || should_use_toggle_adjudicated_stop(state, assistive, toggle_final_pass_enabled())
        {
            self.stop_toggle_and_adjudicate_inner(expected, serial, Some(closed))
                .await
        } else if matches!(state, State::RecHold | State::RecToggle) {
            #[cfg(test)]
            self.observe_capture_settlement(CaptureSettlementStage::Admitted);
            self.cancel_pending_hold_start().await;
            self.finish_recording_locked(serial, Some(closed))
                .await
                .map(|()| CaptureStopOutcome::Stopped)
        } else {
            Ok(CaptureStopOutcome::AlreadyStopping)
        }
    }

    /// Start one explicit Agent-composer take on the shared controller.
    ///
    /// One gesture, one turn. This is the same recorder, the same ledger and
    /// the same reducer every other lane uses — the only thing that differs is
    /// the per-take [`CaptureTurnIntent`], which keeps hands-free utterance
    /// epochs out of a take the user ends explicitly.
    ///
    /// Refused unless the controller is idle: a composer press must never
    /// hijack, restart or silently inherit a capture some other surface owns.
    ///
    /// Returns the identity of the capture the controller actually admitted.
    /// Reading it *after* the start is what makes it evidence rather than a
    /// hopeful guess: a start that produced no session slot produced no take,
    /// and its caller receives no stop permission to hand back later.
    pub async fn start_composer_turn_recording(&self) -> Result<String> {
        match self
            .start_toggle_recording(true, CaptureTurnIntent::SingleTurn)
            .await?
        {
            CaptureAdmission::Admitted(id) => Ok(id),
            CaptureAdmission::NotAdmitted => Err(anyhow::anyhow!(
                "composer take refused: start admitted no capture"
            )),
        }
    }

    /// Compare a caller's admitted capture identity against the live session.
    ///
    /// **Callers must already hold `serial_lock`.** Every start and every stop
    /// crosses that same lock, so the comparison and the stop it authorizes are
    /// one critical section: a replacement take cannot slip between them.
    ///
    /// The gate reads take identity, never capture existence. The `session_id`
    /// slot belongs to the one live dictation take; a capture kept open by
    /// non-take subscribers (agent channel) registers no take here, so stops
    /// correctly report `NoLiveCapture` and leave that capture untouched.
    ///
    /// Production passes a named capture, including stop-current gestures.
    /// `None` remains only for direct terminal-body fixtures.
    async fn capture_gate(&self, expected: Option<&str>) -> CaptureStopOutcome {
        let Some(expected) = expected else {
            return CaptureStopOutcome::Stopped;
        };
        // Snapshot into a local first (Rust 2024 temporary scope): the guard
        // from a `let ... else` scrutinee would outlive the branch and this
        // task takes controller locks again downstream.
        let live = self.session_id.read().await.clone();
        let Some(live) = live else {
            return CaptureStopOutcome::NoLiveCapture;
        };
        if live == expected {
            CaptureStopOutcome::Stopped
        } else if live.strip_suffix(STOPPING_SUFFIX) == Some(expected) {
            CaptureStopOutcome::AlreadyStopping
        } else {
            CaptureStopOutcome::ForeignCapture
        }
    }

    /// Stop the capture this caller opened, and only that one.
    ///
    /// A foreign take survives untouched and no new take is started in its
    /// place: refusing is the whole point of naming the capture.
    pub async fn stop_capture_if_owned(
        self: &Arc<Self>,
        capture_id: &str,
    ) -> Result<CaptureStopOutcome> {
        let result = {
            let Ok(mut slot) = self.capture_settlement.try_lock() else {
                return Ok(CaptureStopOutcome::AdmissionUnavailable);
            };
            if let Some(operation) = slot.as_ref().filter(|op| op.capture_id == capture_id) {
                operation.result.clone()
            } else if slot
                .as_ref()
                .is_some_and(|op| !op.task.is_finished() || op.result.borrow().is_none())
            {
                return Ok(CaptureStopOutcome::AdmissionUnavailable);
            } else {
                self.register_capture_stop(&mut slot, capture_id)
            }
        };
        Self::await_capture_stop(result).await
    }

    /// Synchronous registration: no cancellation point between spawning and
    /// retaining the task. Only one slot owns processing and its result.
    fn register_capture_stop(
        self: &Arc<Self>,
        slot: &mut Option<CaptureSettlement>,
        capture_id: &str,
    ) -> watch::Receiver<Option<CaptureSettlementResult>> {
        let (sender, receiver) = watch::channel(None);
        let (closed_sender, closed) = watch::channel(false);
        let controller = Arc::clone(self);
        let owned_id = capture_id.to_owned();
        let task = tokio::spawn(async move {
            let settled = controller
                .stop_external_capture(Some(&owned_id), closed_sender)
                .await
                .map_err(|error| format!("{error:#}"));
            sender.send_replace(Some(settled));
        });
        *slot = Some(CaptureSettlement {
            capture_id: capture_id.to_owned(),
            result: receiver.clone(),
            closed,
            task,
        });
        #[cfg(test)]
        self.observe_capture_settlement(CaptureSettlementStage::Registered);
        receiver
    }

    async fn await_capture_stop(
        mut result: watch::Receiver<Option<CaptureSettlementResult>>,
    ) -> Result<CaptureStopOutcome> {
        let deadline = tokio::time::Instant::now() + STOP_TIMEOUT;
        let settled = tokio::time::timeout_at(deadline, async {
            loop {
                if let Some(settled) = result.borrow_and_update().clone() {
                    return settled.map_err(anyhow::Error::msg);
                }
                result.changed().await.map_err(|_| anyhow::anyhow!(
                    "capture settlement task ended without a terminal result; recovery remains owed"
                ))?;
            }
        })
        .await;
        match settled {
            Ok(result) => result,
            Err(_) => Ok(CaptureStopOutcome::Pending),
        }
    }

    #[cfg(test)]
    fn observe_capture_settlement(&self, stage: CaptureSettlementStage) {
        if let Some(observer) = self.settlement_observer.lock().unwrap().as_ref() {
            let _ = observer.send(stage);
        }
    }

    /// Format one composer turn exactly once, at terminal processing.
    ///
    /// Returns the text the stop path should deliver. Every early return is a
    /// provider call that never happens rather than one that is made and
    /// discarded:
    ///
    /// - a hands-free take already paid per occurrence during capture;
    /// - a take with no terminal authority has nothing to format;
    /// - an empty or whitespace-only turn produces no request at all;
    /// - a formatter refusal (failed, policy-skipped, healthy no-op) keeps the
    ///   committed document exactly as the ledger sealed it.
    ///
    /// On success delivery selects the source-bound derived version. The Bus
    /// Raw document, shared buffer and acoustic revision remain unchanged.
    async fn format_composer_turn_once(
        &self,
        take_id: Option<&str>,
        runtime_settings: &RuntimeSettingsSnapshot,
        capture_turn: CaptureTurnIntent,
        committed_text: String,
    ) -> String {
        if !capture_turn.formats_once_at_terminal() {
            return committed_text;
        }
        // Snapshot into a local first. Under Rust 2024 the read guard produced
        // inside a `let ... else` scrutinee outlives the diverging branch, and
        // this task takes the same lock again further down.
        let active_presentation = self.active_presentation.read().await.clone();
        let Some(presentation) = active_presentation else {
            warn!("Composer turn: no terminal transcript authority; delivering committed text");
            return committed_text;
        };
        let Some(TerminalFormatterRequest {
            session_id,
            source_revision,
            source_text,
        }) = presentation
            .terminal_formatter_request()
            .filter(|request| Some(request.session_id.as_str()) == take_id)
        else {
            debug!("Composer turn: nothing to format at terminal; no provider call issued");
            return committed_text;
        };
        let language = runtime_settings.values().whisper_language;
        // The one paid call this take is allowed. Same production entry point
        // the explicit overlay formatter uses; no second lane, no retry loop.
        let consultation = match self.selected_max_consultation(runtime_settings).await {
            Ok(consultation) => consultation,
            Err(error) => {
                warn!(%error, "Max consultation unavailable; preserving committed transcript");
                self.publish_formatting_degraded(take_id);
                return committed_text;
            }
        };
        let turn_id = format!("{session_id}:revision:{source_revision}");
        let result = format_text_with_status_for_policy(
            &source_text,
            language.whisper_hint(),
            runtime_settings,
            consultation.as_deref().map(|agent| {
                codescribe_core::ai_formatting::FormattingConsultation {
                    agent,
                    turn_id: &turn_id,
                }
            }),
        )
        .await;
        let formatting_failed =
            result.status == codescribe_core::llm::ai_formatting::AiFormatStatus::Failed;
        match presentation.apply_formatter_revision(
            session_id,
            source_revision,
            runtime_settings.formatting_policy(),
            result,
        ) {
            Ok(commit) => {
                info!(
                    revision = commit.revision,
                    receipt = %commit.provenance_receipt,
                    "Composer turn formatted once at terminal processing"
                );
                // Refused takes are archived before their retained words are
                // delivered. Persist the accepted formatter step there too;
                // ordinary Stop links its archive after this pass.
                self.sync_live_archive(&presentation, &commit.session_id)
                    .await;
                presentation
                    .delivery_projection(runtime_settings.formatting_policy(), &source_text)
                    .map(|projection| projection.rendered_text)
                    .unwrap_or(committed_text)
            }
            Err(refusal) => {
                info!(%refusal, "Composer turn terminal formatting refused; committed text stands");
                if formatting_failed {
                    self.publish_formatting_degraded(take_id);
                }
                committed_text
            }
        }
    }

    async fn select_paste_projection(
        &self,
        take_id: Option<&str>,
        runtime_settings: &RuntimeSettingsSnapshot,
        raw: String,
    ) -> String {
        self.select_paste_projection_with_assembly(
            take_id,
            runtime_settings,
            raw,
            crate::presentation::emitter::ProjectionAssembly::AllOrWholeRaw,
        )
        .await
    }

    async fn select_paste_projection_with_assembly(
        &self,
        take_id: Option<&str>,
        runtime_settings: &RuntimeSettingsSnapshot,
        raw: String,
        assembly: crate::presentation::emitter::ProjectionAssembly,
    ) -> String {
        use crate::presentation::emitter::ProjectionAssembly;
        let policy = runtime_settings.formatting_policy();
        let policy_supported = match assembly {
            ProjectionAssembly::AllOrWholeRaw => matches!(
                policy,
                FormattingPolicy::Correction | FormattingPolicy::Smart | FormattingPolicy::Max
            ),
            ProjectionAssembly::RefusedPerOccurrence => {
                matches!(
                    policy,
                    FormattingPolicy::Correction | FormattingPolicy::Smart
                )
            }
        };
        if !policy_supported {
            return raw;
        }
        let Some(presentation) = self.active_presentation.read().await.clone() else {
            return raw;
        };
        let expected_session = take_id.map(|id| id.strip_suffix(STOPPING_SUFFIX).unwrap_or(id));
        if !expected_session.is_some_and(|session| presentation.owns_document(session)) {
            warn!(
                take_id,
                "Stop formatter selection refused a different capture presentation"
            );
            return raw;
        }
        if presentation.literal_delivery() {
            return raw;
        }
        if matches!(
            policy,
            FormattingPolicy::Correction | FormattingPolicy::Smart
        ) {
            match assembly {
                ProjectionAssembly::AllOrWholeRaw => {
                    presentation
                        .wait_for_occurrence_projections(
                            policy,
                            &raw,
                            runtime_settings
                                .ai_execution()
                                .request_timing()
                                .attempt_timeout(),
                        )
                        .await;
                }
                ProjectionAssembly::RefusedPerOccurrence => {
                    presentation
                        .wait_for_refused_occurrence_projections(policy, &raw)
                        .await;
                }
            }
        }
        let Some(projection) =
            presentation.delivery_projection_with_assembly(take_id, policy, &raw, assembly)
        else {
            if assembly == ProjectionAssembly::RefusedPerOccurrence {
                self.publish_formatting_degraded(expected_session);
            }
            return raw;
        };
        if policy == FormattingPolicy::Smart
            && projection.delivered_mode == FormattingPolicy::Correction.as_str()
        {
            info!(status = "Smart unavailable — delivered Corrections", source_revision = projection.source_raw_revision, receipt = %projection.receipt_id);
        }
        if projection.requested_mode != projection.delivered_mode || projection.status != "applied"
        {
            self.publish_formatting_degraded(expected_session);
        }
        if assembly == ProjectionAssembly::RefusedPerOccurrence
            && let Some(session_id) = expected_session
        {
            self.sync_live_archive(&presentation, session_id).await;
        }
        projection.rendered_text
    }

    fn publish_formatting_degraded(&self, take_id: Option<&str>) {
        if let Some(take_id) = take_id {
            let status = PresentationStatusProjection::formatting_degraded(take_id.to_string());
            Self::broadcast_presentation_status(&self.event_broadcast, &status);
        }
    }

    /// Stop recording, transcribe, format, and paste the result
    ///
    /// This is the core processing pipeline that:
    /// 1. Stops the audio recorder
    /// 2. Transcribes the audio via backend
    /// 3. Formats the transcript (if assistive mode enabled)
    /// 4. Pastes the result into the active application
    pub async fn finish_recording(self: &Arc<Self>) -> Result<()> {
        self.stop_recording_from_external_surface().await
    }

    /// Only the capture settlement task calls this with serial_lock held.
    async fn finish_recording_locked(
        &self,
        serial: OwnedMutexGuard<()>,
        closed: Option<watch::Sender<bool>>,
    ) -> Result<()> {
        let stop_start = std::time::Instant::now();
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

        let stop_take_id = self.session_id.read().await.clone();
        let stop = self
            .begin_stop_paste(stop_take_id.as_deref(), stop_start)
            .await;
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

        let result = self
            .process_recording(
                session_id,
                (assistive, hold_mode, force_raw, force_ai),
                stop,
                serial,
                closed,
            )
            .await;

        self.reset_finished_recording_state(&result).await;
        self.handle_processed_recording_result(assistive, &result)
            .await;

        result.map(|_| ())
    }

    /// Process the recording: stop, transcribe, format, paste
    ///
    /// ## Mode Logic:
    /// - `assistive=true`: ALWAYS AI augmentation (HoldMode::Chat / HoldMode::Selection)
    /// - `force_raw=true`: ALWAYS raw transcript (HoldMode::Raw)
    /// - `force_ai=true`: ALWAYS AI formatting (left double Option)
    /// - Neither: Toggle mode - respects the FORMATTING_LEVEL setting
    async fn process_recording(
        &self,
        session_id: Option<String>,
        mode: (bool, HoldMode, bool, bool),
        stop: StopCanvasRequest,
        serial: OwnedMutexGuard<()>,
        closed: Option<watch::Sender<bool>>,
    ) -> Result<ProcessRecordingOutcome> {
        let (assistive, hold_mode, force_raw, force_ai) = mode;
        // The same automatic seal as the take's start: Stop never runs Max.
        let runtime_settings = self.capture_runtime_settings().await;
        if cfg!(test) {
            info!(
                "process_recording: skipped in tests (assistive={}, hold_mode={:?}, force_raw={}, force_ai={})",
                assistive, hold_mode, force_raw, force_ai
            );
            return Ok(ProcessRecordingOutcome::default());
        }

        // Stop the recorder and get audio file path
        let take_id = match session_id {
            Some(id) => Some(id),
            None => self.session_id.read().await.clone(),
        };
        let mut recorder_guard = self.recorder.lock().await;
        let recorder = Self::recorder_from_guard_mut(&mut recorder_guard, "Process-recording")?;
        let serving_engine = recorder.streaming_engine_label();
        let capture_turn = recorder.capture_turn_intent();
        recorder.await_stop_pause(None, stop.stopped_at).await;
        let was_active = recorder.close_capture().await;
        let capture_closed_at = std::time::Instant::now();
        let mut take = recorder.detach_closed_take(was_active);
        let shared_document = take.is_shared_document();
        let capture_session = take.capture_identity().0.map(str::to_owned);
        Self::clear_recorder_callbacks(recorder);
        drop(recorder_guard);
        drop(serial);
        if let Some(closed) = closed {
            closed.send_replace(true);
        }
        let StopCanvasDelivery {
            delivery: initial_delivery,
            preempted,
        } = self
            .deliver_frozen_canvas_at_stop(
                &mut take,
                take_id.as_deref(),
                (assistive, force_ai, capture_turn),
                stop,
                capture_closed_at,
                runtime_settings.as_ref(),
            )
            .await;
        if preempted {
            self.detach_preempted_stop(take, take_id.as_deref()).await?;
            initial_delivery?;
            return Ok(ProcessRecordingOutcome::default());
        }
        let (stopped, observer, pending) =
            try_stop_closed_take_for_terminal(take, take_id.as_deref()).await;
        if let Some(take) = pending {
            self.detach_preempted_stop(take, take_id.as_deref()).await?;
        }
        Self::publish_live_serving_verdict(serving_engine);
        let (streaming_text, raw_audio_path_opt) = match stopped {
            Ok(stopped) => stopped,
            Err(err) => {
                if let Err(delivery_error) = &initial_delivery {
                    warn!(%delivery_error, "initial delivery failed before terminal stop refusal");
                    return Err(err);
                }
                let settled = initial_delivery?;
                return self
                    .process_terminal_stop_error(err, |text| async move {
                        deliver_terminal_unless_settled(settled, || async {
                            let text = self
                                .format_composer_turn_once(
                                    take_id.as_deref(),
                                    runtime_settings.as_ref(),
                                    capture_turn,
                                    text,
                                )
                                .await;
                            self.deliver_stop_transcript(
                                take_id.as_deref(),
                                &text,
                                assistive,
                                force_ai,
                                capture_turn,
                                true,
                            )
                            .await
                        })
                        .await
                    })
                    .await;
            }
        };

        let streaming_text = self
            .format_composer_turn_once(
                take_id.as_deref(),
                runtime_settings.as_ref(),
                capture_turn,
                streaming_text,
            )
            .await;
        let streaming_text = if !assistive {
            self.select_paste_projection(
                take_id.as_deref(),
                runtime_settings.as_ref(),
                streaming_text,
            )
            .await
        } else {
            streaming_text
        };

        let archived = retain_stopped_document(
            take_id.as_deref(),
            capture_session.as_deref(),
            &streaming_text,
            raw_audio_path_opt.as_deref(),
            shared_document,
            &observer,
        )
        .await;
        self.link_live_archive(take_id.as_deref(), archived).await;
        // Ctrl-hold literal (`force_raw`) and the hold flavour are sink-time
        // facts already consumed when the emitter was built; delivery reads
        // only the frozen intent.
        let _ = (hold_mode, force_raw);
        let settled = initial_delivery?;
        deliver_terminal_unless_settled(settled, || async {
            self.deliver_stop_transcript(
                take_id.as_deref(),
                &streaming_text,
                assistive,
                force_ai,
                capture_turn,
                false,
            )
            .await
        })
        .await?;
        Ok(ProcessRecordingOutcome {
            transcript_present: !streaming_text.trim().is_empty(),
            ..ProcessRecordingOutcome::default()
        })
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
        *self.active_presentation.write().await = None;
        *self.pre_overlay_frontmost_app.write().await = None;
        self.reset_session_fields(TranscriptSessionEndReason::TranscriptionFailed)
            .await;

        info!("State reset to IDLE complete");
    }

    /// Check if controller is in a recording state
    pub async fn is_recording(&self) -> bool {
        matches!(
            self.current_state().await,
            State::RecHold | State::RecToggle
        )
    }
}

impl Default for RecordingController {
    /// Build a controller with production defaults (`RecordingController::new`).
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "hold_context_tests.rs"]
mod tests;
