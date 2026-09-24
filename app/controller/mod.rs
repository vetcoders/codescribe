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
/// Per-session assistive context bag (selection, app, images).
mod context_bucket;
/// One destination throne: intent → Agent / Orient / paste. Focus is not king.
mod delivery_route;
/// Session telemetry, image attach helpers, assistive send wiring.
mod helpers;
/// Hold/toggle timing, agent-send vetoes, stop adjudication policy.
mod hotkey_policy;
/// Production-owned, content-private PCM replay of the overlay engine cone.
pub mod production_replay;
/// Public serving-status surface for tray/UI consumers.
pub mod serving_status;
mod transcript_delivery;
/// Controller state, hotkey types, and recording truth metadata.
mod types;

pub use delivery_route::{
    DeliveryIntent, DeliveryRoute, OverlayPasteDelivery, OverlayPasteResult,
    delivery_intent_from_session, format_delivery_route_line, overlay_insert_facts,
    resolve_delivery_route,
};
pub(crate) use delivery_route::{
    TranscriptProjectionAvailability, resolve_transcript_projection_availability,
};
pub use helpers::{
    is_assistive_session, publish_recording_indicator, set_assistive_session,
    set_assistive_target_thread,
};
pub use types::{HotkeyAction, HotkeyInput, HotkeyType, State};

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
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc, watch};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::audio::streaming_recorder::{
    CaptureStopFailure, CaptureTurnIntent, StreamingRecorder, TerminalSealRefused,
};
use crate::config::models::ModelManager;
use crate::config::{Config, RuntimeSettingsSnapshot, UserSettings};
use crate::os::clipboard;
use crate::os::hold_badge::BadgeMode;
use crate::os::hotkeys::{self, HoldMode};
use crate::os::selection::{
    AssistiveContext, capture_assistive_context,
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
    STOP_TIMEOUT, effective_hold_start_delay_ms, should_apply_incoming_mode_flags,
    should_block_hotkey_during_agent_send, should_use_toggle_adjudicated_stop,
    toggle_final_pass_enabled,
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
    /// Apple Speech preflight refused before the microphone opened.
    PreflightRefused,
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
            Self::PreflightRefused
            | Self::AdmissionRefused
            | Self::RecorderUnavailable
            | Self::RecorderStartFailed => TranscriptSessionEndReason::StartFailed,
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
    event_sink: Arc<dyn codescribe_core::pipeline::contracts::EventSink>,
    presentation: Arc<PresentationEmitter>,
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

/// Keep the take WAV under its Bus `session_id` so named followers never
/// share or overwrite a single slot. Also refresh the latest-take alias.
fn retain_session_audio(
    session_id: Option<&str>,
    path: &std::path::Path,
    transcript: codescribe_core::state::SessionTranscriptArchive<'_>,
) {
    if let Err(error) = retain_session_audio_at(
        session_id,
        path,
        transcript,
        &Config::config_dir(),
        codescribe_core::state::archive_session_take_from_file,
    ) {
        warn!("{error:#}");
    }
}

/// Same archive/copy path for ordinary and failed takes. Attempt every owned
/// destination even if one fails, report failure, and never remove the source.
/// Directory and daily-bag operation are injectable without changing HOME or
/// invoking the history transcoder in controller tests.
fn retain_session_audio_at(
    session_id: Option<&str>,
    path: &std::path::Path,
    transcript: codescribe_core::state::SessionTranscriptArchive<'_>,
    root: &std::path::Path,
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
    if has_speech && session_retained {
        let alias = root.join("last_session.wav");
        match root_directory
            .as_ref()
            .map_err(|error| anyhow::anyhow!("{error:#}"))
            .and_then(|directory| publish_session_alias(directory, id))
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
    error: anyhow::Error,
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
    if let Some(path) = failure.audio_path.as_deref()
        && let Err(archive_error) = retain(id.expect("identity checked"), path)
    {
        let message = format!("{archive_error:#}; original processing error: {error:#}");
        return error.context(message);
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

// Provisional budget for the last Apple final (codex/integrator choice, not
// measured and not a contract value: the contract's 4 s is the Layer 1 Whisper
// window). Calibrate from live `last_window_close_ms` p50/p99. Expiry chooses
// terminal delivery so a slow final never pastes a short prefix.
const LAST_WINDOW_CLOSE_BOUND: std::time::Duration = std::time::Duration::from_secs(4);

async fn await_last_window_close_for_delivery(
    recorder: &mut StreamingRecorder,
    stop_start: std::time::Instant,
) -> Option<u128> {
    let closed = recorder
        .wait_last_window_closed(LAST_WINDOW_CLOSE_BOUND)
        .await;
    let elapsed_ms = stop_start.elapsed().as_millis();
    if !closed {
        warn!(
            elapsed_ms,
            bound_ms = LAST_WINDOW_CLOSE_BOUND.as_millis(),
            "last_window_close_timeout"
        );
        return None;
    }
    Some(elapsed_ms)
}

/// Stop the recorder for a finished take and classify the outcome.
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
async fn stop_recorder_for_terminal(
    recorder: &mut StreamingRecorder,
    session_id: Option<&str>,
    capture_closed: Option<bool>,
) -> Result<(String, Option<std::path::PathBuf>)> {
    let (capture_session, capture_epoch) = recorder.capture_identity();
    let capture_session = capture_session.map(str::to_owned);
    let stopped = match capture_closed {
        Some(was_active) => recorder.finish_closed_capture(was_active).await,
        None => recorder.stop().await,
    };
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
                        retain_session_audio(session_id, path, refused_take_archive(&refusal))
                    }
                    None => warn!("refused take has no audio path to retain"),
                }
                Err(anyhow::Error::new(refusal))
            }
            Err(err) => {
                if err.downcast_ref::<CaptureStopFailure>().is_some() {
                    Err(recover_capture_stop_failure(
                        err,
                        session_id,
                        (capture_session.as_deref(), capture_epoch),
                        |id, path| {
                            retain_session_audio_at(
                                Some(id),
                                path,
                                codescribe_core::state::SessionTranscriptArchive::Unavailable(
                                    "capture processing failed; committed text unavailable",
                                ),
                                &Config::config_dir(),
                                codescribe_core::state::archive_session_take_from_file,
                            )
                        },
                    ))
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
    task: JoinHandle<()>,
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
    /// The one observer bus for the active recording. Presentation may publish
    /// mutable drafts through it, but only the stop controller publishes the
    /// immutable product seal after every automatic stage completes.
    active_transcript_bus: Arc<RwLock<Option<Arc<TranscriptBus>>>>,
    /// Retained after microphone teardown so a terminal overlay edit can enter
    /// the exact reducer/ledger pair that authored the visible projection.
    /// Replaced atomically when the next take installs its own authority.
    active_presentation: Arc<RwLock<Option<Arc<PresentationEmitter>>>>,

    /// Max conversation survives capture teardown; microphone lifetime is not
    /// conversation lifetime. No chat selection or OS focus changes this slot.
    max_consultation: Mutex<Option<Arc<crate::agent::max_consultation::MaxConsultation>>>,
    /// The shared approval mechanism, scoped to this controller's Max owner.
    max_approvals: Arc<codescribe_core::agent::ApprovalBroker>,

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
    #[cfg(test)]
    settlement_observer: std::sync::Mutex<Option<mpsc::UnboundedSender<CaptureSettlementStage>>>,

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
        let resources = Self::acquire_startup_resources(recorder_context);
        Self::from_startup_inputs(runtime_settings, resources, Config::config_dir())
    }

    fn acquire_startup_resources(recorder_context: &str) -> ControllerStartupResources {
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

            if !crate::whisper::is_initialized() {
                // Best-effort BACKGROUND prewarm — never block recording readiness.
                //
                // Product invariant: recording readiness is NOT engine readiness.
                // Audio capture must start the moment the user presses record; the
                // live local refinement and explicit Retranscribe lazy-load the
                // engine on first use.
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
                crate::config::note_startup_acquisition("STT prewarm thread");
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
            max_consultation: Mutex::new(None),
            max_approvals: Arc::default(),
            hold_start_task: Arc::new(Mutex::new(None)),
            hold_start_generation: Arc::new(AtomicU64::new(0)),
            start_transition_in_flight: Arc::new(AtomicBool::new(false)),
            serial_lock: Arc::new(Mutex::new(())),
            shutdown_requested: Arc::new(AtomicBool::new(false)),
            capture_settlement: std::sync::Mutex::new(None),
            #[cfg(test)]
            settlement_observer: std::sync::Mutex::new(None),
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
            last_segment_audio_offset: Arc::new(AtomicUsize::new(0)),
            // Conversation mode (lazy init)
            conversation_engine: Arc::new(Mutex::new(None)),
            audio_player: Arc::new(Mutex::new(None)),
            conversation_stop_flag: Arc::new(AtomicBool::new(false)),
            conversation_generation: Arc::new(AtomicU64::new(0)),
            conversation_task: Arc::new(Mutex::new(None)),
            event_broadcast,
        }
    }

    /// Get current state
    pub async fn current_state(&self) -> State {
        *self.state.read().await
    }

    /// Resolve the controller's retained Max consultation without another
    /// recorder or a process-global conversation selection.
    async fn selected_max_consultation(
        &self,
        settings: &RuntimeSettingsSnapshot,
    ) -> Result<Option<Arc<crate::agent::max_consultation::MaxConsultation>>> {
        if settings.formatting_policy() != codescribe_core::config::FormattingPolicy::Max {
            return Ok(None);
        }
        let mut selected = self.max_consultation.lock().await;
        if selected.is_none() {
            let gateway = codescribe_core::agent::ThreadDeliveryGateway::new()?;
            let consultation_id = gateway.selected_max_consultation_id()?;
            let approvals = Arc::clone(&self.max_approvals);
            let approval_handler: codescribe_core::agent::ToolApprovalHandler =
                Arc::new(move |request| approvals.begin(request));
            let consultation = crate::agent::max_consultation::MaxConsultation::start_deferred(
                consultation_id,
                settings,
                Box::new(|| Arc::new(crate::agent::tools::configured_registry())),
                Some(approval_handler),
                gateway,
                Arc::new(|_consultation, _turn, event| {
                    if let codescribe_core::agent::AgentUiEvent::Error(error) = event {
                        warn!(%error, "Max consultation failed");
                    }
                }),
                codescribe_core::config::agent_turn_lease_path(),
            );
            *selected = Some(Arc::new(consultation));
        }
        Ok(selected.clone())
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
        presentation
            .apply_user_revision(UserRevisionIntent {
                session_id,
                source_revision,
                rendered_text,
                provenance,
            })
            .map_err(anyhow::Error::new)
    }

    /// Format the exact retained terminal document through the production
    /// postprocess lane, then commit only an applied result through the same
    /// ledger CAS + projection corridor as an explicit user edit.
    pub async fn apply_formatter_revision_from_overlay(
        &self,
        session_id: String,
        source_revision: u64,
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
        let runtime_settings = self.runtime_settings_arc().await;
        let language = runtime_settings.values().whisper_language;
        let consultation = self
            .selected_max_consultation(runtime_settings.as_ref())
            .await?;
        let turn_id = format!("{session_id}:revision:{source_revision}");
        let result = format_text_with_status_for_policy(
            &source,
            language.whisper_hint(),
            runtime_settings.as_ref(),
            consultation.as_deref().map(|agent| {
                codescribe_core::ai_formatting::FormattingConsultation {
                    agent,
                    turn_id: &turn_id,
                }
            }),
        )
        .await;
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
        presentation
            .apply_formatter_revision(session_id, source_revision, result)
            .map_err(anyhow::Error::new)
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
    ) -> AssistiveContext {
        // Start selection capture first: focus/caret state may disappear as soon
        // as the combo changes the UI. Snapshot the live transcript position
        // concurrently while the OS capture is already in flight.
        let capture_task = tokio::task::spawn_blocking(move || {
            capture_assistive_context_with_image_with_prior_frontmost(prior_frontmost_app)
        });
        let position = self.current_live_transcript_position(state).await;
        let captured_payload = capture_task.await.unwrap_or_default();
        let captured = captured_payload.context;
        let fallback = captured.clone();
        // A selected image is retained before clipboard restoration. If Cmd+C
        // produced no image, preserve the existing clipboard-image behavior.
        let image_png = match captured_payload.image_png {
            some @ Some(_) => some,
            None => tokio::task::spawn_blocking(clipboard::get_image_png_best_effort)
                .await
                .unwrap_or(None),
        };
        let mut bucket = self.context_bucket.lock().await;
        let result = (|| -> anyhow::Result<_> {
            let mut context = captured;
            let mut markers = Vec::new();
            if let Some(selected_text) = context.selected_text.take()
                && let Some(marker) = bucket.add_selection(position, selected_text)?
            {
                markers.push(marker);
            }
            if let Some(png) = image_png
                && let Some(mut marker) = bucket.add_image_png(&png)?
            {
                marker.position = position;
                markers.push(marker);
            }
            Ok((context, markers))
        })();
        drop(bucket);

        match result {
            Ok((context, markers)) => {
                let event_sink = {
                    let recorder = self.recorder.lock().await;
                    recorder
                        .as_ref()
                        .and_then(StreamingRecorder::event_sink_handle)
                };
                if let Some(event_sink) = event_sink {
                    for marker in markers {
                        event_sink.on_event(&EngineEvent::ContextMarker {
                            position: marker.position,
                            label: format!("{{{}}}", marker.label),
                        });
                    }
                } else if !markers.is_empty() {
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
            .capture_assistive_combo_context(current_state, prior_frontmost_app)
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
            DeliveryFacts {
                has_text: true,
                no_speech: false,
                auto_paste_enabled: false,
                overlay_enabled: true,
                live_stream_session: false,
                commit_required: false,
                latched_target_is_self: false,
            },
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
    async fn install_runtime_settings_generation(&self, runtime_settings: RuntimeSettingsSnapshot) {
        *self.runtime_settings.write().await = Arc::new(runtime_settings);
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

    /// Surface a refused start as typed presentation truth and flag the tray.
    /// The refusal happened before any microphone opened, so it deliberately
    /// does not invent a transcript projection or acoustic receipt.
    fn broadcast_admission_refusal(
        event_broadcast: &broadcast::Sender<IpcEvent>,
        session_id: Option<String>,
        blocker: &admission::AdmissionBlocker,
    ) {
        crate::os::tray_status::update_tray_status(crate::os::tray_status::TrayStatus::Error);
        let status = PresentationStatusProjection::admission_refused(
            session_id,
            blocker.code(),
            format!("{} — {}", blocker.explanation(), blocker.action()),
        );
        Self::broadcast_presentation_status(event_broadcast, &status);
    }

    fn broadcast_apple_preflight_refusal(
        event_broadcast: &broadcast::Sender<IpcEvent>,
        session_id: String,
        error: &anyhow::Error,
    ) {
        crate::os::tray_status::update_tray_status(crate::os::tray_status::TrayStatus::Error);
        let status = PresentationStatusProjection::admission_refused(
            Some(session_id),
            "admission_speech_recognition_unavailable",
            format!(
                "Apple dictation could not start: {error:#} — Enable Speech Recognition for Codescribe in System Settings › Privacy & Security › Speech Recognition."
            ),
        );
        Self::broadcast_presentation_status(event_broadcast, &status);
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
        use codescribe_core::audio::capture_receipt::{CaptureLevelAccumulator, CapturePathMeta};
        use codescribe_core::config::energy_calibration::{
            EnergyCalibrationArtifact, EnergyCalibrationProfile, SOURCE_GUIDED_CAPTURE,
            energy_calibration_path,
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
            .map_err(|refusal| anyhow::anyhow!("calibration_refused: {refusal}"))?;
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
        let intent = DeliveryIntent::OverlayInsert;
        let decision =
            resolve_delivery_route(intent, overlay_insert_facts(!trimmed.is_empty(), false));
        info!(
            "{}",
            format_delivery_route_line(intent, decision, target_app.as_deref())
        );
        if trimmed.is_empty() || decision.route == DeliveryRoute::ArchiveOnly {
            return Ok(OverlayPasteResult {
                delivery: OverlayPasteDelivery::Noop,
                target_app_name: None,
                frontmost_app_name: None,
                deferred_insert_shortcut: None,
                deferred_insert_failure: None,
            });
        }
        let config = self.get_config().await;
        let payload = self.delivery_tagger.render(trimmed, &config, None);
        if decision.route == DeliveryRoute::DeferredInsert {
            return self
                .arm_overlay_text(&payload, target_app, Some("Codescribe".to_string()))
                .await;
        }

        self.execute_clipboard_paste(payload, target_app, "Overlay paste")
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
    ) -> Result<OverlayPasteResult> {
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
            focus_confirmed,
            target_observed_frontmost,
            frontmost_is_external,
        );

        let config = self.get_config().await;
        let preflight = clipboard::synthetic_paste_preflight();

        let mut deferred_insert_shortcut = None;
        let mut deferred_insert_failure = None;
        let delivery = if focus_confirmed && preflight.can_post_events() {
            clipboard::paste_and_restore(&paste_text)
                .with_context(|| format!("{context}: failed to paste"))?;
            OverlayPasteDelivery::Pasted
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
        self.deliver_stop_transcript_with_sink(
            take_id,
            text,
            (assistive, force_ai, capture_turn, seal_refused),
            &config,
            |route, text, target| async move {
                if route == DeliveryRoute::DeferredInsert {
                    self.arm_overlay_text(&text, target, Some("Codescribe".to_string()))
                        .await
                } else {
                    self.execute_clipboard_paste(text, target, "Stop-path paste")
                        .await
                }
            },
        )
        .await
    }

    /// Settle the reducer document at microphone close. The later drain may
    /// revise overlay/history, but cannot paste a second document into this app.
    async fn deliver_frozen_canvas_at_stop(
        &self,
        take_id: Option<&str>,
        assistive: bool,
        force_ai: bool,
        capture_turn: CaptureTurnIntent,
        stop_start: std::time::Instant,
        last_window_close_ms: u128,
    ) -> Result<Option<String>> {
        if take_delivers_to_composer(capture_turn) {
            return Ok(None);
        }
        let Some(take_id) = take_id else {
            return Ok(None);
        };
        let presentation = self.active_presentation.read().await.clone();
        let Some(snapshot) = presentation.and_then(|emitter| emitter.visible_canvas_snapshot())
        else {
            return Ok(None);
        };
        if snapshot.session_id != take_id {
            return Ok(None);
        }
        if snapshot.text.trim().is_empty() {
            return Ok(None);
        }
        self.deliver_stop_transcript(
            Some(take_id),
            &snapshot.text,
            assistive,
            force_ai,
            capture_turn,
            false,
        )
        .await?;
        info!(
            stop_to_delivery_ms = stop_start.elapsed().as_millis(),
            last_window_close_ms,
            capture_epoch = snapshot.capture_epoch,
            reducer_revision = snapshot.revision,
            preview_only_words = snapshot.preview_only_words,
            "stop canvas delivery settled"
        );
        Ok(Some(snapshot.text))
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
        let notes_save_only = config.quick_notes_enabled && config.quick_notes_save_only;
        let intent = delivery_intent_from_session(assistive, force_ai, notes_save_only);
        let latched_target = self.pre_overlay_frontmost_app.read().await.clone();
        let decision = resolve_delivery_route(
            intent,
            DeliveryFacts {
                has_text: !trimmed.is_empty(),
                no_speech: false,
                auto_paste_enabled: config.auto_paste_enabled,
                overlay_enabled: config.transcription_overlay_enabled,
                live_stream_session: false,
                commit_required: false,
                latched_target_is_self: false,
            },
        );
        info!(
            seal_refused,
            "{}",
            format_delivery_route_line(intent, decision, latched_target.as_deref())
        );
        if !matches!(
            decision.route,
            DeliveryRoute::ClipboardPaste | DeliveryRoute::DeferredInsert
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
        let outcome = sink(decision.route, payload, latched_target).await;
        self.finish_stop_delivery(outcome, seal_refused).await
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
                let disposition = if matches!(
                    result.delivery,
                    OverlayPasteDelivery::Noop
                        | OverlayPasteDelivery::AccessibilityPermissionNeeded
                ) {
                    TranscriptDelivery::Retained
                } else {
                    TranscriptDelivery::SinkAccepted
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
            return Ok(OverlayPasteResult {
                delivery: OverlayPasteDelivery::Noop,
                target_app_name: None,
                frontmost_app_name: None,
                deferred_insert_shortcut: None,
                deferred_insert_failure: None,
            });
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
    async fn cancel_pending_hold_start(&self) {
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
        recorder.set_level_callback(None);
    }

    /// Bring the recorder to a clean pre-start state: force-stop a stream left
    /// active by a previous session, then clear its callbacks. Refusing to
    /// start here would strand the user behind a session they cannot see.
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
            &self.event_broadcast,
        )
        .await;
        *self.assistive_context.write().await = None;
        self.start_transition_in_flight
            .store(false, Ordering::SeqCst);
        self.assistive_loop_active.store(false, Ordering::SeqCst);
        self.toggle_user_has_text.store(false, Ordering::SeqCst);
        self.toggle_assistant_has_text
            .store(false, Ordering::SeqCst);
        // `state` becomes Idle only once the rest of the session state is consistent.
        self.set_state(State::Idle).await;
    }

    /// End the active Bus session exactly once with a typed reason. This is
    /// the only controller call site of `publish_ended`: the active-take reset
    /// and the pre-active hold unwind both go through here, so no second
    /// lifecycle owner can drift on when the terminal line is written. An
    /// installed Bus that never started ends silently (Bus idempotency).
    async fn end_transcript_bus(
        slot: &RwLock<Option<Arc<TranscriptBus>>>,
        reason: TranscriptSessionEndReason,
        session_wav_exists: bool,
        delivery: TranscriptDelivery,
        delivery_text: Option<String>,
        event_broadcast: &broadcast::Sender<IpcEvent>,
    ) {
        let ended_bus = slot.write().await.take();
        if let Some(bus) = ended_bus
            && let Some(event) = bus.publish_ended_with_delivery_text(
                reason,
                session_wav_exists,
                delivery,
                delivery_text,
            )
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
        if let Some(rec) = rec {
            if rec.recorder.is_active()
                && let Err(stop_err) = rec.stop_and_discard_path().await
            {
                warn!("Hold-start unwind: stale-session stop failed: {stop_err}");
            }
            Self::clear_recorder_callbacks(rec);
        }
        // A hold that never became a recording delivered nothing; the take is
        // not "failed delivery", it had no delivery attempt at all.
        Self::end_transcript_bus(
            &session.active_transcript_bus,
            abort.bus_reason(),
            false,
            TranscriptDelivery::Unattempted,
            None,
            &session.event_broadcast,
        )
        .await;
        *session.active_presentation.write().await = None;
        *session.session_id.write().await = None;
        *session.assistive_context.write().await = None;
        *session.pre_overlay_frontmost_app.write().await = None;
        set_assistive_session(false);
        crate::os::hold_badge::hide_hold_badge();
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
        preview_deltas_enabled: bool,
        event_broadcast: broadcast::Sender<IpcEvent>,
        transcript_bus: Option<Arc<TranscriptBus>>,
        acoustic_ledger: Option<
            Arc<std::sync::Mutex<codescribe_core::pipeline::acoustic_ledger::AcousticLedger>>,
        >,
        delivery_tagger: Arc<TranscriptDeliveryTagger>,
    ) -> RecordingEventPipeline {
        let delta_sink = preview_deltas_enabled.then(|| {
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
        let presentation = Arc::new(
            PresentationEmitter::new_with_authority(
                transcript_buffer,
                delta_sink,
                None,
                transcript_bus,
                acoustic_ledger,
                Some(projection_callback),
            )
            .with_cursor_observer(Arc::new(move |projection| {
                crate::os::hold_badge::update_transcript(
                    cursor_token,
                    &projection.text,
                    projection.degraded,
                );
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
            })),
        );
        let presentation_sink: Arc<dyn codescribe_core::pipeline::contracts::EventSink> =
            presentation.clone();
        let ipc_sink: Arc<dyn codescribe_core::pipeline::contracts::EventSink> =
            Arc::new(helpers::IpcBroadcastSink::new(event_broadcast));
        let delivery_tag_sink: Arc<dyn codescribe_core::pipeline::contracts::EventSink> =
            delivery_tagger;
        let event_sink = Arc::new(codescribe_core::pipeline::sinks::FanoutEventSink::new(
            vec![presentation_sink, ipc_sink, delivery_tag_sink],
        ));
        RecordingEventPipeline {
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
        let (level_tx, mut level_rx) = mpsc::channel::<f32>(AUDIO_LEVEL_QUEUE_CAPACITY);
        recorder.set_level_callback(Some(Arc::new(move |rms| {
            let _ = level_tx.try_send(rms);
        })));

        tokio::spawn(async move {
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
    ) -> Arc<PresentationEmitter> {
        Self::configure_level_broadcast(recorder, event_broadcast.clone());
        let acoustic_ledger = recorder.acoustic_ledger_handle();
        let pipeline = Self::build_recording_event_sink(
            recorder.transcript_buffer_handle(),
            preview_deltas_enabled,
            event_broadcast,
            transcript_bus,
            acoustic_ledger,
            delivery_tagger,
        );
        recorder.set_event_sink(Some(pipeline.event_sink));
        pipeline.presentation
    }

    /// Wire level metering and the event sink for a toggle / hands-off session,
    /// which is ONE continuous recorder session (ADR 2026-05-28 Faza 1).
    fn configure_toggle_event_sink(
        recorder: &mut StreamingRecorder,
        preview_deltas_enabled: bool,
        _flush_voice_chat_on_vad_end: bool,
        event_broadcast: broadcast::Sender<IpcEvent>,
        transcript_bus: Option<Arc<TranscriptBus>>,
        delivery_tagger: Arc<TranscriptDeliveryTagger>,
    ) -> Arc<PresentationEmitter> {
        // Hands-off is ONE continuous recorder session (ADR 2026-05-28 Faza 1).
        // Normal hands-off uses cumulative SessionRendered deltas in the transcription overlay.
        //
        // Assistive hands-off is intentionally callback-driven: every finalized utterance
        // appends into the current chat user bubble, and VAD end commits that bubble to the
        // agent without stopping the recorder. Do not route assistive live preview deltas
        // into the same bubble, or previews and finals will duplicate.
        Self::configure_level_broadcast(recorder, event_broadcast.clone());
        let acoustic_ledger = recorder.acoustic_ledger_handle();
        let pipeline = Self::build_recording_event_sink(
            recorder.transcript_buffer_handle(),
            preview_deltas_enabled,
            event_broadcast,
            transcript_bus,
            acoustic_ledger,
            delivery_tagger,
        );
        recorder.set_event_sink(Some(pipeline.event_sink));
        pipeline.presentation
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
    pub async fn handle_hotkey_event(self: &Arc<Self>, event: HotkeyInput) -> Result<()> {
        // Stop gestures enter before mode updates: a RAW toggle during hold
        // must not rewrite the take's destination while asking to end it.
        let stop_gesture = {
            let Ok(state) = self.state.try_read() else {
                return Err(anyhow::anyhow!("hotkey admission unavailable"));
            };
            matches!(
                (event.key_type, event.action, *state),
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
                && *state == State::RecHold)
        };
        if stop_gesture {
            return self.stop_recording_from_external_surface().await;
        }
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
                                .capture_assistive_combo_context(current_state, prior_frontmost_app)
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
                                .capture_assistive_combo_context(current_state, prior_frontmost_app)
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
        match event.key_type {
            HotkeyType::Hold => self.handle_hold_event(event).await,
            HotkeyType::Toggle => self.handle_toggle_event(event).await,
            HotkeyType::Conversation => self.handle_conversation_event(event).await,
        }
    }

    /// Handle hold-type hotkey events
    async fn handle_hold_event(self: &Arc<Self>, event: HotkeyInput) -> Result<()> {
        match event.action {
            HotkeyAction::Down => {
                let current_state = self.current_state().await;
                if current_state == State::Idle {
                    self.schedule_hold_start(event.assistive).await?;
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
                self.start_toggle_recording(event.assistive, CaptureTurnIntent::HandsFree)
                    .await?;
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

    /// Start conversation mode (full-duplex Moshi)
    ///
    /// Initializes ConversationEngine and AudioPlayer, then starts the audio
    /// processing loop that feeds mic input to Moshi and plays responses.
    async fn start_conversation_mode(&self) -> Result<()> {
        let _serial = self.serial_lock.lock().await;
        if self.shutdown_requested.load(Ordering::SeqCst)
            || self.current_state().await != State::Idle
        {
            return Err(anyhow::anyhow!(
                "conversation capture admission unavailable"
            ));
        }
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
    async fn schedule_hold_start(&self, assistive: bool) -> Result<()> {
        // Scheduling selects the take generation. Refresh and every actual
        // start/stop transition cross this same boundary, while the spawned
        // task itself is never awaited under the guard.
        let _scheduling_guard = self.serial_lock.lock().await;

        if self.shutdown_requested.load(Ordering::SeqCst) {
            return Err(anyhow::anyhow!("capture admission closed for shutdown"));
        }
        // Cancel any existing delayed start before selecting the next Arc.
        self.cancel_pending_hold_start().await;
        self.refresh_pending_runtime_settings_locked().await?;
        let task_generation = self.hold_start_generation.load(Ordering::SeqCst);
        let runtime_settings = self.runtime_settings_arc().await;
        let config = runtime_settings.values().clone();

        let live_formatting_agent = if !assistive
            && !*self.force_raw_mode.read().await
            && config.ai_formatting_enabled
        {
            match self
                .selected_max_consultation(runtime_settings.as_ref())
                .await
            {
                Ok(consultation) => consultation
                    .map(|agent| agent as Arc<dyn codescribe_core::ai_formatting::FormattingAgent>),
                Err(error) => {
                    warn!(%error, "Max live executor unavailable; capture remains transcription-only");
                    None
                }
            }
        } else {
            None
        };

        // Hold mode never runs the assistive loop
        self.assistive_loop_active.store(false, Ordering::SeqCst);
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
        // Every exit after the start guard releases exactly these slots.
        let hold_session = HoldStartSession {
            session_id: Arc::clone(&self.session_id),
            active_transcript_bus: Arc::clone(&self.active_transcript_bus),
            active_presentation: Arc::clone(&self.active_presentation),
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

            let _start_guard = AtomicFlagGuard::new(Arc::clone(&start_transition_in_flight));

            // Generate session ID
            let new_session_id = Uuid::new_v4().to_string();
            *hold_session.session_id.write().await = Some(new_session_id.clone());

            info!("Starting hold recording (session={})", new_session_id);

            let hold_mode = *hold_mode.read().await;
            let is_assistive = matches!(hold_mode, HoldMode::Chat | HoldMode::Selection);
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
            let overlay_enabled = apply_runtime_transcription_profile(
                &config,
                runtime_settings.user_settings(),
                is_assistive,
            );

            // Apple live must-have: refuse start before audio when Speech is not
            // ready (empty mid-take death is not an acceptable product mode).
            // Runs BEFORE the recorder lock and on the blocking pool: the probe
            // spawns a bridge child and can block on the Speech TCC dialog for
            // as long as the user takes — holding the recorder mutex (or a
            // runtime worker) for that window froze stop/tray/second-hotkey.
            if !cfg!(test) {
                let preflight =
                    tokio::task::spawn_blocking(codescribe_core::stt::preflight_apple_live_ready)
                        .await
                        .unwrap_or_else(|join| {
                            Err(anyhow::anyhow!("Apple STT preflight task panicked: {join}"))
                        });
                if let Err(e) = preflight {
                    error!("Hold-start aborted (Apple STT preflight): {e:#}");
                    Self::broadcast_apple_preflight_refusal(
                        &event_broadcast,
                        new_session_id.clone(),
                        &e,
                    );
                    Self::unwind_hold_start(&hold_session, None, HoldStartAbort::PreflightRefused)
                        .await;
                    return;
                }
            }

            // Acoustic admission must-have (same gate as toggle): refuse before
            // the recorder lock and before any microphone opens. Hold has no
            // return channel to the UI, so the refusal rides the engine
            // Warning channel the bridge forwards as a terminal error.
            if !cfg!(test) {
                // `.clone()` (not `Arc::clone`) on purpose: C15D counts one
                // recorder binding per start body; this is the same Arc.
                let admission_settings = runtime_settings.clone();
                let verdict = tokio::task::spawn_blocking(move || {
                    admission::evaluate_live_admission_arc(&admission_settings)
                })
                .await
                .unwrap_or_else(|join| {
                    Err(admission::AdmissionBlocker::CaptureDeviceUnavailable {
                        reason: format!("admission probe panicked: {join}"),
                    })
                });
                match verdict {
                    Ok(grant) => info!(
                        device = %grant.device_name,
                        sample_rate = grant.sample_rate,
                        calibration_version = %grant.calibration_version,
                        "acoustic admission granted for hold start"
                    ),
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
            rec.bind_session_authority(new_session_id.clone(), Arc::clone(&runtime_settings));
            rec.set_live_formatting_agent(live_formatting_agent);
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
            })
            .map(Arc::new);
            // Install the Bus before the recorder starts: from here every exit
            // runs through `unwind_hold_start`, which ends whatever is in this
            // slot — silently while un-started, with one terminal line once
            // `publish_started` has been written.
            *hold_session.active_transcript_bus.write().await = transcript_bus.clone();

            // Runtime pipeline is always event-based. Hold mode has no utterance callback;
            // text is finalized on key-up in `finish_recording`.
            let presentation = Self::configure_hold_event_sink(
                rec,
                is_assistive || overlay_enabled,
                event_broadcast.clone(),
                transcript_bus.clone(),
                Arc::clone(&hold_session.delivery_tagger),
            );
            presentation.set_literal_delivery(*hold_session.force_raw_mode.read().await);
            *hold_session.active_presentation.write().await = Some(presentation);
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
                        let presentation = Self::configure_hold_event_sink(
                            rec,
                            is_assistive || overlay_enabled,
                            event_broadcast.clone(),
                            transcript_bus.clone(),
                            Arc::clone(&hold_session.delivery_tagger),
                        );
                        presentation
                            .set_literal_delivery(*hold_session.force_raw_mode.read().await);
                        *hold_session.active_presentation.write().await = Some(presentation);
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
        let runtime_settings = self.runtime_settings_arc().await;
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
        let language = config.whisper_language;
        let toggle_silence_sec = config.toggle_silence_sec;
        let beep_enabled = config.beep_on_start;
        let sound_volume = config.sound_volume;
        let overlay_enabled = apply_runtime_transcription_profile(
            config,
            runtime_settings.user_settings(),
            is_assistive,
        );

        // Apple live must-have preflight, BEFORE the recorder lock and on the
        // blocking pool: the probe spawns a bridge child and can block on the
        // Speech TCC dialog indefinitely — holding the recorder mutex (or a
        // runtime worker) for that window froze every other recorder surface.
        if !cfg!(test) {
            let preflight =
                tokio::task::spawn_blocking(codescribe_core::stt::preflight_apple_live_ready)
                    .await
                    .unwrap_or_else(|join| {
                        Err(anyhow::anyhow!("Apple STT preflight task panicked: {join}"))
                    });
            if let Err(e) = preflight {
                // Must log the actual cause — silent "resetting flags" made padaka undiagnosable.
                error!("Toggle-start aborted (Apple STT preflight): {e:#}");
                Self::broadcast_apple_preflight_refusal(
                    &self.event_broadcast,
                    new_session_id.clone(),
                    &e,
                );
                self.reset_session_after_start_failure("Toggle-start Apple STT preflight")
                    .await;
                return Err(e);
            }
        }

        // Acoustic admission must-have: a take whose occurrences can never
        // qualify (no measured calibration, seal lane disarmed) must be refused
        // HERE, before the recorder lock and before any microphone opens — not
        // recorded into a WAV that grows while the Bus stays on session_started.
        if !cfg!(test) {
            match self.admission_readiness().await {
                Ok(grant) => info!(
                    device = %grant.device_name,
                    sample_rate = grant.sample_rate,
                    calibration_version = %grant.calibration_version,
                    "acoustic admission granted for toggle start"
                ),
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
        recorder.bind_session_authority(new_session_id.clone(), Arc::clone(&runtime_settings));
        self.delivery_tagger.begin(
            if is_assistive { "agent" } else { "dictation" },
            config.whisper_language.as_str(),
        );
        *self.composer_delivery_payload.write().await = None;
        let live_formatting_agent = if !is_assistive
            && capture_turn.schedules_live_formatting()
            && !*self.force_raw_mode.read().await
            && config.ai_formatting_enabled
        {
            match self
                .selected_max_consultation(runtime_settings.as_ref())
                .await
            {
                Ok(consultation) => consultation
                    .map(|agent| agent as Arc<dyn codescribe_core::ai_formatting::FormattingAgent>),
                Err(error) => {
                    warn!(%error, "Max live executor unavailable; capture remains transcription-only");
                    None
                }
            }
        } else {
            None
        };
        recorder.set_live_formatting_agent(live_formatting_agent);
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
        })
        .map(Arc::new);

        // Runtime pipeline is always event-based.
        let presentation = Self::configure_toggle_event_sink(
            recorder,
            overlay_enabled,
            is_assistive,
            self.event_broadcast.clone(),
            transcript_bus.clone(),
            Arc::clone(&self.delivery_tagger),
        );
        presentation.set_literal_delivery(*self.force_raw_mode.read().await);
        *self.active_presentation.write().await = Some(presentation);
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
                let presentation = Self::configure_toggle_event_sink(
                    recorder,
                    overlay_enabled,
                    is_assistive,
                    self.event_broadcast.clone(),
                    transcript_bus.clone(),
                    Arc::clone(&self.delivery_tagger),
                );
                presentation.set_literal_delivery(*self.force_raw_mode.read().await);
                *self.active_presentation.write().await = Some(presentation);
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
            let was_active = recorder.close_capture().await;
            let last_window_close_ms =
                await_last_window_close_for_delivery(recorder, stop_start).await;
            let initial_delivery = if let Some(last_window_close_ms) = last_window_close_ms {
                self.deliver_frozen_canvas_at_stop(
                    session_id_snapshot.as_deref(),
                    assistive,
                    force_ai,
                    capture_turn,
                    stop_start,
                    last_window_close_ms,
                )
                .await
            } else {
                Ok(None)
            };
            let stopped =
                stop_recorder_for_terminal(recorder, session_id_snapshot.as_deref(), Some(was_active)).await;
            rec_stop_secs = phase2.elapsed().as_secs_f64();
            // Read the take's own intent before the per-take state is cleared.
            // The recorder is the single owner of this fact; the stop path must
            // not re-derive it from the assistive flag or the active screen.
            Self::clear_recorder_callbacks(recorder);
            drop(recorder_guard);
            // The session is over whichever way `stop()` went; the engine that
            // served it is the same on a clean stop and on a refused seal.
            Self::publish_live_serving_verdict(serving_engine);
            let (streaming_text, raw_audio_path_opt) = match stopped {
                Ok(stopped) => stopped,
                Err(err) => {
                    if initial_delivery.as_ref().ok().is_some_and(Option::is_some) {
                        return self
                            .process_terminal_stop_error(err, |_| async {
                                Ok(TranscriptDelivery::SinkAccepted)
                            })
                            .await;
                    }
                    if let Err(delivery_error) = &initial_delivery {
                        warn!(%delivery_error, "initial delivery failed before terminal stop refusal");
                        return Err(err);
                    }
                    initial_delivery?;
                    return self.process_terminal_stop_error(err, |text| async move {
                        self.deliver_stop_transcript(
                            session_id_snapshot.as_deref(),
                            &text,
                            assistive,
                            force_ai,
                            capture_turn,
                            true,
                        )
                        .await
                    }).await;
                }
            };
            let initial_text = initial_delivery?;
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
                .format_composer_turn_once(capture_turn, streaming_text)
                .await;
            if let Some(path) = raw_audio_path_opt.as_deref() {
                retain_session_audio(
                    session_id_snapshot.as_deref(),
                    path,
                    codescribe_core::state::SessionTranscriptArchive::from_committed(
                        streaming_text.as_str(),
                    ),
                );
            }
            if initial_text.is_none() {
                self.deliver_stop_transcript(
                    session_id_snapshot.as_deref(),
                    &streaming_text,
                    assistive,
                    force_ai,
                    capture_turn,
                    false,
                )
                .await?;
            }
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
        let Ok(slot) = self.capture_settlement.try_lock() else {
            return false;
        };
        if slot
            .as_ref()
            .is_some_and(|op| !op.task.is_finished() || op.result.borrow().is_none())
        {
            return false;
        }
        let Ok(_serial) = self.serial_lock.try_lock() else {
            return false;
        };
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

    /// Stop-current is admitted without queuing behind a start or a terminal
    /// owner. A delayed gesture must not discover a replacement after waiting.
    pub async fn stop_current_capture(self: &Arc<Self>) -> Result<CaptureStopOutcome> {
        let receiver = {
            let Ok(mut slot) = self.capture_settlement.try_lock() else {
                return Ok(CaptureStopOutcome::AdmissionUnavailable);
            };
            if let Some(operation) = slot
                .as_ref()
                .filter(|op| !op.task.is_finished() || op.result.borrow().is_none())
            {
                operation.result.clone()
            } else {
                let Ok(_serial) = self.serial_lock.try_lock() else {
                    return Ok(CaptureStopOutcome::AdmissionUnavailable);
                };
                let Ok(state) = self.state.try_read() else {
                    return Ok(CaptureStopOutcome::AdmissionUnavailable);
                };
                if *state == State::Idle {
                    // No active terminal body to cancel. Invalidate a delayed
                    // hold that has not crossed this same admission boundary.
                    self.hold_start_generation.fetch_add(1, Ordering::SeqCst);
                    return Ok(CaptureStopOutcome::NoLiveCapture);
                }
                if !matches!(*state, State::RecHold | State::RecToggle) {
                    // Conversation has a separate loop owner, and Busy without
                    // a settlement receipt is not proof of a released resource.
                    return Ok(CaptureStopOutcome::AdmissionUnavailable);
                }
                let Ok(identity) = self.session_id.try_read() else {
                    return Ok(CaptureStopOutcome::AdmissionUnavailable);
                };
                let Some(id) = identity.as_deref() else {
                    return Ok(CaptureStopOutcome::AdmissionUnavailable);
                };
                self.register_capture_stop(&mut slot, id)
            }
        };
        Self::await_capture_stop(receiver).await
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
    async fn stop_external_capture(&self, expected: Option<&str>) -> Result<CaptureStopOutcome> {
        let _serial = self.serial_lock.lock().await;
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
            self.stop_toggle_and_adjudicate_inner(expected).await
        } else if matches!(state, State::RecHold | State::RecToggle) {
            #[cfg(test)]
            self.observe_capture_settlement(CaptureSettlementStage::Admitted);
            self.cancel_pending_hold_start().await;
            self.finish_recording_locked()
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
        let controller = Arc::clone(self);
        let owned_id = capture_id.to_owned();
        let task = tokio::spawn(async move {
            let settled = controller
                .stop_external_capture(Some(&owned_id))
                .await
                .map_err(|error| format!("{error:#}"));
            sender.send_replace(Some(settled));
        });
        *slot = Some(CaptureSettlement {
            capture_id: capture_id.to_owned(),
            result: receiver.clone(),
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
    /// On success the committed revision is the delivered text, so the Bus, the
    /// delivery buffer and the ledger CAS keep seeing the same bytes.
    async fn format_composer_turn_once(
        &self,
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
        }) = presentation.terminal_formatter_request()
        else {
            debug!("Composer turn: nothing to format at terminal; no provider call issued");
            return committed_text;
        };
        let runtime_settings = self.runtime_settings_arc().await;
        let language = runtime_settings.values().whisper_language;
        // The one paid call this take is allowed. Same production entry point
        // the explicit overlay formatter uses; no second lane, no retry loop.
        let consultation = match self
            .selected_max_consultation(runtime_settings.as_ref())
            .await
        {
            Ok(consultation) => consultation,
            Err(error) => {
                warn!(%error, "Max consultation unavailable; preserving committed transcript");
                return committed_text;
            }
        };
        let turn_id = format!("{session_id}:revision:{source_revision}");
        let result = format_text_with_status_for_policy(
            &source_text,
            language.whisper_hint(),
            runtime_settings.as_ref(),
            consultation.as_deref().map(|agent| {
                codescribe_core::ai_formatting::FormattingConsultation {
                    agent,
                    turn_id: &turn_id,
                }
            }),
        )
        .await;
        match presentation.apply_formatter_revision(session_id, source_revision, result) {
            Ok(commit) => {
                info!(
                    revision = commit.revision,
                    receipt = %commit.provenance_receipt,
                    "Composer turn formatted once at terminal processing"
                );
                commit.rendered_text
            }
            Err(refusal) => {
                info!(%refusal, "Composer turn terminal formatting refused; committed text stands");
                committed_text
            }
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

        let result = self
            .process_recording(session_id, assistive, hold_mode, force_raw, force_ai)
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
    /// - Neither: Toggle mode - respects AI_FORMATTING_ENABLED setting
    async fn process_recording(
        &self,
        session_id: Option<String>,
        assistive: bool,
        hold_mode: HoldMode,
        force_raw: bool,
        force_ai: bool,
    ) -> Result<ProcessRecordingOutcome> {
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
        let stop_start = std::time::Instant::now();
        let was_active = recorder.close_capture().await;
        let last_window_close_ms = await_last_window_close_for_delivery(recorder, stop_start).await;
        let initial_delivery = if let Some(last_window_close_ms) = last_window_close_ms {
            self.deliver_frozen_canvas_at_stop(
                take_id.as_deref(),
                assistive,
                force_ai,
                CaptureTurnIntent::HandsFree,
                stop_start,
                last_window_close_ms,
            )
            .await
        } else {
            Ok(None)
        };
        let stopped =
            stop_recorder_for_terminal(recorder, take_id.as_deref(), Some(was_active)).await;
        Self::clear_recorder_callbacks(recorder);
        drop(recorder_guard); // Release lock
        Self::publish_live_serving_verdict(serving_engine);
        let (streaming_text, raw_audio_path_opt) = match stopped {
            Ok(stopped) => stopped,
            Err(err) => {
                if initial_delivery.as_ref().ok().is_some_and(Option::is_some) {
                    return self
                        .process_terminal_stop_error(err, |_| async {
                            Ok(TranscriptDelivery::SinkAccepted)
                        })
                        .await;
                }
                if let Err(delivery_error) = &initial_delivery {
                    warn!(%delivery_error, "initial delivery failed before terminal stop refusal");
                    return Err(err);
                }
                initial_delivery?;
                return self
                    .process_terminal_stop_error(err, |text| async move {
                        self.deliver_stop_transcript(
                            take_id.as_deref(),
                            &text,
                            assistive,
                            force_ai,
                            // The hold path has no composer surface: no caller here
                            // can open a one-turn take.
                            CaptureTurnIntent::HandsFree,
                            true,
                        )
                        .await
                    })
                    .await;
            }
        };
        let initial_text = initial_delivery?;

        if let Some(path) = raw_audio_path_opt.as_deref() {
            retain_session_audio(
                take_id.as_deref(),
                path,
                codescribe_core::state::SessionTranscriptArchive::from_committed(
                    streaming_text.as_str(),
                ),
            );
        }
        // Ctrl-hold literal (`force_raw`) and the hold flavour are sink-time
        // facts already consumed when the emitter was built; delivery reads
        // only the frozen intent.
        let _ = (hold_mode, force_raw);
        if initial_text.is_none() {
            self.deliver_stop_transcript(
                take_id.as_deref(),
                &streaming_text,
                assistive,
                force_ai,
                CaptureTurnIntent::HandsFree,
                false,
            )
            .await?;
        }
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

    /// Check if controller is busy processing
    pub async fn is_busy(&self) -> bool {
        self.current_state().await == State::Busy
    }
}

impl Default for RecordingController {
    /// Build a controller with production defaults (`RecordingController::new`).
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod terminal_delivery_target_falsifiers {
    use super::*;

    #[tokio::test]
    async fn failed_stop_publishes_failure_instead_of_completed() {
        use crate::presentation::transcript_bus::{
            CleanTranscriptEvent, TranscriptMode, TranscriptProjectionPhase, TranscriptSession,
        };
        for failure in ["terminal seal refused", "recorder failed after drain"] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("events.jsonl");
            let controller = RecordingController::new_without_keychain();
            let bus = TranscriptBus::open_at(
                TranscriptSession {
                    session_id: "failed-stop".to_string(),
                    mode: TranscriptMode::Dictation,
                    has_latched_target: false,
                    latched_target_is_self: false,
                },
                path.clone(),
                None,
            )
            .unwrap();
            bus.publish_started();
            *controller.active_transcript_bus.write().await = Some(Arc::new(bus));

            controller
                .reset_finished_recording_state(&Err(anyhow::anyhow!(failure)))
                .await;

            let rows: Vec<CleanTranscriptEvent> = std::fs::read_to_string(path)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(rows.len(), 2);
            assert_eq!(
                rows[1].end_reason,
                Some(TranscriptSessionEndReason::TranscriptionFailed)
            );
            assert_eq!(rows[1].phase, TranscriptProjectionPhase::Error);
            assert_eq!(controller.current_state().await, State::Idle);
        }
    }

    /// A completed take returns the recorder to Idle before the overlay Insert
    /// click. Its foreign caret must survive that transition, while explicit
    /// recovery still clears the latch so a later take cannot inherit it.
    #[tokio::test]
    async fn completed_reset_preserves_target_until_explicit_recovery() {
        let controller = RecordingController::new_without_keychain();
        *controller.pre_overlay_frontmost_app.write().await = Some("Ghostty".to_string());

        controller
            .reset_finished_recording_state(&Ok(ProcessRecordingOutcome::default()))
            .await;

        assert_eq!(controller.current_state().await, State::Idle);
        assert_eq!(
            controller.paste_target_app_name().await.as_deref(),
            Some("Ghostty")
        );

        controller.reset().await;
        assert!(controller.paste_target_app_name().await.is_none());
    }

    /// Hold release goes through `finish_recording`, which first clears the
    /// delayed-start task. The pre-overlay app latched at hotkey press is the
    /// take's delivery target, not the start task's scratch: the overlay Insert
    /// click that follows the sealed canvas still needs it. Falsifier for the
    /// `target_app=None frontmost_app=Some("vc-terminal")` lines in the log.
    #[tokio::test]
    async fn hold_release_after_live_start_preserves_target_latch() {
        let controller = Arc::new(RecordingController::new_without_keychain());
        *controller.pre_overlay_frontmost_app.write().await = Some("vc-terminal".to_string());
        let started = tokio::spawn(async {});
        while !started.is_finished() {
            tokio::task::yield_now().await;
        }
        *controller.hold_start_task.lock().await = Some(started);

        controller
            .finish_recording()
            .await
            .expect("finish at Idle is an ignored race, not an error");

        assert_eq!(
            controller.paste_target_app_name().await.as_deref(),
            Some("vc-terminal"),
            "a finished hold start is a live take; its delivery target survives release"
        );
    }

    /// Releasing the hold key before the start delay elapsed means no take ever
    /// existed, so the latch captured for it must go with the cancelled task.
    #[tokio::test]
    async fn quick_hold_release_before_start_clears_target_latch() {
        let controller = RecordingController::new_without_keychain();
        *controller.pre_overlay_frontmost_app.write().await = Some("vc-terminal".to_string());
        let pending = tokio::spawn(std::future::pending::<()>());
        *controller.hold_start_task.lock().await = Some(pending);

        controller.cancel_pending_hold_start().await;

        assert!(controller.paste_target_app_name().await.is_none());
    }

    /// A start that never became a take has no terminal canvas and therefore
    /// must not leave a target behind for an unrelated later Insert click.
    #[tokio::test]
    async fn failed_start_clears_target_latch() {
        let controller = RecordingController::new_without_keychain();
        *controller.pre_overlay_frontmost_app.write().await = Some("Ghostty".to_string());

        controller
            .reset_session_after_start_failure("delivery target falsifier")
            .await;

        assert!(controller.paste_target_app_name().await.is_none());
    }

    /// One take, one destination. A composer turn belongs to the Agent draft of
    /// the thread that opened it; posting a synthetic paste as well is how a
    /// voice note lands in whatever window happened to be frontmost.
    #[tokio::test]
    async fn a_composer_turn_is_owed_to_the_composer_and_to_nothing_else() {
        let controller = RecordingController::new_without_keychain();

        controller
            .deliver_stop_transcript(
                Some("take-composer"),
                "words for the draft",
                true,
                false,
                CaptureTurnIntent::SingleTurn,
                false,
            )
            .await
            .unwrap();

        assert_eq!(
            *controller.delivery_disposition.read().await,
            TranscriptDelivery::ComposerPending
        );
    }

    /// The wrapper is created after route selection and is handed to both OS
    /// transports as one immutable payload. A deferred reuse must not wrap it
    /// a second time.
    #[tokio::test]
    async fn stop_clipboard_and_deferred_routes_receive_one_tagged_payload() {
        for (auto_paste_enabled, expected_route) in [
            (true, DeliveryRoute::ClipboardPaste),
            (false, DeliveryRoute::DeferredInsert),
        ] {
            let controller = RecordingController::new_without_keychain();
            controller.delivery_tagger.begin("dictation", "pl");
            let config = Config {
                transcript_tagging_enabled: true,
                transcript_tag_template: "<codescribe mode=\"{mode}\">{text}</codescribe>".into(),
                auto_paste_enabled,
                transcription_overlay_enabled: true,
                quick_notes_enabled: false,
                ..Config::default()
            };

            controller
                .deliver_stop_transcript_with_sink(
                    Some(if auto_paste_enabled {
                        "tagged-clipboard"
                    } else {
                        "tagged-deferred"
                    }),
                    "working text",
                    (false, false, CaptureTurnIntent::HandsFree, false),
                    &config,
                    |route, payload, _| async move {
                        assert_eq!(route, expected_route);
                        assert_eq!(
                            payload,
                            "<codescribe mode=\"dictation\">working text</codescribe>"
                        );
                        assert_eq!(payload.matches("<codescribe").count(), 1);
                        Ok(OverlayPasteResult {
                            delivery: if route == DeliveryRoute::ClipboardPaste {
                                OverlayPasteDelivery::Pasted
                            } else {
                                OverlayPasteDelivery::DeferredInsertArmed
                            },
                            target_app_name: None,
                            frontmost_app_name: None,
                            deferred_insert_shortcut: None,
                            deferred_insert_failure: None,
                        })
                    },
                )
                .await
                .unwrap();
        }
    }

    /// Composer delivery gets its own sink payload while the reducer document
    /// remains the clean text passed into this boundary.
    #[tokio::test]
    async fn composer_route_retains_tagged_delivery_separate_from_working_text() {
        let controller = RecordingController::new_without_keychain();
        controller.delivery_tagger.begin("agent", "en");
        let config = Config {
            transcript_tagging_enabled: true,
            transcript_tag_template: "[{mode}|{lang}|{conf}] {text}".into(),
            ..Config::default()
        };

        controller
            .deliver_stop_transcript_with_sink(
                Some("composer-tagged"),
                "editable draft",
                (true, false, CaptureTurnIntent::SingleTurn, false),
                &config,
                |_, _, _| async { panic!("composer route must not call an OS sink") },
            )
            .await
            .unwrap();

        assert_eq!(
            controller.composer_delivery_payload.read().await.as_ref(),
            Some(&(
                "composer-tagged".to_string(),
                "[agent|en|unknown] editable draft".to_string()
            ))
        );
    }

    /// `ComposerPending` is an obligation, so an empty capture must not create
    /// one. Nothing was said; nothing is owed.
    #[tokio::test]
    async fn an_empty_composer_turn_creates_no_delivery_obligation() {
        let controller = RecordingController::new_without_keychain();

        controller
            .deliver_stop_transcript(
                Some("take-empty"),
                "   \n  ",
                true,
                false,
                CaptureTurnIntent::SingleTurn,
                false,
            )
            .await
            .unwrap();

        assert_eq!(
            *controller.delivery_disposition.read().await,
            TranscriptDelivery::Retained
        );
    }

    #[tokio::test]
    async fn empty_stop_canvas_cannot_claim_the_take_before_terminal_words_arrive() {
        let controller = RecordingController::new_without_keychain();
        let config = Config::default();
        let first = controller
            .deliver_stop_transcript_with_sink(
                Some("open-last-window"),
                "",
                (false, false, CaptureTurnIntent::HandsFree, false),
                &config,
                |_, _, _| async { panic!("empty canvas has no sink payload") },
            )
            .await;
        assert!(first.is_ok());
        let terminal = controller
            .deliver_stop_transcript_with_sink(
                Some("open-last-window"),
                "last words",
                (false, false, CaptureTurnIntent::HandsFree, false),
                &config,
                |_, text, _| async move {
                    assert_eq!(text, "last words");
                    Ok(OverlayPasteResult {
                        delivery: OverlayPasteDelivery::Pasted,
                        target_app_name: None,
                        frontmost_app_name: None,
                        deferred_insert_shortcut: None,
                        deferred_insert_failure: None,
                    })
                },
            )
            .await;
        assert!(
            terminal.is_ok(),
            "terminal words were suppressed: {terminal:?}"
        );
    }

    #[tokio::test]
    async fn sink_admission_after_last_window_ignores_slow_tail_work() {
        let controller = RecordingController::new_without_keychain();
        let config = Config {
            auto_paste_enabled: true,
            ..Config::default()
        };
        let l1 = tokio::spawn(async { tokio::time::sleep(Duration::from_millis(650)).await });
        let recovery = tokio::spawn(async { tokio::time::sleep(Duration::from_millis(700)).await });
        let stop_start = std::time::Instant::now();
        tokio::time::sleep(Duration::from_millis(55)).await;
        let last_window_close_ms = stop_start.elapsed().as_millis();
        controller
            .deliver_stop_transcript_with_sink(
                Some("timed-take"),
                "all committed words",
                (false, false, CaptureTurnIntent::HandsFree, false),
                &config,
                |route, text, _| async move {
                    assert_eq!(route, DeliveryRoute::ClipboardPaste);
                    assert_eq!(text, "all committed words");
                    Ok(OverlayPasteResult {
                        delivery: OverlayPasteDelivery::Pasted,
                        target_app_name: None,
                        frontmost_app_name: None,
                        deferred_insert_shortcut: None,
                        deferred_insert_failure: None,
                    })
                },
            )
            .await
            .unwrap();
        let stop_to_sink_ms = stop_start.elapsed().as_millis();
        eprintln!("last_window_close_ms={last_window_close_ms} stop_to_sink_ms={stop_to_sink_ms}");
        assert!(stop_to_sink_ms < last_window_close_ms + 300);
        assert!(!l1.is_finished());
        assert!(!recovery.is_finished());
        l1.await.unwrap();
        recovery.await.unwrap();
    }

    /// A refused seal degrades the coverage claim, not the route. The committed
    /// words still belong to the composer that captured them.
    #[tokio::test]
    async fn a_refused_seal_keeps_the_composer_as_the_destination() {
        let controller = RecordingController::new_without_keychain();

        controller
            .deliver_stop_transcript(
                Some("take-refused"),
                "partial but real words",
                true,
                false,
                CaptureTurnIntent::SingleTurn,
                true,
            )
            .await
            .unwrap();

        assert_eq!(
            *controller.delivery_disposition.read().await,
            TranscriptDelivery::ComposerPending
        );
    }

    /// The converse, which is the assertion that keeps the branch honest: a
    /// hands-free take is never owed to the composer.
    #[tokio::test]
    async fn a_hands_free_take_is_never_owed_to_the_composer() {
        let controller = RecordingController::new_without_keychain();

        let result = controller
            .deliver_stop_transcript_with_sink(
                Some("take-hands-free"),
                "dictated words",
                (false, false, CaptureTurnIntent::HandsFree, false),
                &controller.get_config().await,
                |_, _, _| async { Err(anyhow::anyhow!("injected sink refusal")) },
            )
            .await;

        if let Err(error) = result {
            assert!(error.is::<StopDeliveryFailure>());
        }

        assert_ne!(
            *controller.delivery_disposition.read().await,
            TranscriptDelivery::ComposerPending
        );
    }

    /// A named stop refuses when a different capture owns the microphone, and
    /// says nothing is live when nothing is. Both are refusals, and they are
    /// different refusals: one must leave a foreign take running.
    #[tokio::test]
    async fn the_capture_gate_tells_a_foreign_take_apart_from_no_take() {
        let controller = RecordingController::new_without_keychain();

        assert_eq!(
            controller.capture_gate(Some("mine")).await,
            CaptureStopOutcome::NoLiveCapture
        );

        *controller.session_id.write().await = Some("theirs".to_string());
        assert_eq!(
            controller.capture_gate(Some("mine")).await,
            CaptureStopOutcome::ForeignCapture
        );

        *controller.session_id.write().await = Some("mine".to_string());
        assert_eq!(
            controller.capture_gate(Some("mine")).await,
            CaptureStopOutcome::Stopped
        );

        *controller.session_id.write().await = Some(format!("mine{STOPPING_SUFFIX}"));
        assert_eq!(
            controller.capture_gate(Some("mine")).await,
            CaptureStopOutcome::AlreadyStopping
        );
    }

    /// The hotkey and tray contract is unchanged: an unnamed stop still stops
    /// whatever is live, including a take it did not open.
    #[tokio::test]
    async fn an_unnamed_stop_keeps_its_unconditional_contract() {
        let controller = RecordingController::new_without_keychain();
        *controller.session_id.write().await = Some("someone-elses-take".to_string());

        assert_eq!(
            controller.capture_gate(None).await,
            CaptureStopOutcome::Stopped
        );
    }

    /// A conditional stop that loses the identity race must not fall through to
    /// the stop path. The foreign take keeps the microphone.
    #[tokio::test]
    async fn a_conditional_stop_for_a_foreign_take_does_not_run_the_stop_path() {
        let controller = Arc::new(RecordingController::new_without_keychain());
        controller.set_state(State::RecToggle).await;
        *controller.session_id.write().await = Some("theirs".to_string());

        let outcome = controller
            .stop_capture_if_owned("mine")
            .await
            .expect("a refusal is an outcome, not an error");

        assert_eq!(outcome, CaptureStopOutcome::ForeignCapture);
        assert_eq!(controller.current_state().await, State::RecToggle);
        assert_eq!(
            controller.session_id.read().await.as_deref(),
            Some("theirs"),
            "the foreign take's identity is untouched"
        );
    }
}

/// Authored W2 falsifiers, UNRUN. The ledger/emitter/Bus/reset are real; only
/// capture input and receiver responses are synthetic. No process_recording
/// test shortcut is evidence for these terminal decisions.
#[cfg(test)]
mod refusal_recovery_tests {
    use super::*;
    use crate::presentation::transcript_bus::{
        ProjectedSealCoverageReceipt, TranscriptBusEvidenceEvent, TranscriptProjectionPhase,
    };
    use codescribe_core::audio::capture_receipt::{
        AcousticAvailability, AcousticSpeechEvidence, CaptureEvidenceIdentity,
    };
    use codescribe_core::pipeline::acoustic_ledger::{
        AcousticEvidence, AcousticLedger, EnergyCalibration, ObservationIdentity,
        ObservationProducer, OccurrenceIdentity, SealRefusal,
    };
    use codescribe_core::pipeline::contracts::EventSink;
    use codescribe_core::stt::tail_provider::TailSampleRange;

    const TAKE: &str = "refusal-capture";
    const WORDS: &str = "Te słowa zostały.";

    fn fixture_refusal(
        receipt: &codescribe_core::pipeline::acoustic_ledger::SealCoverageReceipt,
    ) -> codescribe_core::pipeline::acoustic_ledger::TerminalFinalityRefusal {
        let mut ledger = AcousticLedger::new();
        assert!(ledger.record_seal_coverage(receipt.clone()));
        ledger
            .terminal_finality(&receipt.session_id, receipt.capture_epoch)
            .into_refusal()
            .expect("fixture has no issued terminal seal")
    }

    struct Take {
        controller: RecordingController,
        bus: Arc<TranscriptBus>,
        ledger: Arc<std::sync::Mutex<AcousticLedger>>,
        emitter: PresentationEmitter,
        refusal: TerminalSealRefused,
        events: broadcast::Receiver<IpcEvent>,
        dir: tempfile::TempDir,
    }

    async fn take(state: State, words: bool) -> Take {
        take_with(state, words, true).await
    }

    /// `observed` selects which refusal the ledger issues: measured uncovered
    /// speech, or no authenticated acoustic measurement at all. Both must reach
    /// the same recovery path and keep the same committed words.
    async fn take_with(state: State, words: bool, observed: bool) -> Take {
        let controller = RecordingController::new_without_keychain();
        controller.set_state(state).await;
        *controller.session_id.write().await = Some(format!("{TAKE}:stopping"));
        *controller.pre_overlay_frontmost_app.write().await = Some("original-editor".into());
        let events = controller.subscribe_events();
        let dir = tempfile::tempdir().unwrap();
        let bus = Arc::new(
            TranscriptBus::open_at(
                TranscriptSession {
                    session_id: TAKE.into(),
                    mode: TranscriptMode::Agent,
                    has_latched_target: true,
                    latched_target_is_self: false,
                },
                dir.path().join("bus.jsonl"),
                None,
            )
            .unwrap(),
        );
        bus.publish_started();
        *controller.active_transcript_bus.write().await = Some(Arc::clone(&bus));
        let ledger = Arc::new(std::sync::Mutex::new(AcousticLedger::new()));
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::new(Mutex::new(String::new())),
            None,
            None,
            Some(Arc::clone(&bus)),
            Some(Arc::clone(&ledger)),
            None,
        );
        if words {
            let occurrence = OccurrenceIdentity::new(TAKE, 7, 0, 16_000);
            let calibration = EnergyCalibration {
                version: "refusal-synthetic-test".into(),
                min_energy_integral: 1.0,
                min_valley_samples: 1,
            };
            let evidence = AcousticEvidence {
                occurrence: occurrence.clone(),
                duration_ms: 1_000.0,
                energy_integral: 10.0,
                mean_rms_dbfs: -12.0,
                peak_dbfs: -3.0,
                vad_open_sample: Some(0),
                vad_close_sample: Some(16_000),
                evidence_calibration_version: calibration.version.clone(),
            };
            let observation =
                ObservationIdentity::new(ObservationProducer::Apple, 1, 0, occurrence);
            let receipt = {
                let mut ledger = ledger.lock().unwrap();
                assert!(ledger.qualify(&evidence, &calibration).is_qualified());
                ledger.admit(&observation, WORDS)
            };
            emitter.on_event(&EngineEvent::LedgerMutation {
                observation,
                label: WORDS.into(),
                receipt,
            });
        }
        let receipt = {
            let mut ledger = ledger.lock().unwrap();
            let identity = CaptureEvidenceIdentity::new(TAKE, 7);
            let evidence = if observed {
                AcousticSpeechEvidence::measured(
                    identity,
                    "capture_energy",
                    AcousticAvailability::Observed {
                        observed_samples: 48_000,
                    },
                    vec![TailSampleRange {
                        session: TAKE.into(),
                        capture_epoch: 7,
                        sample_start: 0,
                        sample_end: 48_000,
                    }],
                )
            } else {
                AcousticSpeechEvidence::unavailable(
                    identity,
                    "capture_energy",
                    AcousticAvailability::NotObserved,
                )
            };
            let coverage = ledger.assess_seal_coverage(TAKE, 7, &evidence, 8_000);
            assert!(!coverage.status.is_complete());
            assert!(ledger.record_seal_coverage(coverage.clone()));
            assert_eq!(
                ledger.seal_terminal(TAKE, 7),
                Err(SealRefusal::CoverageIncomplete)
            );
            coverage
        };
        emitter.on_event(&EngineEvent::SealCoverage {
            receipt: receipt.clone(),
            comparison: None,
        });
        emitter.finish().await;
        let audio = dir.path().join("refused.wav");
        std::fs::write(&audio, b"synthetic retained WAV witness").unwrap();
        Take {
            controller,
            bus,
            ledger,
            emitter,
            events,
            dir,
            refusal: TerminalSealRefused {
                finality: fixture_refusal(&receipt),
                audio_path: Some(audio),
                committed_text: if words { WORDS.into() } else { String::new() },
            },
        }
    }

    /// The ledger refused the seal, not the words: history must receive the
    /// committed document as `Raw` text instead of a `_failed` audio bag.
    #[tokio::test]
    async fn a_refused_take_with_committed_words_archives_them_as_raw_text() {
        let take = take(State::RecHold, true).await;
        assert_eq!(
            refused_take_archive(&take.refusal),
            codescribe_core::state::SessionTranscriptArchive::Committed(WORDS)
        );
    }

    /// No committed words = nothing to archive as speech; the diagnostic-only
    /// retention stays, and the reason is never persisted as transcript text.
    #[tokio::test]
    async fn a_refused_take_without_words_keeps_the_diagnostic_only_retention() {
        let take = take(State::RecHold, false).await;
        assert!(matches!(
            refused_take_archive(&take.refusal),
            codescribe_core::state::SessionTranscriptArchive::Unavailable(_)
        ));
    }

    #[tokio::test]
    async fn complete_coverage_without_seal_retains_words_without_certifying_finality() {
        let mut take = take(State::RecHold, true).await;
        let coverage = {
            let mut ledger = take.ledger.lock().unwrap();
            let speech = AcousticSpeechEvidence::measured(
                CaptureEvidenceIdentity::new(TAKE, 7),
                "synthetic_complete_test",
                AcousticAvailability::Observed {
                    observed_samples: 16_000,
                },
                vec![TailSampleRange {
                    session: TAKE.into(),
                    capture_epoch: 7,
                    sample_start: 0,
                    sample_end: 16_000,
                }],
            );
            let coverage = ledger.assess_seal_coverage(TAKE, 7, &speech, 0);
            assert!(coverage.status.is_complete());
            assert!(ledger.record_seal_coverage(coverage.clone()));
            take.refusal.finality = ledger.terminal_finality(TAKE, 7).into_refusal().unwrap();
            coverage
        };
        take.emitter.on_event(&EngineEvent::SealCoverage {
            receipt: coverage.clone(),
            comparison: None,
        });
        let result = take
            .controller
            .process_terminal_stop_error(
                anyhow::Error::new(take.refusal.clone()),
                |text| async move {
                    assert_eq!(text, WORDS);
                    Ok(TranscriptDelivery::SinkAccepted)
                },
            )
            .await;
        let outcome = result.as_ref().unwrap();
        assert_eq!(
            outcome.refusal.as_ref().unwrap().finality.coverage(),
            Some(&coverage)
        );
        take.controller
            .reset_finished_recording_state(&result)
            .await;
        let (terminals, _) = terminal_events(&mut take);
        assert_eq!(terminals.len(), 1);
        assert_eq!(
            terminals[0].phase,
            TranscriptProjectionPhase::CoverageRefused
        );
        assert_eq!(
            terminals[0].seal_coverage,
            Some(ProjectedSealCoverageReceipt::from(&coverage))
        );
        assert!(
            terminals[0]
                .acoustic_receipts
                .iter()
                .all(|receipt| receipt.seal_receipt.is_none())
        );
    }

    fn terminal_events(
        take: &mut Take,
    ) -> (Vec<TranscriptBusEvidenceEvent>, Vec<(String, String)>) {
        let mut terminals = Vec::new();
        let mut warnings = Vec::new();
        while let Ok(event) = take.events.try_recv() {
            match event.payload {
                IpcEventPayload::TranscriptProjection { json } => {
                    let event: TranscriptBusEvidenceEvent = serde_json::from_str(&json).unwrap();
                    if event.lifecycle_terminal {
                        terminals.push(event);
                    }
                }
                IpcEventPayload::Engine(EngineEventWire::Warning { code, message }) => {
                    warnings.push((code, message))
                }
                _ => {}
            }
        }
        (terminals, warnings)
    }

    #[tokio::test]
    async fn hold_and_toggle_refusal_preserve_receipts_and_pending_receiver() {
        for state in [State::RecHold, State::RecToggle] {
            let mut take = take(state, true).await;
            assert!(
                take.bus
                    .matches_refused_document(&take.refusal.finality, WORDS)
            );
            let controller = &take.controller;
            let result = take
                .controller
                .process_terminal_stop_error(
                    anyhow::Error::new(take.refusal.clone()),
                    |text| async move {
                        controller
                            .deliver_stop_transcript(
                                Some(TAKE),
                                &text,
                                true,
                                false,
                                CaptureTurnIntent::SingleTurn,
                                true,
                            )
                            .await
                    },
                )
                .await;
            assert!(result.as_ref().unwrap().refusal.is_some());
            take.controller
                .reset_finished_recording_state(&result)
                .await;
            take.controller
                .handle_processed_recording_result(true, &result)
                .await;
            take.controller
                .reset_finished_recording_state(&result)
                .await;
            let (terminals, warnings) = terminal_events(&mut take);
            assert_eq!(terminals.len(), 1);
            let terminal = &terminals[0];
            assert_eq!(terminal.session_id, TAKE);
            assert_eq!(terminal.capture_epoch, 7);
            assert_eq!(terminal.rendered_text, WORDS);
            assert_eq!(terminal.phase, TranscriptProjectionPhase::CoverageRefused);
            assert_eq!(terminal.delivery, TranscriptDelivery::ComposerPending);
            assert_eq!(
                terminal.seal_coverage,
                Some(ProjectedSealCoverageReceipt::from(
                    take.refusal.finality.coverage().unwrap()
                ))
            );
            assert!(
                terminal
                    .acoustic_receipts
                    .iter()
                    .all(|receipt| receipt.seal_receipt.is_none())
            );
            assert_eq!(warnings.len(), 1);
            assert_eq!(warnings[0].0, "terminal_coverage_refused");
            assert!(warnings[0].1.contains("no current terminal seal"));
            assert!(!warnings[0].1.contains("coverage is incomplete"));
            assert_eq!(
                take.controller.paste_target_app_name().await.as_deref(),
                Some("original-editor")
            );
            assert_eq!(
                std::fs::read(take.refusal.audio_path.as_ref().unwrap()).unwrap(),
                b"synthetic retained WAV witness"
            );
            let rows = std::fs::read_to_string(take.dir.path().join("bus.jsonl")).unwrap();
            assert_eq!(
                rows.lines()
                    .filter(|line| line.contains("\"status\":\"session_ended\""))
                    .count(),
                1
            );
            assert!(rows.contains("\"end_reason\":\"coverage_refused\""));
        }
    }

    /// Missing acoustic measurement reaches the same authenticated recovery
    /// path as measured uncovered speech: the committed words are delivered, the
    /// take WAV is retained, and the lifecycle is released.
    ///
    /// This is the guard the earlier shape would have failed. It compared the
    /// receipt against `Incomplete` specifically, so an unavailable refusal —
    /// which the ledger, recorder and emitter all now refuse on — would have
    /// been rejected here as "not the active capture" and the words lost.
    #[tokio::test]
    async fn unavailable_measurement_recovers_the_committed_words() {
        for state in [State::RecHold, State::RecToggle] {
            let mut take = take_with(state, true, false).await;
            assert_eq!(
                take.refusal.finality.coverage().unwrap().status,
                codescribe_core::pipeline::acoustic_ledger::SealCoverageStatus::Unavailable(
                    codescribe_core::pipeline::acoustic_ledger::AcousticEvidenceGap::NotObserved
                )
            );
            assert_eq!(
                take.refusal.finality.coverage().unwrap().coverage_ratio(),
                None
            );
            assert!(
                take.bus
                    .matches_refused_document(&take.refusal.finality, WORDS)
            );
            let controller = &take.controller;
            let result = take
                .controller
                .process_terminal_stop_error(
                    anyhow::Error::new(take.refusal.clone()),
                    |text| async move {
                        controller
                            .deliver_stop_transcript(
                                Some(TAKE),
                                &text,
                                true,
                                false,
                                CaptureTurnIntent::SingleTurn,
                                true,
                            )
                            .await
                    },
                )
                .await;
            let outcome = result
                .as_ref()
                .expect("unavailable measurement is a recoverable refusal");
            assert!(outcome.refusal.is_some());
            assert!(outcome.transcript_present);
            take.controller
                .reset_finished_recording_state(&result)
                .await;
            take.controller
                .handle_processed_recording_result(true, &result)
                .await;
            take.controller
                .reset_finished_recording_state(&result)
                .await;

            let (terminals, warnings) = terminal_events(&mut take);
            assert_eq!(terminals.len(), 1);
            let terminal = &terminals[0];
            assert_eq!(terminal.rendered_text, WORDS, "the words must survive");
            assert_eq!(terminal.phase, TranscriptProjectionPhase::CoverageRefused);
            assert_eq!(terminal.delivery, TranscriptDelivery::ComposerPending);
            let projected = terminal
                .seal_coverage
                .as_ref()
                .expect("the refusal projects its coverage evidence");
            assert_eq!(projected.status, "unavailable");
            assert_eq!(
                projected.unavailable_reason.as_deref(),
                Some("not_observed")
            );
            assert_eq!(
                projected.coverage_ratio, None,
                "the Bus must not render an unknown extent as a covered fraction"
            );
            assert_eq!(
                projected,
                &ProjectedSealCoverageReceipt::from(take.refusal.finality.coverage().unwrap())
            );
            assert!(
                terminal
                    .acoustic_receipts
                    .iter()
                    .all(|receipt| receipt.seal_receipt.is_none()),
                "no occurrence may acquire a terminal seal on this path"
            );
            assert_eq!(warnings.len(), 1);
            assert_eq!(warnings[0].0, "terminal_coverage_refused");
            assert!(warnings[0].1.contains("no current terminal seal"));
            assert!(!warnings[0].1.contains("coverage is incomplete"));
            assert_eq!(
                std::fs::read(take.refusal.audio_path.as_ref().unwrap()).unwrap(),
                b"synthetic retained WAV witness"
            );
            let rows = std::fs::read_to_string(take.dir.path().join("bus.jsonl")).unwrap();
            assert!(rows.contains("\"end_reason\":\"coverage_refused\""));
            assert!(
                !rows.contains("\"coverage_ratio\""),
                "an absent ratio must be absent on the wire too: {rows}"
            );
        }
    }

    #[tokio::test]
    async fn empty_and_string_refusals_never_call_delivery_and_remain_visible() {
        for typed in [false, true] {
            let mut take = take(State::RecHold, false).await;
            let error = if typed {
                anyhow::Error::new(take.refusal.clone())
            } else {
                anyhow::anyhow!("terminal seal refused")
            };
            let result = take
                .controller
                .process_terminal_stop_error(error, |_| async {
                    panic!("no authenticated words may reach a receiver")
                })
                .await;
            assert!(result.is_err());
            take.controller
                .reset_finished_recording_state(&result)
                .await;
            take.controller
                .handle_processed_recording_result(false, &result)
                .await;
            let (terminals, warnings) = terminal_events(&mut take);
            assert_eq!(terminals.len(), 1);
            assert_eq!(terminals[0].phase, TranscriptProjectionPhase::Error);
            assert_eq!(terminals[0].delivery, TranscriptDelivery::Unattempted);
            assert_eq!(warnings[0].0, "transcription_failed");
            if typed {
                assert!(warnings[0].1.contains("without a current terminal seal"));
                assert!(!warnings[0].1.contains("incomplete speech coverage"));
            }
            let rows = std::fs::read_to_string(take.dir.path().join("bus.jsonl")).unwrap();
            assert!(rows.contains(if typed {
                "coverage_refused_empty"
            } else {
                "transcription_failed"
            }));
        }
    }

    #[tokio::test]
    async fn refused_sink_failure_is_retained_and_never_accepted() {
        let mut take = take(State::RecToggle, true).await;
        let controller = &take.controller;
        let result = take
            .controller
            .process_terminal_stop_error(
                anyhow::Error::new(take.refusal.clone()),
                |text| async move {
                    assert_eq!(text, WORDS);
                    assert_eq!(
                        controller.paste_target_app_name().await.as_deref(),
                        Some("original-editor")
                    );
                    controller
                        .finish_stop_delivery(Err(anyhow::anyhow!("receiver unavailable")), true)
                        .await
                },
            )
            .await;
        assert!(result.as_ref().unwrap_err().is::<StopDeliveryFailure>());
        take.controller
            .reset_finished_recording_state(&result)
            .await;
        take.controller
            .handle_processed_recording_result(false, &result)
            .await;
        let (terminals, warnings) = terminal_events(&mut take);
        assert_eq!(terminals[0].phase, TranscriptProjectionPhase::Error);
        assert_eq!(terminals[0].rendered_text, WORDS);
        assert_eq!(terminals[0].delivery, TranscriptDelivery::Retained);
        assert!(warnings[0].1.contains("receiver unavailable"));
        assert!(warnings[0].1.contains("no current terminal seal"));
        assert!(!warnings[0].1.contains("coverage is incomplete"));
        assert_eq!(warnings[0].0, "transcription_failed");
    }

    #[tokio::test]
    async fn forged_coverage_text_and_successor_identity_cannot_authorize_handoff() {
        let mut take = take(State::RecToggle, true).await;
        let mut forged = take.refusal.finality.coverage().unwrap().clone();
        forged.max_uncovered_samples += 1;
        take.emitter.on_event(&EngineEvent::SealCoverage {
            receipt: forged.clone(),
            comparison: None,
        });
        assert!(
            take.bus
                .matches_refused_document(&take.refusal.finality, WORDS)
        );
        assert!(
            !take
                .bus
                .matches_refused_document(&fixture_refusal(&forged), WORDS)
        );
        for mutation in 0..3 {
            let mut refusal = take.refusal.clone();
            match mutation {
                0 => refusal.committed_text = "preview is not committed".into(),
                1 => refusal.finality = fixture_refusal(&forged),
                _ => *take.controller.session_id.write().await = Some("successor-capture".into()),
            }
            let result = take
                .controller
                .process_terminal_stop_error(anyhow::Error::new(refusal), |_| async {
                    panic!("foreign or unauthenticated text reached handoff")
                })
                .await;
            assert!(result.is_err());
        }
        assert_eq!(
            take.controller.session_id.read().await.as_deref(),
            Some("successor-capture")
        );
        assert!(terminal_events(&mut take).0.is_empty());
    }

    #[tokio::test]
    async fn hold_refusal_routes_once_to_original_sink_with_exact_disposition() {
        for delivery in [
            OverlayPasteDelivery::Pasted,
            OverlayPasteDelivery::Noop,
            OverlayPasteDelivery::AccessibilityPermissionNeeded,
        ] {
            let mut take = take(State::RecHold, true).await;
            let controller = &take.controller;
            let mut config = controller.get_config().await;
            config.auto_paste_enabled = true;
            config.quick_notes_enabled = false;
            let calls = AtomicUsize::new(0);
            let call_count = &calls;
            let result = controller
                .process_terminal_stop_error(
                    anyhow::Error::new(take.refusal.clone()),
                    |text| async move {
                        controller
                            .deliver_stop_transcript_with_sink(
                                Some(TAKE),
                                &text,
                                (false, false, CaptureTurnIntent::HandsFree, true),
                                &config,
                                |route, text, target| async move {
                                    call_count.fetch_add(1, Ordering::SeqCst);
                                    assert_eq!(route, DeliveryRoute::ClipboardPaste);
                                    assert_eq!(text, WORDS);
                                    assert_eq!(target.as_deref(), Some("original-editor"));
                                    Ok(OverlayPasteResult {
                                        delivery,
                                        target_app_name: target,
                                        frontmost_app_name: Some("original-editor".into()),
                                        deferred_insert_shortcut: None,
                                        deferred_insert_failure: None,
                                    })
                                },
                            )
                            .await
                    },
                )
                .await;
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert_eq!(result.is_ok(), delivery == OverlayPasteDelivery::Pasted);
            controller.reset_finished_recording_state(&result).await;
            controller
                .handle_processed_recording_result(false, &result)
                .await;
            let (terminals, _) = terminal_events(&mut take);
            assert_eq!(terminals.len(), 1);
            assert_eq!(terminals[0].rendered_text, WORDS);
            assert_eq!(
                terminals[0].delivery,
                if delivery == OverlayPasteDelivery::Pasted {
                    TranscriptDelivery::SinkAccepted
                } else {
                    TranscriptDelivery::Retained
                }
            );
        }
    }

    #[tokio::test]
    async fn refused_wav_uses_original_capture_name_without_claiming_archive_success() {
        let take = take(State::RecToggle, true).await;
        let root = take.dir.path().join("retention");
        let source = take.refusal.audio_path.as_ref().unwrap();
        let result = retain_session_audio_at(
            Some("refusal-capture:stopping"),
            source,
            codescribe_core::state::SessionTranscriptArchive::Unavailable("incomplete coverage"),
            &root,
            |_, _| None,
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("daily audio archive failed")
        );
        assert_eq!(
            std::fs::read(source).unwrap(),
            b"synthetic retained WAV witness"
        );
        assert_eq!(
            std::fs::read(root.join("sessions/refusal-capture.wav")).unwrap(),
            b"synthetic retained WAV witness"
        );
        assert!(!root.join("sessions/refusal-capture:stopping.wav").exists());
        assert!(
            take.bus
                .matches_refused_document(&take.refusal.finality, WORDS)
        );
    }

    #[tokio::test]
    async fn delivery_claim_rejects_repeat_and_admits_a_successor() {
        let controller = RecordingController::new_without_keychain();
        for (id, allowed) in [
            (TAKE, true),
            ("refusal-capture:stopping", false),
            ("successor-capture", true),
        ] {
            let result = controller
                .deliver_stop_transcript_with_sink(
                    Some(id),
                    WORDS,
                    (true, false, CaptureTurnIntent::SingleTurn, true),
                    &controller.get_config().await,
                    |_, _, _| async { panic!("composer must not paste") },
                )
                .await;
            assert_eq!(result.is_ok(), allowed);
            assert_eq!(
                *controller.delivery_disposition.read().await,
                TranscriptDelivery::ComposerPending
            );
        }
    }
}

/// W2 contracts: UNRUN until the integrator closes the joined structure.
/// Real controller/empty recorder/temporary Bus, no provider or microphone.
#[cfg(test)]
mod owned_capture_settlement_tests {
    use super::*;
    use crate::presentation::transcript_bus::CleanTranscriptEvent;

    async fn fixture() -> (
        Arc<RecordingController>,
        mpsc::UnboundedReceiver<CaptureSettlementStage>,
        tempfile::TempDir,
    ) {
        let controller = Arc::new(RecordingController::new_without_keychain());
        controller.set_state(State::RecToggle).await;
        *controller.assistive_mode.write().await = true;
        *controller.session_id.write().await = Some("owned".to_string());
        controller
            .recorder
            .lock()
            .await
            .as_mut()
            .expect("empty recorder fixture")
            .set_capture_turn_intent(CaptureTurnIntent::SingleTurn);
        let dir = tempfile::tempdir().unwrap();
        let bus = TranscriptBus::open_at(
            TranscriptSession {
                session_id: "owned".to_string(),
                mode: TranscriptMode::Agent,
                has_latched_target: false,
                latched_target_is_self: false,
            },
            dir.path().join("events.jsonl"),
            None,
        )
        .unwrap();
        bus.publish_started();
        *controller.active_transcript_bus.write().await = Some(Arc::new(bus));
        let (sender, stages) = mpsc::unbounded_channel();
        *controller.settlement_observer.lock().unwrap() = Some(sender);
        (controller, stages, dir)
    }

    async fn stage(
        stages: &mut mpsc::UnboundedReceiver<CaptureSettlementStage>,
        expected: CaptureSettlementStage,
    ) {
        loop {
            let observed = tokio::time::timeout(STOP_TIMEOUT * 2, stages.recv())
                .await
                .expect("owner must acknowledge arrival")
                .expect("observer remains installed");
            if observed == expected {
                return;
            }
        }
    }

    fn rows(dir: &tempfile::TempDir) -> Vec<CleanTranscriptEvent> {
        std::fs::read_to_string(dir.path().join("events.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn every_stop_route_joins_one_operation_past_caller_budget() {
        for route in 0..5 {
            let (controller, mut stages, dir) = fixture().await;
            if route == 1 || route == 2 {
                controller.set_state(State::RecHold).await;
            }
            let held = controller.hold_mode.write().await;
            let caller = tokio::spawn({
                let controller = Arc::clone(&controller);
                async move {
                    match route {
                        0 => controller.stop_recording_from_external_surface().await,
                        1 => {
                            controller
                                .handle_hotkey_event(HotkeyInput {
                                    key_type: HotkeyType::Hold,
                                    action: HotkeyAction::Up,
                                    assistive: true,
                                    hold_mode: HoldMode::Chat,
                                    force_raw: false,
                                    force_ai: false,
                                })
                                .await
                        }
                        2 | 3 => {
                            controller
                                .handle_hotkey_event(HotkeyInput {
                                    key_type: HotkeyType::Toggle,
                                    action: HotkeyAction::Press,
                                    assistive: false,
                                    hold_mode: HoldMode::Raw,
                                    force_raw: true,
                                    force_ai: false,
                                })
                                .await
                        }
                        _ => controller.finish_recording().await,
                    }
                }
            });
            stage(&mut stages, CaptureSettlementStage::Admitted).await;
            let task_id = controller
                .capture_settlement
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .task
                .id();
            assert!(
                caller
                    .await
                    .unwrap()
                    .unwrap_err()
                    .to_string()
                    .contains("Pending")
            );
            assert_eq!(controller.current_state().await, State::Busy);
            assert!(controller.serial_lock.try_lock().is_err());
            assert_eq!(rows(&dir).len(), 1);
            assert_eq!(
                controller.stop_capture_if_owned("owned").await.unwrap(),
                CaptureStopOutcome::Pending
            );
            assert_eq!(
                controller
                    .capture_settlement
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .task
                    .id(),
                task_id
            );
            drop(held);
            assert_eq!(
                controller.stop_capture_if_owned("owned").await.unwrap(),
                CaptureStopOutcome::Stopped
            );
            assert_eq!(rows(&dir).len(), 2, "route {route}: exactly one terminal");
            assert_eq!(controller.current_state().await, State::Idle);
        }
    }

    #[tokio::test]
    async fn stop_current_refuses_queued_admission_and_cannot_target_successor() {
        let (controller, _, dir) = fixture().await;
        let held = controller.serial_lock.lock().await;
        assert_eq!(
            controller.stop_current_capture().await.unwrap(),
            CaptureStopOutcome::AdmissionUnavailable
        );
        assert!(controller.capture_settlement.lock().unwrap().is_none());
        *controller.session_id.write().await = Some("replacement".into());
        drop(held);
        assert_eq!(
            controller.stop_capture_if_owned("owned").await.unwrap(),
            CaptureStopOutcome::ForeignCapture
        );
        assert_eq!(
            controller.session_id.read().await.as_deref(),
            Some("replacement")
        );
        assert_eq!(rows(&dir).len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_external_caller_keeps_terminal_owner() {
        let (controller, mut stages, dir) = fixture().await;
        let held = controller.hold_mode.write().await;
        let caller = tokio::spawn({
            let controller = Arc::clone(&controller);
            async move { controller.stop_recording_from_external_surface().await }
        });
        stage(&mut stages, CaptureSettlementStage::Resetting).await;
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        assert_eq!(
            controller.stop_current_capture().await.unwrap(),
            CaptureStopOutcome::Pending
        );
        assert_eq!(rows(&dir).len(), 1);
        drop(held);
        assert_eq!(
            controller.stop_capture_if_owned("owned").await.unwrap(),
            CaptureStopOutcome::Stopped
        );
        assert_eq!(rows(&dir).len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_pending_preserves_owner_and_closes_successor_admission() {
        let (controller, mut stages, dir) = fixture().await;
        controller.request_capture_shutdown();
        let held = controller.hold_mode.write().await;
        let caller = tokio::spawn({
            let controller = Arc::clone(&controller);
            async move { controller.stop_current_capture().await }
        });
        stage(&mut stages, CaptureSettlementStage::Resetting).await;
        assert_eq!(caller.await.unwrap().unwrap(), CaptureStopOutcome::Pending);
        assert!(!controller.capture_shutdown_settled());
        assert_eq!(rows(&dir).len(), 1);
        drop(held);
        assert_eq!(
            controller.stop_capture_if_owned("owned").await.unwrap(),
            CaptureStopOutcome::Stopped
        );
        // Observe the actual task exit, not merely its watch publication.
        let operation = controller
            .capture_settlement
            .lock()
            .unwrap()
            .take()
            .unwrap();
        operation.task.await.unwrap();
        assert!(controller.capture_shutdown_settled());
        assert!(controller.start_composer_turn_recording().await.is_err());
        assert!(controller.schedule_hold_start(false).await.is_err());
        assert!(controller.start_conversation_mode().await.is_err());
        assert_eq!(rows(&dir).len(), 2);
    }

    #[tokio::test]
    async fn external_failure_and_named_retry_share_one_failed_terminal() {
        let (controller, _, dir) = fixture().await;
        *controller.recorder.lock().await = None;
        let failure = controller
            .stop_recording_from_external_surface()
            .await
            .unwrap_err()
            .to_string();
        assert!(failure.contains("recorder unavailable"));
        assert_eq!(
            controller
                .stop_capture_if_owned("owned")
                .await
                .unwrap_err()
                .to_string(),
            failure
        );
        let events = rows(&dir);
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[1].end_reason,
            Some(TranscriptSessionEndReason::TranscriptionFailed)
        );
        assert_eq!(controller.current_state().await, State::Idle);
    }

    #[tokio::test]
    async fn idle_stop_invalidates_only_unadmitted_hold_generation() {
        let controller = Arc::new(RecordingController::new_without_keychain());
        let generation = controller.hold_start_generation.load(Ordering::SeqCst);
        assert_eq!(
            controller.stop_current_capture().await.unwrap(),
            CaptureStopOutcome::NoLiveCapture
        );
        assert_ne!(
            controller.hold_start_generation.load(Ordering::SeqCst),
            generation
        );
        let generation = controller.hold_start_generation.load(Ordering::SeqCst);
        assert_eq!(
            controller.stop_capture_if_owned("foreign").await.unwrap(),
            CaptureStopOutcome::NoLiveCapture
        );
        assert_eq!(
            controller.hold_start_generation.load(Ordering::SeqCst),
            generation
        );
    }

    #[tokio::test]
    async fn busy_without_a_stop_receipt_is_not_shutdown_success() {
        let controller = Arc::new(RecordingController::new_without_keychain());
        controller.set_state(State::Busy).await;
        controller.request_capture_shutdown();
        assert_eq!(
            controller.stop_current_capture().await.unwrap(),
            CaptureStopOutcome::AdmissionUnavailable
        );
        assert!(!controller.capture_shutdown_settled());
        assert_eq!(controller.current_state().await, State::Busy);
    }

    #[tokio::test(start_paused = true)]
    async fn held_hold_mode_cannot_block_public_stop_and_settles_once_after_release() {
        let (controller, mut stages, dir) = fixture().await;
        let held = controller.hold_mode.write().await;
        let caller = tokio::spawn({
            let controller = Arc::clone(&controller);
            async move { controller.stop_capture_if_owned("owned").await }
        });
        stage(&mut stages, CaptureSettlementStage::Resetting).await;
        // The obstacle is STILL HELD during the independent outer deadline.
        let outcome = tokio::time::timeout(STOP_TIMEOUT * 2, caller)
            .await
            .expect("public Stop must return while hold_mode stays locked")
            .unwrap()
            .unwrap();
        assert_eq!(outcome, CaptureStopOutcome::Pending);
        assert_eq!(controller.current_state().await, State::Busy);
        assert!(controller.serial_lock.try_lock().is_err());
        assert_eq!(rows(&dir).len(), 1, "pending is not a terminal receipt");
        drop(held);
        assert_eq!(
            controller.stop_capture_if_owned("owned").await.unwrap(),
            CaptureStopOutcome::Stopped
        );
        assert_eq!(
            controller.stop_capture_if_owned("owned").await.unwrap(),
            CaptureStopOutcome::Stopped
        );
        assert_eq!(controller.current_state().await, State::Idle);
        let events = rows(&dir);
        assert_eq!(events.len(), 2, "one start and exactly one terminal");
        assert_eq!(
            events[1].end_reason,
            Some(TranscriptSessionEndReason::Completed)
        );
        assert!(controller.active_transcript_bus.read().await.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_caller_and_duplicate_keep_one_owner_and_cannot_reset_successor() {
        let (controller, mut stages, dir) = fixture().await;
        let held = controller.hold_mode.write().await;
        let caller = tokio::spawn({
            let controller = Arc::clone(&controller);
            async move { controller.stop_capture_if_owned("owned").await }
        });
        stage(&mut stages, CaptureSettlementStage::Resetting).await;
        let task_id = controller
            .capture_settlement
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .task
            .id();
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        assert_eq!(
            controller.stop_capture_if_owned("owned").await.unwrap(),
            CaptureStopOutcome::Pending
        );
        assert_eq!(
            controller
                .capture_settlement
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .task
                .id(),
            task_id
        );
        // A successor obeys the real start serialization boundary. It cannot
        // install new state until every predecessor terminal side effect ends.
        let (entered, mut successor_entered) = tokio::sync::oneshot::channel();
        let successor = tokio::spawn({
            let controller = Arc::clone(&controller);
            async move {
                let _serial = controller.serial_lock.lock().await;
                *controller.session_id.write().await = Some("successor".to_string());
                controller.set_state(State::RecToggle).await;
                entered.send(()).unwrap();
            }
        });
        assert!(successor_entered.try_recv().is_err());
        drop(held);
        successor_entered.await.unwrap();
        successor.await.unwrap();
        assert_eq!(
            controller.stop_capture_if_owned("owned").await.unwrap(),
            CaptureStopOutcome::Stopped
        );
        assert_eq!(
            controller.session_id.read().await.as_deref(),
            Some("successor")
        );
        assert_eq!(controller.current_state().await, State::RecToggle);
        assert_eq!(rows(&dir).len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn pre_admission_wait_is_pending_and_rechecks_foreign_identity_after_release() {
        let (controller, mut stages, dir) = fixture().await;
        let held = controller.serial_lock.lock().await;
        let caller = tokio::spawn({
            let controller = Arc::clone(&controller);
            async move { controller.stop_capture_if_owned("owned").await }
        });
        stage(&mut stages, CaptureSettlementStage::Registered).await;
        let pending = tokio::time::timeout(STOP_TIMEOUT * 2, caller)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(pending, CaptureStopOutcome::Pending);
        assert_eq!(controller.current_state().await, State::RecToggle);
        *controller.session_id.write().await = Some("foreign".to_string());
        drop(held);
        assert_eq!(
            controller.stop_capture_if_owned("owned").await.unwrap(),
            CaptureStopOutcome::ForeignCapture
        );
        assert_eq!(
            controller.session_id.read().await.as_deref(),
            Some("foreign")
        );
        assert_eq!(rows(&dir).len(), 1, "foreign take and Bus untouched");
    }

    #[tokio::test(start_paused = true)]
    async fn routing_lock_is_inside_public_budget_too() {
        let (controller, mut stages, dir) = fixture().await;
        let held = controller.state.write().await;
        let caller = tokio::spawn({
            let controller = Arc::clone(&controller);
            async move { controller.stop_capture_if_owned("owned").await }
        });
        stage(&mut stages, CaptureSettlementStage::Registered).await;
        assert_eq!(
            tokio::time::timeout(STOP_TIMEOUT * 2, caller)
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            CaptureStopOutcome::Pending
        );
        assert_eq!(rows(&dir).len(), 1);
        drop(held);
        assert_eq!(
            controller.stop_capture_if_owned("owned").await.unwrap(),
            CaptureStopOutcome::Stopped
        );
    }

    #[tokio::test]
    async fn unavailable_registration_admits_no_task_or_mutation() {
        let (controller, _, dir) = fixture().await;
        let outcome = {
            let held = controller.capture_settlement.lock().unwrap();
            let mut call = Box::pin(controller.stop_capture_if_owned("owned"));
            let mut context = std::task::Context::from_waker(std::task::Waker::noop());
            let std::task::Poll::Ready(result) =
                std::future::Future::poll(call.as_mut(), &mut context)
            else {
                panic!("registration contention must not suspend");
            };
            assert!(held.is_none());
            result.unwrap()
        };
        assert_eq!(outcome, CaptureStopOutcome::AdmissionUnavailable);
        assert_eq!(controller.current_state().await, State::RecToggle);
        assert_eq!(rows(&dir).len(), 1);
    }

    #[tokio::test]
    async fn absent_and_already_stopping_are_not_success() {
        for (identity, state, expected) in [
            (None, State::Idle, CaptureStopOutcome::NoLiveCapture),
            (
                Some("owned:stopping"),
                State::Busy,
                CaptureStopOutcome::AlreadyStopping,
            ),
        ] {
            let controller = Arc::new(RecordingController::new_without_keychain());
            *controller.session_id.write().await = identity.map(str::to_string);
            controller.set_state(state).await;
            assert_eq!(
                controller.stop_capture_if_owned("owned").await.unwrap(),
                expected
            );
            assert_eq!(controller.current_state().await, state);
        }
    }

    #[tokio::test]
    async fn explicit_recorder_failure_is_retained_as_failure_with_one_terminal() {
        let (controller, _, dir) = fixture().await;
        *controller.recorder.lock().await = None;
        let failure = controller
            .stop_capture_if_owned("owned")
            .await
            .unwrap_err()
            .to_string();
        assert!(failure.contains("recorder unavailable"));
        assert_eq!(
            controller
                .stop_capture_if_owned("owned")
                .await
                .unwrap_err()
                .to_string(),
            failure
        );
        assert_eq!(controller.current_state().await, State::Idle);
        assert_eq!(rows(&dir).len(), 2);
        assert_eq!(
            rows(&dir)[1].end_reason,
            Some(TranscriptSessionEndReason::TranscriptionFailed)
        );
        *controller.session_id.write().await = Some("successor".into());
        controller.set_state(State::RecToggle).await;
        assert_eq!(
            controller
                .stop_capture_if_owned("owned")
                .await
                .unwrap_err()
                .to_string(),
            failure
        );
        assert_eq!(
            controller.session_id.read().await.as_deref(),
            Some("successor")
        );
        assert_eq!(controller.current_state().await, State::RecToggle);
        assert_eq!(
            rows(&dir).len(),
            2,
            "duplicate failure cannot end the successor"
        );
    }
}

#[cfg(test)]
mod serving_status_producer_falsifiers {
    use super::*;

    /// Settings "Active STT" reads `serving_status::current_last_serving()`.
    /// The producer left with the old adjudicating stop path (`ac6d399b3`);
    /// since then the row was always "Not yet served". A finished toggle stop
    /// must publish the engine the live session actually ran on — the
    /// recorder's label, never the configured `stt_engine`.
    ///
    /// `StreamingRecorder::new` opens no device, and `stop()` on a session
    /// that never started returns an empty transcript, so this drives the real
    /// `stop_toggle_and_adjudicate_inner` success branch without CoreAudio.
    #[tokio::test]
    async fn toggle_stop_publishes_the_live_session_engine() {
        let _serialized = serving_status::test_store_lock().lock().await;
        serving_status::clear_last_serving();
        let controller = RecordingController::new_without_keychain();
        assert!(
            controller.recorder.lock().await.is_some(),
            "test controller must own a StreamingRecorder (no device needed)"
        );
        controller.set_state(State::RecToggle).await;

        let _serial = controller.serial_lock.lock().await;
        controller
            .stop_toggle_and_adjudicate_inner(None)
            .await
            .expect("idle recorder stops cleanly");

        let verdict = serving_status::current_last_serving()
            .expect("stop path publishes the serving verdict");
        assert_eq!(verdict.engine, "local_apple");
        assert_eq!(verdict.routing_mode, serving_status::LIVE_ROUTING_MODE);
        assert_eq!(verdict.disposition, None);
        assert!(!verdict.fallback_used);
        assert_eq!(controller.current_state().await, State::Idle);
        serving_status::clear_last_serving();
    }
}

#[cfg(test)]
mod admission_presentation_status_falsifiers {
    use super::*;

    /// W3-T11 baseline witness: a deterministic refusal must leave the engine
    /// warning side-channel and travel as a typed presentation projection.
    #[test]
    fn acoustic_admission_refusal_uses_presentation_projection_event() {
        let (events, mut receiver) = broadcast::channel(2);
        let blocker = admission::AdmissionBlocker::CalibrationUnusable {
            device_name: "Built-in Microphone".to_string(),
            reason: "capture generation changed: measured 88200Hz/1ch, current 48000Hz/1ch"
                .to_string(),
        };

        RecordingController::broadcast_admission_refusal(
            &events,
            Some("session-1".to_string()),
            &blocker,
        );

        let event = receiver.try_recv().expect("one refusal projection");
        let payload = serde_json::to_value(event.payload).expect("serialize IPC payload");
        assert_eq!(payload["event"], "presentation_status");
        let projection: serde_json::Value = serde_json::from_str(
            payload["json"]
                .as_str()
                .expect("presentation status owns typed JSON"),
        )
        .expect("valid presentation status projection");
        assert_eq!(projection["kind"], "admission_refused");
        assert_eq!(projection["code"], "admission_calibration_unusable");
        assert_eq!(projection["session_id"], "session-1");
        assert!(
            projection["message"]
                .as_str()
                .expect("human-readable refusal")
                .contains("Settings › Audio")
        );
    }
}

#[cfg(test)]
mod c15d_settings_one_path_falsifiers {
    use super::*;
    use crate::config::Config;
    use codescribe_core::audio::capture_receipt::{
        CAPTURE_LEVEL_RECEIPT_CODE, CaptureLevelReceipt, CapturePathMeta,
    };
    use codescribe_core::config::energy_calibration::{
        EnergyCalibrationArtifact, EnergyCalibrationProfile, SOURCE_SYNTHETIC_FIXTURE,
        energy_calibration_path,
    };
    use codescribe_core::pipeline::streaming::SealLaneProbe;
    use std::ffi::OsString;

    struct DataDirGuard(Option<OsString>);

    impl DataDirGuard {
        fn install(path: &std::path::Path) -> Self {
            let previous = std::env::var_os("CODESCRIBE_DATA_DIR");
            // SAFETY: the test is serial and restores the process variable.
            unsafe { std::env::set_var("CODESCRIBE_DATA_DIR", path) };
            Self(previous)
        }
    }

    impl Drop for DataDirGuard {
        fn drop(&mut self) {
            // SAFETY: paired restoration for the serial test above.
            unsafe {
                match self.0.take() {
                    Some(previous) => std::env::set_var("CODESCRIBE_DATA_DIR", previous),
                    None => std::env::remove_var("CODESCRIBE_DATA_DIR"),
                }
            }
        }
    }

    fn calibration_receipt(device_name: &str) -> CaptureLevelReceipt {
        CaptureLevelReceipt {
            code: CAPTURE_LEVEL_RECEIPT_CODE,
            device_name: device_name.to_string(),
            sample_rate: 48_000,
            channels: 1,
            sample_count: 480_000,
            digital_zero_samples: 0,
            active_speech_samples: 240_000,
            clipping_samples: 0,
            dropout_blocks: 0,
            all_audio_median_db: -40.0,
            active_speech_median_db: -30.0,
            peak_db: -6.0,
            noise_floor_db: -80.0,
            snr_db: Some(50.0),
            threshold_db: -52.0,
            low: false,
        }
    }

    /// C15D-A structural falsifier: the controller source has one mutable
    /// settings handle, one write site and no compatibility setter.
    #[test]
    fn controller_has_exactly_one_mutable_settings_generation() {
        let source = include_str!("mod.rs");
        let retired_config_field = ["config: Arc<RwLock<", "Config>>"].concat();
        let runtime_arc_field =
            ["runtime_settings: RwLock<Arc<", "RuntimeSettingsSnapshot>>"].concat();
        let runtime_arc_write = ["*self.runtime_settings", ".write().await"].concat();
        let retired_setter = ["pub async ", "fn set_", "config"].concat();

        assert!(!source.contains(&retired_config_field));
        assert_eq!(source.matches(&runtime_arc_field).count(), 1);
        assert_eq!(source.matches(&runtime_arc_write).count(), 1);
        assert!(!source.contains(&retired_setter));
    }

    /// C15D-A projection falsifier: public config is a value projected from
    /// the current Arc, not separately stored controller state.
    #[tokio::test]
    async fn get_config_projects_current_runtime_snapshot_values() {
        let controller = RecordingController::new_without_keychain();
        let snapshot = controller.runtime_settings_arc().await;
        let projected = controller.get_config().await;

        assert_eq!(
            projected.hold_start_delay_ms,
            snapshot.values().hold_start_delay_ms
        );
        assert_eq!(projected.beep_on_start, snapshot.values().beep_on_start);
        assert_eq!(
            projected.transcription_overlay_enabled,
            snapshot.values().transcription_overlay_enabled
        );
    }

    /// C15D-A generation falsifier: idle refresh performs one Arc replacement
    /// and cannot mutate a previously selected Arc.
    #[tokio::test]
    #[serial_test::serial]
    async fn idle_refresh_replaces_one_arc_and_preserves_previous_generation() {
        let temp = tempfile::tempdir().unwrap();
        let _data_dir = DataDirGuard::install(temp.path());
        let controller = RecordingController::new_without_keychain();
        let before = controller.runtime_settings_arc().await;
        let before_digest = before.digest().as_str().to_string();
        let before_delay = before.values().hold_start_delay_ms;
        let mut settings = UserSettings::load();
        settings.auto_paste_enabled = Some(false);
        settings.save().unwrap();
        assert!(
            controller
                .refresh_runtime_settings_from_disk()
                .await
                .expect("refresh idle settings")
        );
        let after = controller.runtime_settings_arc().await;
        assert!(!Arc::ptr_eq(&before, &after));
        assert!(!after.values().auto_paste_enabled);
        assert!(
            !controller
                .runtime_settings_refresh_pending
                .load(Ordering::SeqCst)
        );
        assert_eq!(
            controller.get_config().await.hold_start_delay_ms,
            after.values().hold_start_delay_ms
        );
        assert_eq!(before.digest().as_str(), before_digest.as_str());
        assert_eq!(before.values().hold_start_delay_ms, before_delay);
    }

    /// W3-T11 race falsifier: after the profile is stored, the calibration
    /// path must replace the live controller Arc before Swift can issue its
    /// immediate readiness probe.
    #[tokio::test]
    #[serial_test::serial]
    async fn calibration_refresh_makes_the_new_profile_visible_before_return() {
        let temp = tempfile::tempdir().expect("temporary data dir");
        let _data_dir = DataDirGuard::install(temp.path());
        let controller = RecordingController::new_without_keychain();
        let before = controller.runtime_settings_arc().await;
        assert_eq!(admission::calibration_status_view(&before).code, "missing");

        let measured_at = 10_000;
        let profile = EnergyCalibrationProfile::derive(
            &calibration_receipt("Fixture Mic"),
            measured_at,
            SOURCE_SYNTHETIC_FIXTURE,
        )
        .expect("fixture calibration derives");
        EnergyCalibrationArtifact::record_profile(&energy_calibration_path(), profile, measured_at)
            .expect("profile persists");

        let _serial_guard = controller.serial_lock.lock().await;
        controller
            .refresh_runtime_settings_after_calibration_locked()
            .await
            .expect("calibration refresh seals a new generation");
        let after = controller.runtime_settings_arc().await;

        assert!(!Arc::ptr_eq(&before, &after));
        assert_eq!(admission::calibration_status_view(&after).code, "sealed");
        let grant = admission::evaluate_admission_readiness_at(
            &after,
            Ok(CapturePathMeta {
                device_name: "Fixture Mic".to_string(),
                sample_rate: 48_000,
                channels: 1,
            }),
            SealLaneProbe {
                armed: true,
                vad_available: true,
            },
            measured_at,
        )
        .expect("immediate readiness probe sees the persisted profile");
        assert_eq!(grant.calibration_version, "cal2-fixture-mic-10000@48000hz");
    }

    /// C15D-A lifecycle falsifier: an unfinished delayed hold owns its selected
    /// generation, so refresh defers and leaves the Arc untouched.
    #[tokio::test]
    async fn pending_hold_rejects_refresh_and_preserves_generation() {
        let controller = RecordingController::new_without_keychain();
        let before = controller.runtime_settings_arc().await;
        let (release, wait) = tokio::sync::oneshot::channel::<()>();
        let pending = tokio::spawn(async move {
            let _ = wait.await;
        });
        *controller.hold_start_task.lock().await = Some(pending);

        assert!(
            !controller
                .refresh_runtime_settings_from_disk()
                .await
                .unwrap()
        );
        let after = controller.runtime_settings_arc().await;
        assert!(Arc::ptr_eq(&before, &after));

        let _ = release.send(());
        controller.cancel_pending_hold_start().await;
    }

    /// C15D-A stale-handle falsifier: a completed delayed task is cleanup, not
    /// live ownership, and therefore cannot block an idle refresh forever.
    #[tokio::test]
    async fn finished_hold_handle_does_not_block_idle_refresh() {
        let controller = RecordingController::new_without_keychain();
        let finished = tokio::spawn(async {});
        while !finished.is_finished() {
            tokio::task::yield_now().await;
        }
        *controller.hold_start_task.lock().await = Some(finished);

        assert!(
            controller
                .refresh_runtime_settings_from_disk()
                .await
                .unwrap()
        );
        assert!(controller.hold_start_task.lock().await.is_none());
    }

    /// C15D-A active-take falsifier: both recording states keep their current
    /// Arc until the existing serialized stop/finalize path returns to Idle.
    #[tokio::test]
    async fn active_hold_and_toggle_reject_refresh() {
        let controller = RecordingController::new_without_keychain();
        for active_state in [State::RecHold, State::RecToggle] {
            *controller.state.write().await = active_state;
            let before = controller.runtime_settings_arc().await;
            assert!(
                !controller
                    .refresh_runtime_settings_from_disk()
                    .await
                    .unwrap()
            );
            let after = controller.runtime_settings_arc().await;
            assert!(Arc::ptr_eq(&before, &after));
        }
        *controller.state.write().await = State::Idle;
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn deferred_settings_refresh_is_consumed_at_next_idle_selection() {
        let temp = tempfile::tempdir().unwrap();
        let _data_dir = DataDirGuard::install(temp.path());
        let controller = RecordingController::new_without_keychain();
        let before = controller.runtime_settings_arc().await;
        *controller.state.write().await = State::RecToggle;
        let mut settings = UserSettings::load();
        settings.auto_paste_enabled = Some(false);
        settings.save().unwrap();
        assert!(
            !controller
                .refresh_runtime_settings_from_disk()
                .await
                .unwrap()
        );
        assert!(Arc::ptr_eq(
            &before,
            &controller.runtime_settings_arc().await
        ));
        assert!(
            controller
                .runtime_settings_refresh_pending
                .load(Ordering::SeqCst)
        );

        // Exercise the serialized selection boundary without opening a microphone.
        let _serial_guard = controller.serial_lock.lock().await;
        *controller.state.write().await = State::Idle;
        controller
            .refresh_pending_runtime_settings_locked()
            .await
            .unwrap();
        let after = controller.runtime_settings_arc().await;
        assert!(!Arc::ptr_eq(&before, &after));
        assert!(!after.values().auto_paste_enabled);
        assert!(
            !controller
                .runtime_settings_refresh_pending
                .load(Ordering::SeqCst)
        );
        controller
            .refresh_pending_runtime_settings_locked()
            .await
            .unwrap();
        assert!(Arc::ptr_eq(
            &after,
            &controller.runtime_settings_arc().await
        ));
    }

    /// C15D-A source falsifier: each start body selects exactly one Arc under
    /// `serial_lock`, then derives config, UserSettings and recorder binding
    /// from that named Arc. This is structural evidence, not a scheduler proof.
    #[test]
    fn hold_and_toggle_each_derive_take_facts_from_one_selected_arc() {
        let source = include_str!("mod.rs");
        let hold_signature = ["async fn schedule_hold_", "start"].concat();
        let toggle_signature = ["async fn start_toggle_", "recording"].concat();
        let stop_signature = ["async fn stop_toggle_", "and_adjudicate"].concat();
        let hold_start = source.find(&hold_signature).expect("hold start body");
        let toggle_start = source.find(&toggle_signature).expect("toggle start body");
        let stop_start = source[toggle_start..]
            .find(&stop_signature)
            .map(|offset| toggle_start + offset)
            .expect("toggle stop boundary");
        let hold_body = &source[hold_start..toggle_start];
        let toggle_body = &source[toggle_start..stop_start];

        let serial_lock = ["self.serial_lock", ".lock().await"].concat();
        let select_arc = ["self.runtime_settings_", "arc().await"].concat();
        let config_projection = ["runtime_settings", ".values()"].concat();
        let user_projection = ["runtime_settings", ".user_settings()"].concat();
        let recorder_binding = ["Arc::clone(&runtime_", "settings)"].concat();

        for body in [hold_body, toggle_body] {
            assert_eq!(body.matches(&select_arc).count(), 1);
            assert_eq!(body.matches(&config_projection).count(), 1);
            assert_eq!(body.matches(&user_projection).count(), 1);
            assert_eq!(body.matches(&recorder_binding).count(), 1);
            assert!(
                body.find(&serial_lock).expect("lifecycle lock")
                    < body.find(&select_arc).expect("settings Arc selection")
            );
        }
    }

    /// C15D falsifier: profile publish uses one snapshot's values and settings.
    #[test]
    fn recording_profile_uses_controller_snapshot_user_settings() {
        let snapshot =
            Config::load_runtime_snapshot_without_keychain().expect("seal runtime settings");
        let enabled =
            apply_runtime_transcription_profile(snapshot.values(), snapshot.user_settings(), false);
        assert_eq!(enabled, snapshot.values().transcription_overlay_enabled);
    }
}

#[cfg(test)]
mod hold_start_terminal_lifecycle_falsifiers {
    use super::*;
    use crate::presentation::transcript_bus::{
        CleanTranscriptEvent, TRANSCRIPT_BUS_PATH_ENV, TranscriptSessionEndReason,
    };

    fn hold_input(action: HotkeyAction) -> HotkeyInput {
        HotkeyInput {
            key_type: HotkeyType::Hold,
            action,
            assistive: true,
            hold_mode: HoldMode::Chat,
            force_raw: false,
            force_ai: false,
        }
    }

    /// W2 P0 falsifier: key-up lands after `session_started` and before
    /// `RecHold`. The recorder mutex is the deterministic gate — the task
    /// cannot reach `TranscriptBus::open` while the test holds it, and there is
    /// no generation check between that gate and the post-start check, so a
    /// key-up issued while the gate is held and then releasing it drives exactly
    /// the audited interleaving. Virtual time only; no wall-clock sleeps.
    #[tokio::test(start_paused = true)]
    async fn keyup_between_bus_start_and_rec_hold_ends_the_session_exactly_once() {
        const CHILD: &str = "CODESCRIBE_HOLD_START_FIXTURE_CHILD";
        if std::env::var_os(CHILD).is_none() {
            // Follow tests/logging_isolation.rs: change only the child's env.
            // The parent's prior Bus path survives even if the fixture panics;
            // current-thread Tokio and serial_test alone cannot isolate env.
            let temp = tempfile::tempdir().expect("temp bus dir");
            let output = std::process::Command::new(
                std::env::current_exe().expect("resolve controller test binary"),
            )
            .args([
                "--exact",
                "controller::hold_start_terminal_lifecycle_falsifiers::keyup_between_bus_start_and_rec_hold_ends_the_session_exactly_once",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env(
                TRANSCRIPT_BUS_PATH_ENV,
                temp.path().join("transcript-events.jsonl"),
            )
            .output()
            .expect("launch isolated controller fixture");
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                output.status.success(),
                "controller fixture failed: {stdout}\n{stderr}"
            );
            assert!(
                stdout.contains("1 passed; 0 failed; 0 ignored"),
                "the exact controller fixture must execute: {stdout}\n{stderr}"
            );
            return;
        }
        let bus_path = std::path::PathBuf::from(
            std::env::var_os(TRANSCRIPT_BUS_PATH_ENV).expect("isolated child Bus path"),
        );

        let controller = Arc::new(RecordingController::new_without_keychain());
        let recorder_gate = controller.recorder.lock().await;
        assert!(
            recorder_gate.is_some(),
            "falsifier needs a constructed recorder to reach the Bus start"
        );
        *controller.hold_mode.write().await = HoldMode::Chat;

        controller
            .handle_hold_event(hold_input(HotkeyAction::Down))
            .await
            .expect("hold down schedules a delayed start");
        let task = controller
            .hold_start_task
            .lock()
            .await
            .take()
            .expect("delayed hold start scheduled");
        let scheduled_generation = controller.hold_start_generation.load(Ordering::SeqCst);

        // Step virtual time until the task has passed its pre-lock checks and
        // is parked on the recorder gate (the start guard is raised just
        // before the session id is written, ahead of the recorder lock).
        while !controller.start_transition_in_flight.load(Ordering::SeqCst) {
            assert!(
                !task.is_finished(),
                "hold start finished before reaching the recorder gate"
            );
            tokio::time::advance(Duration::from_millis(1)).await;
        }
        assert_eq!(controller.current_state().await, State::Idle);
        assert!(controller.session_id.read().await.is_some());
        assert!(controller.active_transcript_bus.read().await.is_none());

        // The real key-up path: state is still Idle, so this cancels the
        // pending start by bumping the generation — without `serial_lock`.
        controller
            .handle_hold_event(hold_input(HotkeyAction::Up))
            .await
            .expect("key-up while idle cancels");
        assert_ne!(
            controller.hold_start_generation.load(Ordering::SeqCst),
            scheduled_generation
        );

        drop(recorder_gate);
        task.await.expect("hold start task joins");

        let lines: Vec<CleanTranscriptEvent> = std::fs::read_to_string(&bus_path)
            .expect("bus file written")
            .lines()
            .map(|line| serde_json::from_str(line).expect("bus line"))
            .collect();
        let statuses: Vec<&str> = lines.iter().map(|event| event.status.as_str()).collect();
        assert_eq!(statuses, ["session_started", "session_ended"]);
        assert_eq!(lines[0].session_id, lines[1].session_id);
        assert_eq!(
            lines[1].end_reason,
            Some(TranscriptSessionEndReason::StartSuperseded)
        );
        assert!(lines[1].text.is_empty());

        assert_eq!(controller.current_state().await, State::Idle);
        assert!(controller.session_id.read().await.is_none());
        assert!(controller.active_transcript_bus.read().await.is_none());
        assert!(controller.assistive_context.read().await.is_none());
        assert!(controller.pre_overlay_frontmost_app.read().await.is_none());
        assert!(!controller.start_transition_in_flight.load(Ordering::SeqCst));
        assert!(
            !controller
                .recorder
                .lock()
                .await
                .as_ref()
                .expect("recorder")
                .recorder
                .is_active()
        );
    }

    /// Structural falsifier: exactly one controller call site writes the
    /// terminal Bus line, and the hold task installs its Bus into the active
    /// slot before any recorder start so every later exit can end it.
    #[test]
    fn controller_has_one_terminal_bus_publisher() {
        let source = include_str!("mod.rs");
        let publish_ended = [".publish_", "ended"].concat();
        assert_eq!(source.matches(&publish_ended).count(), 1);

        let hold_signature = ["async fn schedule_hold_", "start"].concat();
        let toggle_signature = ["async fn start_toggle_", "recording"].concat();
        let hold_start = source.find(&hold_signature).expect("hold start body");
        let toggle_start = source.find(&toggle_signature).expect("toggle start body");
        let hold_body = &source[hold_start..toggle_start];
        let bus_install = ["hold_session.active_transcript_bus", ".write().await = "].concat();
        let recorder_start = ["rec.start_event_", "session("].concat();
        assert!(
            hold_body
                .find(&bus_install)
                .expect("bus installed in hold body")
                < hold_body
                    .find(&recorder_start)
                    .expect("recorder start in hold body")
        );
    }
}

/// W2 recovery contracts, UNRUN. Exercise the production consumer and copy
/// owner with temporary directories; the daily history encoder is injected.
#[cfg(test)]
mod capture_failure_recovery_tests {
    use super::*;
    use codescribe_core::state::SessionTranscriptArchive;

    fn failure(path: Option<std::path::PathBuf>) -> anyhow::Error {
        anyhow::Error::new(CaptureStopFailure {
            session_id: Some("capture-owner".into()),
            capture_epoch: 7,
            audio_path: path,
            cause: std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "original processing failure",
            )
            .into(),
            task_failure: None,
        })
    }

    #[test]
    fn matching_failure_retains_exact_audio_and_stays_a_typed_error() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.wav");
        let bytes = b"exact producer archive bytes";
        std::fs::write(&source, bytes).unwrap();
        let root = dir.path().join("archive");
        let error = recover_capture_stop_failure(
            failure(Some(source.clone())),
            Some("capture-owner:stopping"),
            (Some("capture-owner"), 7),
            |id, path| {
                retain_session_audio_at(
                    Some(id),
                    path,
                    SessionTranscriptArchive::Unavailable("processing failed"),
                    &root,
                    |file, transcript| {
                        use std::io::Read;
                        assert!(matches!(
                            transcript,
                            SessionTranscriptArchive::Unavailable(_)
                        ));
                        let mut archived = Vec::new();
                        file.read_to_end(&mut archived).unwrap();
                        assert_eq!(archived, bytes);
                        Some(source.clone())
                    },
                )
            },
        );
        let typed = error.downcast_ref::<CaptureStopFailure>().unwrap();
        assert_eq!(typed.audio_path.as_deref(), Some(source.as_path()));
        assert_eq!(
            typed.cause.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::BrokenPipe
        );
        assert!(error.downcast_ref::<TerminalSealRefused>().is_none());
        for path in [source, root.join("sessions/capture-owner.wav")] {
            assert_eq!(std::fs::read(path).unwrap(), bytes);
        }
        assert!(!root.join("last_session.wav").exists());
    }

    #[test]
    fn mismatched_or_missing_identity_never_reaches_archive() {
        for (id, session, epoch) in [
            (Some("other-take"), Some("capture-owner"), 7),
            (Some("capture-owner"), Some("other-take"), 7),
            (Some("capture-owner"), Some("capture-owner"), 8),
            (Some("capture-owner"), Some("capture-owner"), 0),
            (None, Some("capture-owner"), 7),
            (Some("../capture-owner"), Some("capture-owner"), 7),
        ] {
            let error = recover_capture_stop_failure(
                failure(Some("unused.wav".into())),
                id,
                (session, epoch),
                |_, _| panic!("foreign evidence must not archive or deliver"),
            );
            assert!(error.to_string().contains("identity mismatch"));
            assert!(error.downcast_ref::<CaptureStopFailure>().is_some());
        }
    }

    #[test]
    fn no_archive_receipt_never_invents_a_saved_file() {
        let error = recover_capture_stop_failure(
            failure(None),
            Some("capture-owner"),
            (Some("capture-owner"), 7),
            |_, _| panic!("no path exists to retain"),
        );
        assert!(
            error
                .downcast_ref::<CaptureStopFailure>()
                .unwrap()
                .audio_path
                .is_none()
        );
        assert!(error.to_string().contains("no finalized WAV receipt"));
        assert!(error.to_string().contains("committed text unavailable"));
    }

    #[test]
    fn failed_copy_and_daily_archive_preserve_source_and_original_failure() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.wav");
        std::fs::write(&source, b"source survives").unwrap();
        let root = dir.path().join("not-a-directory");
        std::fs::write(&root, b"block copies").unwrap();
        let error = recover_capture_stop_failure(
            failure(Some(source.clone())),
            Some("capture-owner"),
            (Some("capture-owner"), 7),
            |id, path| {
                retain_session_audio_at(
                    Some(id),
                    path,
                    SessionTranscriptArchive::Unavailable("processing failed"),
                    &root,
                    |_, _| None,
                )
            },
        );
        assert!(error.to_string().contains("daily audio archive failed"));
        assert!(error.to_string().contains("sessions/capture-owner.wav"));
        assert!(error.to_string().contains("original processing failure"));
        assert!(error.downcast_ref::<CaptureStopFailure>().is_some());
        assert_eq!(std::fs::read(source).unwrap(), b"source survives");
        assert_eq!(std::fs::read(root).unwrap(), b"block copies");
    }

    #[test]
    fn daily_archive_failure_does_not_skip_session_copy() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.wav");
        std::fs::write(&source, b"retained despite daily bag failure").unwrap();
        let root = dir.path().join("copies");
        let result = retain_session_audio_at(
            Some("capture-owner"),
            &source,
            SessionTranscriptArchive::Unavailable("failure"),
            &root,
            |_, _| None,
        );
        assert!(result.is_err());
        assert_eq!(
            std::fs::read(root.join("sessions/capture-owner.wav")).unwrap(),
            std::fs::read(source).unwrap()
        );
    }

    fn retain_fixture(source: &std::path::Path, root: &std::path::Path) -> Result<()> {
        retain_session_audio_at(
            Some("capture-owner"),
            source,
            SessionTranscriptArchive::Committed("fixture"),
            root,
            |_, _| Some(source.to_path_buf()),
        )
    }

    #[test]
    fn quiet_take_keeps_last_spoken_audio_and_each_session_links_its_take() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("data");
        std::fs::create_dir(&root).unwrap();
        let speech = dir.path().join("speech.wav");
        let quiet = dir.path().join("quiet.wav");
        std::fs::write(&speech, b"spoken take").unwrap();
        std::fs::write(&quiet, b"quiet take").unwrap();
        retain_session_audio_at(
            Some("speech-take"),
            &speech,
            SessionTranscriptArchive::Committed("hello"),
            &root,
            |_, _| Some(speech.clone()),
        )
        .unwrap();
        retain_session_audio_at(
            Some("quiet-take"),
            &quiet,
            SessionTranscriptArchive::NoSpeech,
            &root,
            |_, _| Some(quiet.clone()),
        )
        .unwrap();
        assert_eq!(
            std::fs::metadata(&speech).unwrap().ino(),
            std::fs::metadata(root.join("sessions/speech-take.wav"))
                .unwrap()
                .ino()
        );
        assert_eq!(
            std::fs::metadata(&quiet).unwrap().ino(),
            std::fs::metadata(root.join("sessions/quiet-take.wav"))
                .unwrap()
                .ino()
        );
        assert_eq!(
            std::fs::read(root.join("last_session.wav")).unwrap(),
            b"spoken take"
        );
    }

    #[test]
    fn repeated_retention_and_source_destination_hardlinks_preserve_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("archive");
        std::fs::create_dir_all(root.join("sessions")).unwrap();
        let source = dir.path().join("take.wav");
        let bytes = b"take bytes must never be truncated";
        std::fs::write(&source, bytes).unwrap();
        let session = root.join("sessions/capture-owner.wav");
        let alias = root.join("last_session.wav");
        std::fs::hard_link(&source, &session).unwrap();
        std::fs::hard_link(&source, &alias).unwrap();
        for donor in [&source, &source, &session] {
            retain_fixture(donor, &root).unwrap();
            for path in [&source, &session, &alias] {
                assert_eq!(std::fs::read(path).unwrap(), bytes);
            }
        }
        let later = dir.path().join("later.wav");
        std::fs::write(&later, b"a later legitimate take").unwrap();
        retain_fixture(&later, &root).unwrap();
        assert_eq!(std::fs::read(source).unwrap(), bytes);
        assert_eq!(std::fs::read(session).unwrap(), b"a later legitimate take");
        assert_eq!(std::fs::read(alias).unwrap(), b"a later legitimate take");
    }

    #[test]
    fn destination_links_are_replaced_without_writing_source_or_external_target() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("archive");
        std::fs::create_dir_all(root.join("sessions")).unwrap();
        let source = dir.path().join("take.wav");
        let external = dir.path().join("unrelated.wav");
        std::fs::write(&source, b"take").unwrap();
        std::fs::write(&external, b"unrelated").unwrap();
        let session = root.join("sessions/capture-owner.wav");
        let alias = root.join("last_session.wav");
        symlink(&external, &session).unwrap();
        symlink(&source, &alias).unwrap();
        retain_fixture(&source, &root).unwrap();
        assert_eq!(std::fs::read(&source).unwrap(), b"take");
        assert_eq!(std::fs::read(external).unwrap(), b"unrelated");
        assert!(std::fs::symlink_metadata(&session).unwrap().is_file());
        assert!(
            std::fs::symlink_metadata(&alias)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(session).unwrap(), b"take");
        assert_eq!(std::fs::read(alias).unwrap(), b"take");
    }

    #[test]
    fn sessions_symlink_refuses_escape_and_preserves_alias() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("archive");
        let external = dir.path().join("external");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&external).unwrap();
        let unrelated = external.join("capture-owner.wav");
        std::fs::write(&unrelated, b"external bytes").unwrap();
        symlink(&external, root.join("sessions")).unwrap();
        let source = dir.path().join("take.wav");
        std::fs::write(&source, b"take").unwrap();
        let error = retain_fixture(&source, &root).unwrap_err();
        assert!(error.to_string().contains("sessions/capture-owner.wav"));
        assert_eq!(std::fs::read(unrelated).unwrap(), b"external bytes");
        assert_eq!(std::fs::read(source).unwrap(), b"take");
        assert!(!root.join("last_session.wav").exists());
    }

    #[test]
    fn root_symlink_and_parent_traversal_are_refused() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let external = dir.path().join("external");
        std::fs::create_dir(&external).unwrap();
        let root = dir.path().join("archive");
        symlink(&external, &root).unwrap();
        let source = dir.path().join("take.wav");
        std::fs::write(&source, b"take").unwrap();
        assert!(retain_fixture(&source, &root).is_err());
        assert!(retain_fixture(&source, &external.join("../escape")).is_err());
        assert_eq!(std::fs::read_dir(external).unwrap().count(), 0);
        assert!(!dir.path().join("escape").exists());
        assert_eq!(std::fs::read(source).unwrap(), b"take");
    }

    #[test]
    fn nonregular_sources_are_refused_before_archive_or_copy() {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let regular = dir.path().join("take.wav");
        std::fs::write(&regular, b"take").unwrap();
        let link = dir.path().join("link.wav");
        symlink(&regular, &link).unwrap();
        let fifo = dir.path().join("fifo.wav");
        let fifo_name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: a NUL-terminated path in this test's private temporary directory.
        assert_eq!(unsafe { libc::mkfifo(fifo_name.as_ptr(), 0o600) }, 0);
        for source in [link.as_path(), fifo.as_path(), dir.path()] {
            assert!(
                retain_session_audio_at(
                    Some("capture-owner"),
                    source,
                    SessionTranscriptArchive::Unavailable("fixture"),
                    &dir.path().join("archive"),
                    |_, _| panic!("unsafe source reached archive"),
                )
                .is_err()
            );
        }
        assert!(!dir.path().join("archive").exists());
        assert_eq!(std::fs::read(regular).unwrap(), b"take");
    }

    #[test]
    fn failed_publication_preserves_destination_and_cleans_temporary_entry() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("archive");
        let blocked = root.join("sessions/capture-owner.wav");
        std::fs::create_dir_all(&blocked).unwrap();
        std::fs::write(blocked.join("unrelated"), b"keep directory bytes").unwrap();
        let source = dir.path().join("take.wav");
        std::fs::write(&source, b"take").unwrap();
        assert!(retain_fixture(&source, &root).is_err());
        assert_eq!(std::fs::read(source).unwrap(), b"take");
        assert_eq!(
            std::fs::read(blocked.join("unrelated")).unwrap(),
            b"keep directory bytes"
        );
        assert!(!root.join("last_session.wav").exists());
        assert_eq!(std::fs::read_dir(root.join("sessions")).unwrap().count(), 1);
        assert_eq!(std::fs::read_dir(root).unwrap().count(), 1);
    }

    #[test]
    fn source_path_replacement_after_open_cannot_change_copied_object() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("take.wav");
        let original = dir.path().join("original.wav");
        let root = dir.path().join("archive");
        std::fs::write(&source, b"opened take").unwrap();
        retain_session_audio_at(
            Some("capture-owner"),
            &source,
            SessionTranscriptArchive::Unavailable("fixture"),
            &root,
            |file, _| {
                use std::io::Read;
                std::fs::rename(&source, &original).unwrap();
                std::fs::write(&source, b"replacement must not be copied").unwrap();
                let mut archived = Vec::new();
                file.read_to_end(&mut archived).unwrap();
                assert_eq!(archived, b"opened take");
                Some(source.clone())
            },
        )
        .unwrap();
        assert_eq!(std::fs::read(original).unwrap(), b"opened take");
        assert_eq!(
            std::fs::read(source).unwrap(),
            b"replacement must not be copied"
        );
        assert_eq!(
            std::fs::read(root.join("sessions/capture-owner.wav")).unwrap(),
            b"opened take"
        );
        assert!(!root.join("last_session.wav").exists());
    }

    #[test]
    fn parent_and_leaf_replacement_after_open_stays_in_pinned_directories() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("take.wav");
        std::fs::write(&source, b"opened take").unwrap();
        let root = dir.path().join("archive");
        let moved = dir.path().join("moved-archive");
        let external = dir.path().join("external");
        std::fs::create_dir(&external).unwrap();
        let unrelated = external.join("capture-owner.wav");
        std::fs::write(&unrelated, b"external bytes").unwrap();
        retain_session_audio_at(
            Some("capture-owner"),
            &source,
            SessionTranscriptArchive::Unavailable("fixture"),
            &root,
            |_, _| {
                std::fs::rename(root.join("sessions"), root.join("pinned-sessions")).unwrap();
                symlink(&external, root.join("sessions")).unwrap();
                symlink(&unrelated, root.join("pinned-sessions/capture-owner.wav")).unwrap();
                symlink(&source, root.join("last_session.wav")).unwrap();
                std::fs::rename(&root, &moved).unwrap();
                symlink(&external, &root).unwrap();
                Some(source.to_path_buf())
            },
        )
        .unwrap();
        assert_eq!(std::fs::read(&unrelated).unwrap(), b"external bytes");
        assert_eq!(std::fs::read_dir(external).unwrap().count(), 1);
        assert_eq!(std::fs::read(source).unwrap(), b"opened take");
        assert_eq!(
            std::fs::read(moved.join("pinned-sessions/capture-owner.wav")).unwrap(),
            b"opened take"
        );
        assert_eq!(
            std::fs::read(moved.join("last_session.wav")).unwrap(),
            b"opened take"
        );
    }

    #[tokio::test]
    async fn recovery_copy_failure_reaches_visible_warning_and_failed_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let controller = RecordingController::new_without_keychain();
        let mut events = controller.subscribe_events();
        let bus_path = dir.path().join("bus.jsonl");
        let bus = TranscriptBus::open_at(
            TranscriptSession {
                session_id: "capture-owner".into(),
                mode: TranscriptMode::Dictation,
                has_latched_target: false,
                latched_target_is_self: false,
            },
            bus_path.clone(),
            None,
        )
        .unwrap();
        bus.publish_started();
        *controller.active_transcript_bus.write().await = Some(Arc::new(bus));
        let source = dir.path().join("take.wav");
        std::fs::write(&source, b"failed take survives").unwrap();
        let root = dir.path().join("blocked-root");
        std::fs::write(&root, b"unrelated root bytes").unwrap();
        let error = recover_capture_stop_failure(
            failure(Some(source.clone())),
            Some("capture-owner"),
            (Some("capture-owner"), 7),
            |_, path| retain_fixture(path, &root),
        );
        assert!(error.downcast_ref::<CaptureStopFailure>().is_some());
        assert_eq!(std::fs::read(source).unwrap(), b"failed take survives");
        assert_eq!(std::fs::read(root).unwrap(), b"unrelated root bytes");
        let result = Err(error);
        controller.reset_finished_recording_state(&result).await;
        controller
            .handle_processed_recording_result(false, &result)
            .await;
        let mut visible = false;
        while let Ok(event) = events.try_recv() {
            if let IpcEventPayload::Engine(EngineEventWire::Warning { code, message }) =
                event.payload
                && code == "transcription_failed"
            {
                assert!(message.contains("audio retention failed"));
                assert!(message.contains("original processing failure"));
                visible = true;
            }
        }
        assert!(visible);
        let rows: Vec<crate::presentation::transcript_bus::CleanTranscriptEvent> =
            std::fs::read_to_string(bus_path)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[1].end_reason,
            Some(TranscriptSessionEndReason::TranscriptionFailed)
        );
        assert_eq!(controller.current_state().await, State::Idle);
    }
}

#[cfg(test)]
mod explicit_startup_tests {
    use super::*;
    use crate::config::{CapturedRuntimeInputs, StartupAcquisitionProbe};

    fn snapshot(root: &std::path::Path) -> RuntimeSettingsSnapshot {
        Config::runtime_snapshot_from_captured(CapturedRuntimeInputs::defaults_at(
            root.to_path_buf(),
            1_700_000_000_000,
        ))
    }

    #[tokio::test]
    async fn capture_shutdown_settled_requires_conversation_task_completion() {
        let root = tempfile::tempdir().unwrap();
        let probe = StartupAcquisitionProbe::forbid();
        let controller = RecordingController::from_startup_inputs(
            snapshot(root.path()),
            ControllerStartupResources::inert(),
            root.path(),
        );
        assert!(!controller.capture_shutdown_settled());
        controller.request_capture_shutdown();
        assert!(controller.conversation_task.lock().await.is_none());
        assert!(controller.capture_shutdown_settled(), "no task is settled");

        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            started_tx.send(()).unwrap();
            finish_rx.await.unwrap();
        });
        let task_id = task.id();
        *controller.conversation_task.lock().await = Some(task);
        started_rx.await.unwrap();
        assert!(
            !controller.capture_shutdown_settled(),
            "a running task must block shutdown settlement"
        );

        finish_tx.send(()).unwrap();
        let mut conversation = controller.conversation_task.lock().await;
        let task = conversation.as_mut().unwrap();
        // Await by reference so the completed handle remains in the owner slot.
        (&mut *task).await.unwrap();
        assert!(task.is_finished());
        assert_eq!(task.id(), task_id);
        assert!(
            !controller.capture_shutdown_settled(),
            "even a finished task cannot bypass the conversation lock"
        );
        drop(conversation);
        assert!(
            controller.capture_shutdown_settled(),
            "a finished retained task is settled"
        );
        assert_eq!(
            controller
                .conversation_task
                .lock()
                .await
                .as_ref()
                .unwrap()
                .id(),
            task_id,
            "settlement must not remove the finished handle"
        );
        assert!(probe.attempts().is_empty());
    }

    #[tokio::test]
    async fn explicit_assembly_keeps_real_idle_lifecycle_and_passed_context_root() {
        let root = tempfile::tempdir().unwrap();
        let probe = StartupAcquisitionProbe::forbid();
        let controller = RecordingController::from_startup_inputs(
            snapshot(root.path()),
            ControllerStartupResources::inert(),
            root.path(),
        );
        assert_eq!(controller.current_state().await, State::Idle);
        assert!(controller.recorder.lock().await.is_none());
        assert!(!controller.shutdown_requested.load(Ordering::SeqCst));
        let bucket = controller.context_bucket.lock().await;
        let expected = ContextBucket::for_codescribe_data_dir(root.path());
        assert_eq!(format!("{bucket:?}"), format!("{expected:?}"));
        assert!(
            !root.path().join("context").exists(),
            "assembly must not create context storage"
        );
        assert!(probe.attempts().is_empty());
    }

    #[test]
    fn normal_constructor_shared_path_still_invokes_resource_acquisition() {
        let probe = StartupAcquisitionProbe::forbid();
        let result = std::panic::catch_unwind(|| {
            RecordingController::with_runtime_settings(
                snapshot(std::path::Path::new("/fixture/normal")),
                "fixture adapter witness",
            )
        });
        assert!(result.is_err());
        assert_eq!(probe.attempts(), ["controller startup resources"]);
    }

    #[test]
    fn both_normal_constructors_still_invoke_core_host_capture() {
        for constructor in [
            RecordingController::new,
            RecordingController::new_without_keychain,
        ] {
            let probe = StartupAcquisitionProbe::forbid();
            assert!(std::panic::catch_unwind(constructor).is_err());
            assert_eq!(probe.attempts(), ["settings capture"]);
        }
    }
}

#[cfg(test)]
mod max_start_order_tests {
    #[test]
    fn recording_selection_does_not_discover_tools_before_audio_opens() {
        let controller = include_str!("mod.rs");
        let selection = controller
            .split("async fn selected_max_consultation(")
            .nth(1)
            .expect("Max selection exists")
            .split("pub fn subscribe_max_approval_changes")
            .next()
            .expect("selection body exists");
        let discovery = selection
            .find("configured_registry()")
            .expect("selected consultation uses the production registry");
        assert!(
            selection[..discovery].rfind("Box::new(||").is_some(),
            "recording selection constructs the tool registry before opening audio"
        );
    }
}
