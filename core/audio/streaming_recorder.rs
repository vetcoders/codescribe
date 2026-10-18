//! Live capture wired to the streaming transcription pipeline.
//!
//! [`StreamingRecorder`] owns a [`Recorder`] and forwards every captured block
//! down a bounded channel to `transcription_session`, which emits `EngineEvent`s
//! to a caller-supplied sink. The channel is deliberately deep
//! (`AUDIO_BACKLOG_CHUNKS`): a cold Whisper load happens *behind* it, so the
//! user's first words queue up instead of being dropped while the model loads.
//!
//! Shutdown is ordered and matters. Stopping capture is not enough — the session
//! task has to drain, and the presentation layer publishes on its own task, so
//! both `stop` paths require its FIFO acknowledgement before releasing the sink.
//! The three-second bound refuses an unfinished drain; buffer length cannot
//! prove publication. Dropping the sink early truncates delivered text.

use crate::asr_session::recorder::{RecorderLifecycleHandle, recorder_lifecycle_channel};
use crate::audio::recorder::{
    CaptureArchiveError, ClosedCaptureArchive, Recorder, RecorderConfig, SpillSender, SpillSink,
    convert_mono_f32_to_i16,
};
use crate::config::{RuntimeSettingsSnapshot, UserSettings};
use crate::pipeline::acoustic_ledger::AcousticLedger;
#[cfg(test)]
use crate::pipeline::acoustic_ledger::SealCoverageReceipt;
use crate::pipeline::contracts::{EngineEvent, EventSink};
use crate::pipeline::replay_card::{ReplayBuild, ReplayCard};
use crate::pipeline::streaming::{
    SessionConfig, TailPatchSessionReceipt, collect_buffered_engine_events_with_config,
    stream_log_path, transcription_session,
};
use anyhow::{Context, Result, anyhow};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

/// Real capture retained after an explicit Stop gesture, before subscriber EOF.
pub const STOP_CAPTURE_GRACE: Duration = Duration::from_millis(1500);

/// How many turns one capture gesture owns.
///
/// This is per-take capture intent, never a stored preference and never a mode
/// flag. It is decided by the surface that opened the microphone and frozen for
/// exactly that take: the hands-free lanes keep the utterance-epoch contract
/// they always had, and one composer gesture owns one explicit turn.
///
/// It deliberately does **not** describe destination, engine, or acoustic
/// evidence. Silero segmentation, ledger qualification, and Layer 1 tail repair
/// remain active for every variant — only the UI-visible epoch lifecycle and
/// the *live* paid formatter lane read this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureTurnIntent {
    /// Hotkey toggle, hold, tray, and the assistive overlay.
    ///
    /// Trailing silence past the configured threshold closes an utterance
    /// epoch, and every sealed occurrence may reach the live formatter as it
    /// happens. This is the pre-existing behaviour of every non-composer lane.
    #[default]
    HandsFree,
    /// One composer gesture, one explicit take.
    ///
    /// Silence never closes the take — only an explicit stop does — and no
    /// paid formatting is launched per silence-delimited fragment. The turn is
    /// formatted once at terminal processing instead.
    SingleTurn,
    /// A channel delivers shaped Raw after acoustic refinement, without LLM formatting.
    AgentChannel,
}

impl CaptureTurnIntent {
    /// The per-take utterance silence threshold this intent asks the recorder
    /// for, given the configured hands-free value.
    ///
    /// `None` is the pipeline's legacy contract: one continuous stream for the
    /// whole take, no epoch decisions at all. It is the *engine lifecycle* that
    /// rests, not the VAD — Silero keeps running for identity and tail repair.
    pub fn utterance_silence_sec(self, configured_sec: f32) -> Option<f32> {
        match self {
            Self::HandsFree | Self::AgentChannel => Some(configured_sec),
            Self::SingleTurn => None,
        }
    }

    /// Whether a sealed occurrence may open a paid formatter slot *while the
    /// take is still live*.
    ///
    /// A one-turn take collects the whole turn and formats it once at terminal
    /// processing, so per-fragment provider calls are refused at the arming
    /// seam rather than deduplicated after the fact.
    pub const fn schedules_live_formatting(self) -> bool {
        matches!(self, Self::HandsFree)
    }

    /// Whether the take may be formatted once when it terminates.
    ///
    /// A hands-free take has already paid per occurrence; a channel never
    /// formats with an LLM. Only a composer take formats at stop.
    pub const fn formats_once_at_terminal(self) -> bool {
        matches!(self, Self::SingleTurn)
    }
}

/// What kind of client holds the shared capture open.
///
/// Capture lifetime is refcounted over subscribers, never identical to one
/// take: the physical stream opens for the first subscriber and closes when
/// the last one releases. The dictation take is the first subscriber kind;
/// the agent channel (FN-1) is the second.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CaptureSubscriberKind {
    /// A dictation take (hold, toggle, assistive, composer). Owns the ledger,
    /// the reducer and stop-path delivery; at most one at a time, enforced by
    /// [`StreamingRecorder::start_event_session`].
    Take,
    /// A non-take consumer (agent channel): taps the same PCM through the
    /// bounded fan-out but binds no session authority, owns no ledger and
    /// never drives the dictation state machine.
    Channel,
}

/// Identity of one registered capture subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CaptureSubscriberId(u64);

/// What releasing the take's own subscription means for the physical capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TakeFeedRelease {
    /// The take was the last subscriber: the stop path owes the physical
    /// close and the whole-capture WAV, exactly the pre-refcount contract.
    LastSubscriber,
    /// Remaining subscribers keep the capture open: no physical close, and no
    /// whole-capture WAV belongs to this take.
    CaptureShared,
    /// No take subscription was registered (stale-cleanup and recovery
    /// paths); the stop path keeps the unconditional close it always had.
    NoTakeFeed,
}

/// Transport admission only: never invoke the executor while opening audio.
fn live_max_capability(
    intent: CaptureTurnIntent,
    enabled: bool,
    policy: crate::config::FormattingPolicy,
    agent: Option<&Arc<dyn crate::ai_formatting::FormattingAgent>>,
) -> Option<Arc<dyn crate::ai_formatting::FormattingAgent>> {
    if intent.schedules_live_formatting()
        && enabled
        && policy == crate::config::FormattingPolicy::Max
    {
        agent.cloned()
    } else {
        None
    }
}

/// Ledger refusal of the terminal transcript after a successful capture stop.
///
/// Raised by [`StreamingRecorder::stop`] when the acoustic ledger cannot
/// authenticate an issued terminal seal. Complete coverage is not finality.
/// The capture itself succeeded — `audio_path` is the take WAV
/// already written to disk — which is why this is a typed error rather than a
/// string: the stop path must retain that audio and close the take instead of
/// reporting a recorder failure.
#[derive(Debug, Clone)]
pub struct TerminalSealRefused {
    pub finality: crate::pipeline::acoustic_ledger::TerminalFinalityRefusal,
    pub audio_path: Option<std::path::PathBuf>,
    /// The committed live document at refusal time. The ledger refused the
    /// seal, not the words: the controller may still hand this text to the
    /// user as a degraded stop-path delivery. It is not a seal witness and is
    /// never written back into the ledger or history as one.
    pub committed_text: String,
}

impl std::fmt::Display for TerminalSealRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "terminal transcript refused: {}",
            self.finality.reason().as_str()
        )?;
        if let Some(receipt) = self.finality.coverage() {
            write!(
                f,
                " ({}/{} samples covered; measured max gap {}; threshold {})",
                receipt.covered_samples,
                receipt.speech_samples,
                receipt.max_uncovered_samples,
                receipt.incomplete_threshold_samples
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for TerminalSealRefused {}

/// Producer-owned evidence after capture/archive or transcription-task failure.
/// A saved WAV is recovery material, never proof of a successful transcript.
/// There is deliberately no text field: the shared render buffer alone cannot
/// authenticate a committed revision after a worker fails.
#[derive(Debug)]
pub struct CaptureStopFailure {
    pub session_id: Option<String>,
    pub capture_epoch: u64,
    pub audio_path: Option<std::path::PathBuf>,
    pub cause: anyhow::Error,
    /// If archive finalization and the task both failed, keep both causes.
    pub task_failure: Option<anyhow::Error>,
}

impl std::fmt::Display for CaptureStopFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "capture processing failed: {:#}; committed text unavailable",
            self.cause
        )?;
        if let Some(path) = &self.audio_path {
            write!(f, "; source WAV retained at {}", path.display())?;
        } else {
            write!(f, "; no finalized WAV receipt")?;
        }
        if let Some(error) = &self.task_failure {
            write!(f, "; transcription task also failed: {error:#}")?;
        }
        Ok(())
    }
}

impl std::error::Error for CaptureStopFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

// Keep enough raw audio queued to survive a cold Whisper load without dropping
// the user's first words. The STT session drains this backlog once the model is ready.
/// Channel depth for cold Whisper load: first words queue instead of drop.
const AUDIO_BACKLOG_CHUNKS: usize = 2048;

/// Engine that owns the live canvas for every session this recorder starts.
///
/// `transcription_session` has exactly one live route (Apple progressive), so
/// the label is a constant today. It is still the recorder's fact, not the
/// controller's: a future runtime engine switch changes this owner and the
/// stop path keeps reporting whatever the session actually ran.
pub const LIVE_STREAMING_ENGINE_LABEL: &str = "live_apple";

/// Content-free witness returned by the production PCM replay seam.
#[derive(Debug)]
pub struct ProductionSessionReplay {
    /// Ordered event stream emitted by the same session implementation as live capture.
    pub events: Vec<EngineEvent>,
    /// The exact ledger mutated by the replayed production session. Consumers
    /// project these receipts; they must not reconstruct transcript authority
    /// from the legacy text events beside them.
    pub acoustic_ledger: Arc<StdMutex<AcousticLedger>>,
    /// Whether recording-start policy armed a Layer 1 provider before the
    /// single-use decision was consumed by the session.
    pub layer1_armed: bool,
    /// Engine that actually owned the live canvas for this replay session.
    pub streaming_engine_label: String,
    /// Typed local tail-patch arming and bounded-drain evidence emitted by the
    /// production session, when that session reached finality.
    pub tail_patch_receipt: Option<TailPatchSessionReceipt>,
}

/// Replay fixture PCM through the production recording-session cone.
///
/// The only differing boundary is PCM ingress: 100 ms in-memory chunks replace
/// CoreAudio callback blocks. Decision construction, `SessionConfig`, session
/// semantics, Layer 1 fan-out, VAD, Apple/Whisper events, and shutdown drainage
/// all remain owned by the same production symbols as microphone capture.
pub async fn replay_production_session(
    samples: &[f32],
    sample_rate: u32,
    language: Option<String>,
    settings: &UserSettings,
) -> Result<ProductionSessionReplay> {
    let runtime_settings = Arc::new(
        crate::config::Config::load_runtime_snapshot_without_keychain()
            .map_err(|error| anyhow!("runtime settings snapshot refused: {error:?}"))?,
    );
    let acoustic_ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
    let (layer1, _decision_receipt) = crate::asr_session::layer1_decision(&runtime_settings);
    let layer1_armed = layer1.is_armed();
    // `transcription_session` has one live canvas route: Apple progressive.
    // Report the route we actually enter; never reconstruct it through the
    // deleted global engine selector.
    let streaming_engine_label = LIVE_STREAMING_ENGINE_LABEL.to_string();
    let utterance_silence_sec = settings.toggle_silence_sec.filter(|&sec| sec >= 0.5);
    let config = SessionConfig {
        session_id: uuid::Uuid::new_v4().to_string(),
        capture_epoch: 1,
        runtime_settings,
        live_formatting_agent: None,
        acoustic_ledger: acoustic_ledger.clone(),
        sample_rate,
        capture_device_name: None,
        language,
        stream_log_path: None,
        utterance_silence_sec,
        // Replay reproduces a hands-free recording: the composer take is a UI
        // gesture with no offline equivalent, so this seam never fabricates one.
        capture_turn: CaptureTurnIntent::HandsFree,
        layer1,
        lifecycle_events: None,
        terminal_audio: None,
        last_window_closed: None,
    };
    let events = collect_buffered_engine_events_with_config(samples, config).await?;
    let tail_patch_receipt = TailPatchSessionReceipt::from_events(&events);
    Ok(ProductionSessionReplay {
        events,
        acoustic_ledger,
        layer1_armed,
        streaming_engine_label,
        tail_patch_receipt,
    })
}

/// One session input a recorded replay resolved: what the take recorded,
/// what the replay used, and why they differ when they do.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReplayInputReceipt {
    pub input: &'static str,
    /// The take's value from its card; `None` when the take has no card.
    pub recorded: Option<String>,
    pub effective: Option<String>,
    /// Set whenever `effective` is not the take's own value.
    pub reason: Option<String>,
}

/// A recorded session rebuilt for [`crate::pipeline::streaming::replay_buffered_engine_session`].
pub struct RecordedSessionReplay {
    /// The archive's PCM on the recorder's own i16 scale.
    pub samples: Vec<f32>,
    pub config: SessionConfig,
    pub inputs: Vec<ReplayInputReceipt>,
    /// The build that ran the take, from its card.
    pub recorded_build: Option<ReplayBuild>,
}

impl RecordedSessionReplay {
    /// Inputs whose replay value is not the take's own.
    pub fn divergences(&self) -> impl Iterator<Item = &ReplayInputReceipt> {
        self.inputs.iter().filter(|input| input.reason.is_some())
    }
}

/// Deliberate departures from the take. A replay under a scenario answers
/// "what would this take have given with X", not "what did it give"; every
/// input it changes is named in the receipt with a `scenario:` reason.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplayScenario {
    /// Replay as if the take had been captured on this device.
    pub capture_device: Option<String>,
    /// Replay with seal-ledger admission on or off.
    pub seal_ledger: Option<bool>,
}

impl ReplayScenario {
    /// A short session id tag, so a scenario's evidence sits beside the plain
    /// replay's instead of replacing it. At most 12 bytes: the trail limits a
    /// session id to 80, and a channel take's own id already uses 52.
    fn session_tag(&self) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(enabled) = self.seal_ledger {
            parts.push(if enabled { "lon" } else { "loff" }.to_string());
        }
        if let Some(device) = &self.capture_device {
            // FNV-1a: stable across runs and builds, unlike the std hasher.
            let hash = device.bytes().fold(0x811c_9dc5_u32, |hash, byte| {
                (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
            });
            parts.push(format!("d{:06x}", hash & 0x00ff_ffff));
        }
        (!parts.is_empty()).then(|| parts.join("-"))
    }
}

/// Rebuild one recorded session from its replay card and WAV archive.
///
/// With a card every input is the take's own: the settings record reseals the
/// take's digest, the capture device selects the same calibration, and the WAV
/// reaches the session as its terminal archive the way a recorder stop hands
/// it over. Without a card (sessions recorded before cards) each input
/// resolves to the nearest honest value and names where it came from.
///
/// Two lanes are never replayed: cloud Layer 1, because the card keeps no
/// credentials, and the live formatter, whose LLM output is not reproducible.
/// Replay parity is the reducer's Raw document. A [`ReplayScenario`] changes
/// named inputs on purpose; its replay is a what-if, not a parity check.
pub fn prepare_recorded_replay(
    recorded_session: Option<&str>,
    card: Option<&ReplayCard>,
    wav: &std::path::Path,
    run: u32,
    scenario: &ReplayScenario,
) -> Result<RecordedSessionReplay> {
    let (samples, sample_rate, recorder_native) = load_replay_pcm(wav)?;
    if samples.is_empty() {
        return Err(anyhow!("replay audio contains no samples"));
    }
    let no_card = |what: &str| Some(format!("no replay card: {what}"));
    let mut inputs = Vec::new();

    let snapshot = match card {
        Some(card) => crate::config::Config::runtime_snapshot_from_captured(
            card.settings.clone().into_captured(),
        ),
        None => crate::config::Config::load_runtime_snapshot_without_keychain()
            .map_err(|error| anyhow!("runtime settings snapshot refused: {error:?}"))?,
    };
    let recorded_ledger = card.map(|_| snapshot.seal_ledger_enabled());
    let ledger_changed = scenario
        .seal_ledger
        .is_some_and(|enabled| enabled != snapshot.seal_ledger_enabled());
    let runtime_settings = match scenario.seal_ledger {
        Some(enabled) if ledger_changed => {
            let mut captured = snapshot.captured_inputs().clone();
            captured.user_settings.seal_ledger_enabled = Some(enabled);
            crate::config::Config::runtime_snapshot_from_captured(captured)
        }
        _ => snapshot,
    };
    let effective_digest = runtime_settings.digest().as_str().to_string();
    inputs.push(ReplayInputReceipt {
        input: "settings_digest",
        recorded: card.map(|card| card.settings_digest.clone()),
        reason: match card {
            None => no_card("host settings at replay time"),
            Some(_) if ledger_changed => {
                Some("scenario: --ledger changes the take's settings".into())
            }
            Some(card) => (effective_digest != card.settings_digest).then(|| {
                "this build resolves the recorded settings to another snapshot".to_string()
            }),
        },
        effective: Some(effective_digest),
    });
    inputs.push(ReplayInputReceipt {
        input: "seal_ledger",
        recorded: recorded_ledger.map(|enabled| enabled.to_string()),
        effective: Some(runtime_settings.seal_ledger_enabled().to_string()),
        reason: match (scenario.seal_ledger, card) {
            (Some(_), Some(_)) if !ledger_changed => None,
            (Some(_), _) => Some("scenario: --ledger".into()),
            (None, Some(_)) => None,
            (None, None) => no_card("ledger choice from host settings"),
        },
    });
    let runtime_settings = Arc::new(runtime_settings);
    let values = runtime_settings.values();

    let recorded_rate = card.map(|card| card.sample_rate);
    inputs.push(ReplayInputReceipt {
        input: "sample_rate",
        recorded: recorded_rate.map(|rate| rate.to_string()),
        effective: Some(sample_rate.to_string()),
        reason: match recorded_rate {
            Some(rate) if rate == sample_rate => None,
            Some(_) => Some("the audio file is not at the take's capture rate".into()),
            None => no_card("rate from the audio file"),
        },
    });

    let calibrated_devices = runtime_settings
        .energy_calibration()
        .artifact()
        .map(|artifact| artifact.device_names())
        .unwrap_or_default();
    let recorded_device = card.and_then(|card| card.capture_device_name.clone());
    let capture_device_name = match (&scenario.capture_device, card) {
        (Some(device), _) => Some(device.clone()),
        (None, Some(_)) => recorded_device.clone(),
        (None, None) if calibrated_devices.len() == 1 => calibrated_devices.first().cloned(),
        (None, None) => None,
    };
    inputs.push(ReplayInputReceipt {
        input: "capture_device",
        reason: match (&scenario.capture_device, card, &capture_device_name) {
            (Some(device), Some(_), _) if recorded_device.as_ref() == Some(device) => None,
            (Some(device), _, _) if calibrated_devices.contains(device) => {
                Some("scenario: --device".into())
            }
            (Some(_), _, _) => Some(
                "scenario: --device names a device the energy calibration has no profile for"
                    .into(),
            ),
            (None, Some(_), _) => None,
            (None, None, Some(_)) => no_card("the only calibrated device"),
            (None, None, None) => no_card("device unknown, so no occurrence can qualify"),
        },
        recorded: recorded_device,
        effective: capture_device_name.clone(),
    });

    let language = match card {
        Some(card) => card.language.clone(),
        None => values.whisper_language.whisper_hint().map(str::to_string),
    };
    inputs.push(ReplayInputReceipt {
        input: "language",
        recorded: card.and_then(|card| card.language.clone()),
        effective: language.clone(),
        reason: card
            .is_none()
            .then(|| "no replay card: language from settings".into()),
    });

    let capture_turn = match card {
        Some(card) => card.capture_turn,
        None if recorded_session.is_some_and(|session| session.starts_with("agent-channel-")) => {
            CaptureTurnIntent::AgentChannel
        }
        None => CaptureTurnIntent::HandsFree,
    };
    inputs.push(ReplayInputReceipt {
        input: "capture_turn",
        recorded: card.map(|card| format!("{:?}", card.capture_turn)),
        effective: Some(format!("{capture_turn:?}")),
        reason: card.is_none().then(|| {
            "no replay card: capture turn from the session id; a composer take reads as hands-free"
                .into()
        }),
    });

    let utterance_silence_sec = match card {
        Some(card) => card.utterance_silence_sec,
        None => capture_turn.utterance_silence_sec(values.toggle_silence_sec),
    };
    inputs.push(ReplayInputReceipt {
        input: "utterance_silence_sec",
        recorded: card.and_then(|card| card.utterance_silence_sec.map(|sec| sec.to_string())),
        effective: utterance_silence_sec.map(|sec| sec.to_string()),
        reason: card
            .is_none()
            .then(|| "no replay card: threshold from settings".into()),
    });

    let (rebuilt, _receipt) = crate::asr_session::layer1_decision(&runtime_settings);
    let rebuilt_token = rebuilt.receipt_token();
    let cloud = matches!(
        rebuilt,
        crate::asr_session::recorder::Layer1Decision::Armed(_)
            | crate::asr_session::recorder::Layer1Decision::Cloud { .. }
    );
    let layer1 = if cloud {
        crate::asr_session::recorder::Layer1Decision::Disarmed
    } else {
        rebuilt
    };
    let recorded_layer1 = card.map(|card| card.layer1.clone());
    inputs.push(ReplayInputReceipt {
        input: "layer1",
        effective: Some(layer1.receipt_token()),
        reason: if cloud {
            Some(format!(
                "{rebuilt_token} is not replayed: the card keeps no credentials"
            ))
        } else if card.is_none() {
            no_card("decision rebuilt from host settings")
        } else {
            (recorded_layer1.as_deref() != Some(rebuilt_token.as_str()))
                .then(|| "the rebuilt decision differs from the take's".into())
        },
        recorded: recorded_layer1,
    });

    let recorded_formatter = card.map(|card| card.live_formatter);
    inputs.push(ReplayInputReceipt {
        input: "live_formatter",
        recorded: recorded_formatter.map(|bound| bound.to_string()),
        effective: Some(false.to_string()),
        reason: (recorded_formatter != Some(false))
            .then(|| "the formatter is not replayed: its LLM output is outside parity".into()),
    });

    let recorded_archive = card.map(|card| card.terminal_archive);
    let terminal_archive = recorder_native && recorded_archive != Some(false);
    inputs.push(ReplayInputReceipt {
        input: "terminal_archive",
        recorded: recorded_archive.map(|handed| handed.to_string()),
        effective: Some(terminal_archive.to_string()),
        reason: if !recorder_native && recorded_archive != Some(false) {
            Some("the audio file is not a recorder archive (mono 16-bit PCM)".into())
        } else {
            card.is_none()
                .then(|| "no replay card: the archive is handed over like a stop".into())
        },
    });

    // Some engine modules read env during the take instead of the settings
    // snapshot. The card cannot carry those values until they move into it.
    inputs.push(ReplayInputReceipt {
        input: "engine_env",
        recorded: None,
        effective: None,
        reason: Some(
            "env read by the engine outside the settings snapshot is this process's, not the take's"
                .into(),
        ),
    });
    // Read from the data dir when the session needs them, as they are now.
    inputs.push(ReplayInputReceipt {
        input: "data_dir_files",
        recorded: None,
        effective: None,
        reason: Some(
            "custom lexicon, protected terms and models are read as they are now, not as the take saw them"
                .into(),
        ),
    });

    let tag = scenario
        .session_tag()
        .map(|tag| format!("-{tag}"))
        .unwrap_or_default();
    let session_id = recorded_session
        .map(|session| format!("replay-{session}{tag}-r{run}"))
        .filter(|id| crate::pipeline::trail::trail_path(std::path::Path::new("/"), id).is_ok())
        .unwrap_or_else(|| format!("replay-{}", uuid::Uuid::new_v4()));
    let capture_epoch = card.map_or(1, |card| card.capture_epoch);
    let terminal_audio = terminal_archive.then(|| {
        let (sender, receiver) = std::sync::mpsc::channel();
        let _ = sender.send(Ok(
            crate::pipeline::streaming::live_audio_buffer::FinalizedPcmArchive {
                session_id: session_id.clone(),
                capture_epoch,
                sample_rate,
                sample_count: samples.len() as u64,
                path: wav.to_path_buf(),
            },
        ));
        receiver
    });

    Ok(RecordedSessionReplay {
        config: SessionConfig {
            session_id,
            capture_epoch,
            runtime_settings,
            live_formatting_agent: None,
            acoustic_ledger: Arc::new(StdMutex::new(AcousticLedger::new())),
            sample_rate,
            capture_device_name,
            language,
            stream_log_path: None,
            utterance_silence_sec,
            capture_turn,
            layer1,
            // A live take's lifecycle channel carries host events (sleep,
            // route loss) that a replay has none of; absent equals idle.
            lifecycle_events: None,
            terminal_audio,
            last_window_closed: None,
        },
        samples,
        inputs,
        recorded_build: card.map(|card| card.build.clone()),
    })
}

/// A take archive decodes on the recorder's own i16 scale, as the terminal
/// witness reads it (`FinalizedPcmArchive::load`), so the fed PCM and the
/// handed-over archive are the same samples. Other audio uses the shared loader.
fn load_replay_pcm(wav: &std::path::Path) -> Result<(Vec<f32>, u32, bool)> {
    if let Ok(reader) = hound::WavReader::open(wav) {
        let spec = reader.spec();
        if spec.channels == 1
            && spec.bits_per_sample == 16
            && spec.sample_format == hound::SampleFormat::Int
        {
            let samples = reader
                .into_samples::<i16>()
                .map(|sample| sample.map(|sample| f32::from(sample) / f32::from(i16::MAX)))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| anyhow!("decode replay archive failed"))?;
            return Ok((samples, spec.sample_rate, true));
        }
    }
    let (samples, sample_rate) =
        crate::audio::load_audio_file(wav).map_err(|_| anyhow!("load replay audio failed"))?;
    Ok((samples, sample_rate, false))
}

/// A recording session that transcribes while it captures.
///
/// Configure the sink and any callbacks first, then call
/// [`StreamingRecorder::start_event_session`]; the sink is cleared on stop, so
/// it must be set again for each session.
pub struct StreamingRecorder {
    pub recorder: Recorder,
    transcript_buffer: Arc<Mutex<String>>,
    transcription_handle: Option<JoinHandle<()>>,
    sample_rate: u32,
    utterance_callback: Option<Arc<dyn Fn(String) + Send + Sync>>,
    utterance_silence_sec: Option<f32>,
    /// How many turns the next/active take owns. Set by the surface that opened
    /// the microphone and cleared back to the default between sessions, so a
    /// one-turn composer take can never leak into the next hands-free one.
    capture_turn: CaptureTurnIntent,
    /// Counter for audio chunks dropped due to channel backpressure.
    dropped_chunks: Arc<AtomicU64>,
    /// Sink used by `start_event_session`. Caller must configure it explicitly.
    event_sink: Option<Arc<dyn EventSink>>,
    /// Per-block input level tap: receives the RMS of every captured audio
    /// block (linear, 0..~1). Runs on the CoreAudio callback thread — keep it
    /// cheap and non-blocking (a broadcast send, an atomic store).
    level_callback: Option<Arc<dyn Fn(f32) + Send + Sync>>,
    /// O(1) host lifecycle signal for the currently active session.
    lifecycle_handle: Option<RecorderLifecycleHandle>,
    /// Session-frozen runtime truth. Set once by the controller before start.
    runtime_settings: Option<Arc<RuntimeSettingsSnapshot>>,
    /// Bound by the host after session authority; cleared on each new bind.
    live_formatting_agent: Option<Arc<dyn crate::ai_formatting::FormattingAgent>>,
    /// The one ledger instance shared by PCM capture, engines, and reducer.
    acoustic_ledger: Option<Arc<StdMutex<AcousticLedger>>>,
    /// Controller-owned session identity bound with the ledger.
    authority_session_id: Option<String>,
    /// Last capture-open epoch issued for the currently bound session.
    /// Zero means this bind has not successfully opened capture yet.
    capture_epoch: u64,
    /// Capture subscribers currently holding the physical capture open. The
    /// take registers here like any other subscriber; the stream closes when
    /// the last subscriber releases, not when a take ends.
    capture_subscribers: Vec<(CaptureSubscriberId, CaptureSubscriberKind)>,
    /// Monotonic source of subscriber identities.
    next_capture_subscriber: u64,
    /// The subscription the active take session owns, if any.
    take_subscriber: Option<CaptureSubscriberId>,
    /// Bounded PCM fan-out, one feed per subscriber. The capture callback
    /// offers every block to every feed and never blocks on a consumer (the
    /// `RecorderLayer1Lane::offer_pcm` contract): a full feed drops the block
    /// and counts it, a closed feed is reaped.
    pcm_feeds: Arc<PcmFeedRegistry>,
    /// Agent-channel tasks. Separate from the take's `transcription_handle`.
    channel_tasks: Vec<ChannelTask>,
    captured_samples: Arc<AtomicU64>,
    /// Exact take archive, independent of the physical capture's lifetime.
    take_archive: Option<TakeArchive>,
    /// The existing capture owner's archive result, frozen before a closed
    /// take can leave the controller slot. Consumed by its ordinary stop tail.
    prepared_capture_archive: Option<Result<Option<std::path::PathBuf>>>,
    /// The actual close result, retained for inline direct-mode stop callers.
    closed_capture_was_active: Option<bool>,
    /// Retained stop owner for callers that cancel and retry inline settlement.
    terminal_take: Option<ClosedTake>,
    terminal_audio_sender: Option<
        std::sync::mpsc::Sender<
            Result<crate::pipeline::streaming::live_audio_buffer::FinalizedPcmArchive, String>,
        >,
    >,
    last_window_closed: Option<oneshot::Receiver<()>>,
}

/// One subscriber's PCM sender. `kind` lets the callback drop channel blocks
/// while agent speech owns the speaker without touching a take feed.
struct PcmFeed {
    id: CaptureSubscriberId,
    kind: CaptureSubscriberKind,
    sender: mpsc::Sender<Vec<f32>>,
    archive: Option<TakeArchiveFeed>,
}

struct TakeArchiveFeed {
    sender: SpillSender,
    captured_samples: Arc<AtomicU64>,
    send_failed: Arc<AtomicBool>,
    /// PCM the bounded spill queue refused, in capture order. Shared with the
    /// owning [`TakeArchive`], which merges it into the recovery evidence.
    refused: Arc<StdMutex<Vec<i16>>>,
}

/// One subscriber's exact PCM archive: the bounded disk spill plus every
/// block its queue refused. A writer stall must not turn a complete capture
/// into an unrecoverable take, so refused PCM stays in RAM as evidence.
pub(crate) struct TakeArchive {
    sink: SpillSink,
    sample_rate: u32,
    captured_samples: Arc<AtomicU64>,
    refused: Arc<StdMutex<Vec<i16>>>,
}

impl TakeArchive {
    pub(crate) fn spawn(sample_rate: u32, captured_samples: Arc<AtomicU64>) -> Result<Self> {
        Ok(Self {
            sink: SpillSink::spawn_take(sample_rate)?,
            sample_rate,
            captured_samples,
            refused: Arc::new(StdMutex::new(Vec::new())),
        })
    }

    fn feed(&self) -> Result<TakeArchiveFeed> {
        Ok(TakeArchiveFeed {
            sender: self
                .sink
                .sender()
                .context("Take archive has no sample sender")?,
            captured_samples: Arc::clone(&self.captured_samples),
            send_failed: self.sink.send_failure_handle(),
            refused: Arc::clone(&self.refused),
        })
    }

    fn captured_samples(&self) -> u64 {
        self.captured_samples.load(Ordering::Relaxed)
    }

    /// Close the spill and account for every refused block. The queue is FIFO
    /// and refusal latches, so refused PCM is exactly the tail after whatever
    /// the writer confirmed or retained. It is reported as unconfirmed PCM the
    /// existing recovery path splices after the written prefix.
    fn finalize(self) -> std::result::Result<(std::path::PathBuf, usize), CaptureArchiveError> {
        let Self {
            sink,
            sample_rate,
            refused,
            ..
        } = self;
        let refused = std::mem::take(
            &mut *refused
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        let mut error = match sink.finalize() {
            Ok(written) if refused.is_empty() => return Ok(written),
            Ok((path, written)) => CaptureArchiveError {
                cause: "take archive queue refused PCM; refused samples retained in memory".into(),
                source_path: Some(path),
                sample_rate,
                captured_samples: 0,
                written_samples: written,
                unwritten_samples: Arc::new(Vec::new()),
                buffer_start_offset: 0,
                retained_samples: Arc::new(Vec::new()),
            },
            Err(error) => error,
        };
        if !refused.is_empty() {
            let mut unwritten = error.unwritten_samples.as_ref().clone();
            unwritten.extend(refused);
            error.unwritten_samples = Arc::new(unwritten);
        }
        if error.retained_samples.is_empty() {
            // A take spill has no RAM ring: the unconfirmed tail ends the take.
            error.buffer_start_offset = error.written_samples + error.unwritten_samples.len();
        }
        error.captured_samples = error.written_samples + error.unwritten_samples.len();
        Err(error)
    }
}

/// Subscriber feeds behind one lock.
type PcmFeedRegistry = StdMutex<Vec<PcmFeed>>;

/// One agent-channel transcription task riding the shared capture.
struct ChannelTask {
    subscriber: CaptureSubscriberId,
    task: JoinHandle<()>,
    lifecycle: RecorderLifecycleHandle,
    last_window: oneshot::Receiver<()>,
    /// The channel's own exact archive, scoped to its subscription. The
    /// device spill spans every subscriber and is never this channel's audio.
    archive: Option<TakeArchive>,
    /// Terminal handoff of that archive to the channel's own session, as a
    /// take's. Sent after finalization and before the task is joined: the
    /// session's stop tail waits on it to run the terminal dictation witness.
    terminal_audio_sender: Option<
        std::sync::mpsc::Sender<
            Result<crate::pipeline::streaming::live_audio_buffer::FinalizedPcmArchive, String>,
        >,
    >,
    /// Identity the session checks the archive receipt against.
    session_id: String,
    sample_rate: u32,
}

/// What [`StreamingRecorder::register_channel_feed`] handed the caller.
pub struct ChannelFeedRegistration {
    pub id: CaptureSubscriberId,
    pub receiver: mpsc::Receiver<Vec<f32>>,
    /// `true` when no physical stream is open. The caller owes `recorder.start()`.
    pub needs_physical_open: bool,
}

/// Hands-free silence for one channel session.
///
/// This is the channel's own `EpochGate::for_session` input. It is never read
/// from the take's `utterance_silence_sec` field.
pub fn channel_session_silence(configured_sec: f32) -> Option<f32> {
    CaptureTurnIntent::HandsFree.utterance_silence_sec(configured_sec)
}

impl StreamingRecorder {
    /// Seal order from the immutable generation bound to this capture.
    pub fn seal_lane_armed(&self) -> bool {
        self.runtime_settings
            .as_ref()
            .is_some_and(|settings| settings.seal_lane_armed())
    }

    /// Light+ pause from the generation frozen for this take.
    pub fn light_plus_sentence_pause_sec(&self) -> f32 {
        self.runtime_settings.as_ref().map_or(0.7, |settings| {
            settings.values().light_plus_sentence_pause_sec
        })
    }

    /// Build a streaming recorder over a default-configured [`Recorder`].
    ///
    /// No sink is attached yet, so `start_event_session` will refuse until
    /// [`Self::set_event_sink`] is called.
    pub fn new() -> Result<Self> {
        let recorder = Recorder::new()?;
        let sample_rate = recorder.config.sample_rate;

        Ok(Self {
            recorder,
            transcript_buffer: Arc::new(Mutex::new(String::new())),
            transcription_handle: None,
            sample_rate,
            utterance_callback: None,
            utterance_silence_sec: None,
            capture_turn: CaptureTurnIntent::HandsFree,
            dropped_chunks: Arc::new(AtomicU64::new(0)),
            event_sink: None,
            level_callback: None,
            lifecycle_handle: None,
            runtime_settings: None,
            live_formatting_agent: None,
            acoustic_ledger: None,
            authority_session_id: None,
            capture_epoch: 0,
            capture_subscribers: Vec::new(),
            next_capture_subscriber: 0,
            take_subscriber: None,
            pcm_feeds: Arc::new(StdMutex::new(Vec::new())),
            channel_tasks: Vec::new(),
            captured_samples: Arc::new(AtomicU64::new(0)),
            take_archive: None,
            prepared_capture_archive: None,
            terminal_take: None,
            closed_capture_was_active: None,
            terminal_audio_sender: None,
            last_window_closed: None,
        })
    }

    /// Build a streaming recorder over a specific [`RecorderConfig`].
    ///
    /// The configured sample rate is only provisional — it is corrected to the
    /// device's actual rate once the stream opens.
    pub fn with_config(config: RecorderConfig) -> Result<Self> {
        let sample_rate = config.sample_rate;
        let recorder = Recorder::with_config(config)?;

        Ok(Self {
            recorder,
            transcript_buffer: Arc::new(Mutex::new(String::new())),
            transcription_handle: None,
            sample_rate,
            utterance_callback: None,
            utterance_silence_sec: None,
            capture_turn: CaptureTurnIntent::HandsFree,
            dropped_chunks: Arc::new(AtomicU64::new(0)),
            event_sink: None,
            level_callback: None,
            lifecycle_handle: None,
            runtime_settings: None,
            live_formatting_agent: None,
            acoustic_ledger: None,
            authority_session_id: None,
            capture_epoch: 0,
            capture_subscribers: Vec::new(),
            next_capture_subscriber: 0,
            take_subscriber: None,
            pcm_feeds: Arc::new(StdMutex::new(Vec::new())),
            channel_tasks: Vec::new(),
            captured_samples: Arc::new(AtomicU64::new(0)),
            take_archive: None,
            prepared_capture_archive: None,
            terminal_take: None,
            closed_capture_was_active: None,
            terminal_audio_sender: None,
            last_window_closed: None,
        })
    }

    /// Track choices from this take's frozen generation; absent snapshots carry no opt-in.
    pub fn seal_ledger_enabled(&self) -> bool {
        self.runtime_settings
            .as_ref()
            .is_some_and(|settings| settings.seal_ledger_enabled())
    }

    pub fn apple_live_enabled(&self) -> bool {
        self.runtime_settings
            .as_ref()
            .is_some_and(|settings| settings.apple_live_enabled())
    }

    /// Bind the next capture to one immutable settings snapshot and one ledger.
    pub fn bind_session_authority(
        &mut self,
        session_id: String,
        runtime_settings: Arc<RuntimeSettingsSnapshot>,
    ) -> Arc<StdMutex<AcousticLedger>> {
        let acoustic_ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        self.capture_epoch = 0;
        self.authority_session_id = Some(session_id);
        self.runtime_settings = Some(runtime_settings);
        self.live_formatting_agent = None;
        self.acoustic_ledger = Some(Arc::clone(&acoustic_ledger));
        acoustic_ledger
    }

    /// Supply the existing host executor without creating another Agent session.
    /// The streaming lane still owes explicit whole-instruction admission.
    pub fn set_live_formatting_agent(
        &mut self,
        agent: Option<Arc<dyn crate::ai_formatting::FormattingAgent>>,
    ) {
        self.live_formatting_agent = agent;
    }

    /// Borrow the ledger handle already bound for the next/active session.
    pub fn acoustic_ledger_handle(&self) -> Option<Arc<StdMutex<AcousticLedger>>> {
        self.acoustic_ledger.as_ref().map(Arc::clone)
    }

    /// Identity bound at capture open, not a latest-file or UI-slot lookup.
    pub fn capture_identity(&self) -> (Option<&str>, u64) {
        (self.authority_session_id.as_deref(), self.capture_epoch)
    }

    /// Register one capture subscriber with its bounded PCM feed.
    ///
    /// [`Self::capture_subscriber_count`] tells the caller whether it opened
    /// the capture (first subscriber) or joined an already open one. The feed
    /// is offered every captured block until the subscription is released.
    pub fn acquire_capture_subscriber(
        &mut self,
        kind: CaptureSubscriberKind,
        pcm_feed: mpsc::Sender<Vec<f32>>,
    ) -> CaptureSubscriberId {
        self.acquire_archived_capture_subscriber(kind, pcm_feed, None)
    }

    /// Register the feed and its archive in one registry push, so the archive
    /// hears exactly the blocks the feed hears.
    fn acquire_archived_capture_subscriber(
        &mut self,
        kind: CaptureSubscriberKind,
        pcm_feed: mpsc::Sender<Vec<f32>>,
        archive: Option<TakeArchiveFeed>,
    ) -> CaptureSubscriberId {
        self.next_capture_subscriber += 1;
        let id = CaptureSubscriberId(self.next_capture_subscriber);
        self.capture_subscribers.push((id, kind));
        self.pcm_feeds
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(PcmFeed {
                id,
                kind,
                sender: pcm_feed,
                archive,
            });
        id
    }

    /// Release one subscription. Returns `true` when it was the last one —
    /// the caller then owes the physical capture close. Dropping the feed
    /// closes that subscriber's PCM channel, so its session observes
    /// end-of-capture and drains exactly as a physical close would make it.
    pub fn release_capture_subscriber(&mut self, id: CaptureSubscriberId) -> bool {
        let before = self.capture_subscribers.len();
        self.capture_subscribers.retain(|(held, _)| *held != id);
        if self.capture_subscribers.len() == before {
            warn!(?id, "release of an unknown capture subscriber");
        }
        self.pcm_feeds
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|feed| feed.id != id);
        if self.take_subscriber == Some(id) {
            self.take_subscriber = None;
        }
        self.capture_subscribers.is_empty()
    }

    /// Subscribers currently holding the capture open.
    pub fn capture_subscriber_count(&self) -> usize {
        self.capture_subscribers.len()
    }

    /// Whether any subscriber holds the capture open.
    pub fn has_capture_subscribers(&self) -> bool {
        !self.capture_subscribers.is_empty()
    }

    /// Whether a dictation take currently subscribes to the capture.
    pub fn has_take_subscriber(&self) -> bool {
        self.capture_subscribers
            .iter()
            .any(|(_, kind)| *kind == CaptureSubscriberKind::Take)
    }

    /// Whether a non-take consumer (agent channel) currently subscribes.
    pub fn has_non_take_subscriber(&self) -> bool {
        self.capture_subscribers
            .iter()
            .any(|(_, kind)| *kind != CaptureSubscriberKind::Take)
    }

    /// Keep this exact subscriber's real PCM flowing until the gesture's fixed
    /// deadline. `None` selects the current take. The controller owns the stop
    /// task and retains exclusive access until it releases this feed; neither a
    /// cancelled waiter nor a repeated Stop can restart this budget.
    ///
    /// Automatic silence, handoff and cleanup do not call this. No synthetic
    /// samples are added, and inactive or already released capture never waits.
    pub async fn await_stop_pause(
        &mut self,
        subscriber: Option<CaptureSubscriberId>,
        stopped_at: Instant,
    ) {
        let Some(id) = subscriber.or(self.take_subscriber) else {
            return;
        };
        if !self.recorder.is_active()
            || !self.capture_subscribers.iter().any(|(held, _)| *held == id)
        {
            return;
        }
        let deadline = tokio::time::Instant::from_std(stopped_at) + STOP_CAPTURE_GRACE;
        info!(?id, "retaining live PCM for explicit Stop grace");
        tokio::time::sleep_until(deadline).await;
    }

    /// Release the take's own subscription ahead of the stop tail.
    fn release_take_pcm_feed(&mut self) -> TakeFeedRelease {
        let Some(id) = self.take_subscriber.take() else {
            return TakeFeedRelease::NoTakeFeed;
        };
        if self.release_capture_subscriber(id) {
            TakeFeedRelease::LastSubscriber
        } else {
            TakeFeedRelease::CaptureShared
        }
    }

    /// Store a per-utterance text callback.
    ///
    /// Note: the stored value is currently never read by this type — completed
    /// utterances reach consumers as `EngineEvent`s through the event sink
    /// instead. The live per-utterance callback is the one on the presentation
    /// emitter, not this one.
    pub fn set_utterance_callback(&mut self, callback: Option<Arc<dyn Fn(String) + Send + Sync>>) {
        self.utterance_callback = callback;
    }

    /// Override how much trailing silence closes an utterance.
    ///
    /// Read when the session starts and passed into `SessionConfig`, so it has
    /// to be set before [`Self::start_event_session`]. `None` keeps the
    /// pipeline default.
    pub fn set_utterance_silence_sec(&mut self, silence_sec: Option<f32>) {
        self.utterance_silence_sec = silence_sec;
    }

    /// Declare how many turns the next take owns.
    ///
    /// Read when the session starts and frozen into `SessionConfig`, so it has
    /// to be set before [`Self::start_event_session`]. The controller resets it
    /// to [`CaptureTurnIntent::HandsFree`] between sessions: a one-turn
    /// composer take must never widen into the hands-free take that follows it.
    pub fn set_capture_turn_intent(&mut self, capture_turn: CaptureTurnIntent) {
        self.capture_turn = capture_turn;
    }

    /// The capture intent frozen for the next/active take.
    pub fn capture_turn_intent(&self) -> CaptureTurnIntent {
        self.capture_turn
    }

    /// Set the per-block input-level tap consumed by UI meters (overlay
    /// waveform). Configure before `start_event_session`; cleared alongside the
    /// other callbacks between sessions.
    pub fn set_level_callback(&mut self, callback: Option<Arc<dyn Fn(f32) + Send + Sync>>) {
        self.level_callback = callback;
    }

    /// Returns a cloned handle to the transcript buffer.
    ///
    /// Shared delivery buffer. Only committed reducer projections may write it;
    /// previews are ephemeral paint and `stop()` only reads the accumulated
    /// committed rendering.
    pub fn transcript_buffer_handle(&self) -> Arc<Mutex<String>> {
        self.transcript_buffer.clone()
    }

    /// Set the event sink for the unified pipeline.
    pub fn set_event_sink(&mut self, sink: Option<Arc<dyn EventSink>>) {
        self.event_sink = sink;
    }

    /// Clone the active session sink so controller-owned presentation events
    /// can enter the same ordered reducer/fanout as engine events.
    pub fn event_sink_handle(&self) -> Option<Arc<dyn EventSink>> {
        self.event_sink.clone()
    }

    /// Returns true when the underlying recorder still has an active audio stream.
    pub fn is_recording(&self) -> bool {
        self.recorder.is_active()
    }

    /// Notify the active transcription task that the host crossed sleep/wake.
    ///
    /// No active capture is a normal no-op. This method only enqueues a typed
    /// boundary; the session loop owns the fail-closed Layer 1 transition.
    pub fn note_sleep_wake(&self) -> bool {
        self.recorder.is_active()
            && self
                .lifecycle_handle
                .as_ref()
                .is_some_and(RecorderLifecycleHandle::note_sleep_wake)
    }

    /// Engine label of the live route this recorder's sessions run on.
    ///
    /// The stop path publishes this as the serving truth for Settings
    /// "Active STT"; it must never be reconstructed from the configured
    /// `stt_engine` preference (see `controller::serving_status`).
    pub fn streaming_engine_label(&self) -> &'static str {
        LIVE_STREAMING_ENGINE_LABEL
    }

    /// Start recording with the new event-based pipeline.
    ///
    /// Uses `transcription_session` which emits `EngineEvent`s to the configured
    /// `event_sink`.
    pub async fn start_event_session(&mut self, language: Option<String>) -> Result<()> {
        anyhow::ensure!(
            self.prepared_capture_archive.is_none()
                && self.take_archive.is_none()
                && self.terminal_take.is_none(),
            "previous closed capture still owns its terminal archive result"
        );
        let event_sink = self.event_sink.clone().ok_or_else(|| {
            anyhow!(
                "start_event_session requires event_sink (set_event_sink(Some(...)) before start)"
            )
        })?;
        let session_id = self
            .authority_session_id
            .clone()
            .ok_or_else(|| anyhow!("start_event_session requires bound session authority"))?;
        let runtime_settings = self
            .runtime_settings
            .as_ref()
            .map(Arc::clone)
            .ok_or_else(|| anyhow!("start_event_session requires RuntimeSettingsSnapshot"))?;
        self.closed_capture_was_active = None;
        let acoustic_ledger = self
            .acoustic_ledger
            .as_ref()
            .map(Arc::clone)
            .ok_or_else(|| anyhow!("start_event_session requires AcousticLedger"))?;
        if self.has_take_subscriber() {
            return Err(anyhow!(
                "start_event_session refused: a dictation take already subscribes to this capture"
            ));
        }
        let next_capture_epoch = self.capture_epoch.checked_add(1).ok_or_else(|| {
            anyhow!("capture epoch overflow: no unused epoch remains for the bound session")
        })?;

        // Clear previous transcript and reset drop counter
        *self.transcript_buffer.lock().await = String::new();
        self.dropped_chunks.store(0, Ordering::Relaxed);
        self.captured_samples = Arc::new(AtomicU64::new(0));

        // Create channel for audio chunks. This is intentionally larger than a
        // normal live queue: cold STT initialization happens behind this buffer.
        let (tx, rx) = mpsc::channel::<Vec<f32>>(AUDIO_BACKLOG_CHUNKS);

        // Open the one physical capture before admitting the take, so its
        // archive uses the actual native rate even when config differs.
        let shared_capture = self.has_non_take_subscriber() && self.recorder.is_active();
        self.install_pcm_fanout_callback();
        if !shared_capture && let Err(error) = self.recorder.start().await {
            return Err(error);
        }
        let actual_sample_rate = self.recorder.actual_sample_rate();
        let archive =
            match TakeArchive::spawn(actual_sample_rate, Arc::clone(&self.captured_samples)) {
                Ok(archive) => archive,
                Err(error) => {
                    if !shared_capture {
                        let _ = self.recorder.stop().await;
                    }
                    return Err(error.context("Cannot admit take without its native PCM archive"));
                }
            };
        let archive_feed = archive.feed()?;
        self.next_capture_subscriber += 1;
        let take_subscriber = CaptureSubscriberId(self.next_capture_subscriber);
        self.capture_subscribers
            .push((take_subscriber, CaptureSubscriberKind::Take));
        self.pcm_feeds
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(PcmFeed {
                id: take_subscriber,
                kind: CaptureSubscriberKind::Take,
                sender: tx,
                archive: Some(archive_feed),
            });
        self.take_subscriber = Some(take_subscriber);
        self.take_archive = Some(archive);
        self.capture_epoch = next_capture_epoch;
        event_sink.on_capture_opened(&session_id, next_capture_epoch);

        // Update sample rate to match real input stream
        let capture_device_name = self.recorder.last_input_device().map(str::to_owned);
        crate::audio::capture_receipt::publish_open_capture_path(
            crate::audio::capture_receipt::CapturePathMeta::from_open_path(
                actual_sample_rate,
                self.recorder.last_native_channels(),
                self.recorder.last_input_device(),
            ),
        );
        if actual_sample_rate != self.sample_rate {
            info!(
                "StreamingRecorder sample_rate updated: config={}Hz -> actual={}Hz",
                self.sample_rate, actual_sample_rate
            );
            self.sample_rate = actual_sample_rate;
        }

        let log_path = stream_log_path();
        let utterance_silence_sec = self.utterance_silence_sec;
        let capture_turn = self.capture_turn;
        let live_formatting_agent = live_max_capability(
            capture_turn,
            runtime_settings.values().ai_formatting_enabled,
            runtime_settings.formatting_policy(),
            self.live_formatting_agent.as_ref(),
        );

        let (layer1, _decision_receipt) = crate::asr_session::layer1_decision(&runtime_settings);
        let (lifecycle_handle, lifecycle_events) = recorder_lifecycle_channel();
        self.lifecycle_handle = Some(lifecycle_handle);
        // Every take owes its own PCM receipt; channel sharing never changes
        // the terminal handoff or makes it wait for whole-device closure.
        let (terminal_tx, terminal_rx) = std::sync::mpsc::channel();
        self.terminal_audio_sender = Some(terminal_tx);
        let terminal_audio = Some(terminal_rx);
        let (last_window_tx, last_window_rx) = oneshot::channel();
        self.last_window_closed = Some(last_window_rx);
        self.transcription_handle = Some(tokio::spawn(async move {
            transcription_session(
                rx,
                event_sink,
                SessionConfig {
                    session_id,
                    capture_epoch: next_capture_epoch,
                    runtime_settings,
                    live_formatting_agent,
                    acoustic_ledger,
                    sample_rate: actual_sample_rate,
                    capture_device_name,
                    language,
                    stream_log_path: log_path,
                    utterance_silence_sec,
                    capture_turn,
                    layer1,
                    lifecycle_events: Some(lifecycle_events),
                    terminal_audio,
                    last_window_closed: Some(last_window_tx),
                },
            )
            .await;
        }));

        Ok(())
    }

    /// Install the one CoreAudio callback that offers every block to every feed.
    fn install_pcm_fanout_callback(&mut self) {
        let dropped = Arc::clone(&self.dropped_chunks);
        let level_callback = self.level_callback.clone();
        let pcm_feeds = Arc::clone(&self.pcm_feeds);
        self.recorder.set_callback(Box::new(move |data| {
            if let Some(ref level_cb) = level_callback {
                level_cb(block_rms(data));
            }
            let mut feeds = pcm_feeds
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            offer_pcm_to_feeds(&mut feeds, data, &dropped);
        }));
    }

    /// Register a Channel subscriber and its PCM feed.
    ///
    /// Does not start the device. `needs_physical_open` is the W1a rule: the
    /// first subscriber owes `Recorder::start`, a later one joins the stream.
    pub fn register_channel_feed(&mut self) -> ChannelFeedRegistration {
        self.register_channel_feed_archived(None)
    }

    /// [`Self::register_channel_feed`] with the channel's own archive feed
    /// registered in the same registry push.
    fn register_channel_feed_archived(
        &mut self,
        archive: Option<TakeArchiveFeed>,
    ) -> ChannelFeedRegistration {
        let needs_physical_open = !self.recorder.is_active();
        let (sender, receiver) = mpsc::channel::<Vec<f32>>(AUDIO_BACKLOG_CHUNKS);
        let id = self.acquire_archived_capture_subscriber(
            CaptureSubscriberKind::Channel,
            sender,
            archive,
        );
        ChannelFeedRegistration {
            id,
            receiver,
            needs_physical_open,
        }
    }

    /// Open a channel session on the shared capture.
    ///
    /// The channel gets its own ledger (passed in), its own event sink, and
    /// its own `utterance_silence_sec`. That silence arms a separate
    /// `EpochGate::for_session` inside `transcription_session`. The take's
    /// silence field and capture epoch are not written.
    ///
    /// `terminal_audio` carries the channel's own exact archive, as a take's.
    /// Without it the session's terminal dictation witness has no owned PCM
    /// and every gap Apple left stays unrecovered: Maciej's channel-0 take of
    /// 2026-10-16 20:43 had 72 % of its speech without a document owner and
    /// `dictation_witness_pcm_unavailable` at stop. The seal now waits on the
    /// channel archive (disk finalization, never the device span) exactly as
    /// a dictation take does.
    pub async fn begin_channel_session(
        &mut self,
        session_id: String,
        runtime_settings: Arc<RuntimeSettingsSnapshot>,
        event_sink: Arc<dyn EventSink>,
        language: Option<String>,
        configured_silence_sec: f32,
        acoustic_ledger: Arc<StdMutex<AcousticLedger>>,
    ) -> Result<CaptureSubscriberId> {
        let utterance_silence_sec = channel_session_silence(configured_silence_sec);
        // The first subscriber owes the physical open (W1a). The device runs
        // before anything is registered, so the channel's exact archive opens
        // at the native rate and joins the registry together with its feed.
        if !self.recorder.is_active() {
            self.install_pcm_fanout_callback();
            self.recorder.start().await?;
            let actual_sample_rate = self.recorder.actual_sample_rate();
            crate::audio::capture_receipt::publish_open_capture_path(
                crate::audio::capture_receipt::CapturePathMeta::from_open_path(
                    actual_sample_rate,
                    self.recorder.last_native_channels(),
                    self.recorder.last_input_device(),
                ),
            );
            if actual_sample_rate != 0 && actual_sample_rate != self.sample_rate {
                info!(
                    "StreamingRecorder sample_rate updated: config={}Hz -> actual={}Hz",
                    self.sample_rate, actual_sample_rate
                );
                self.sample_rate = actual_sample_rate;
            }
        }

        let actual_sample_rate = {
            let actual = self.recorder.actual_sample_rate();
            if actual == 0 {
                self.sample_rate
            } else {
                actual
            }
        };
        // Scoped to this subscription, like a take's. A channel that cannot
        // open its scratch WAV still transcribes; only its retention is lost,
        // and the log says so.
        let archive = match TakeArchive::spawn(actual_sample_rate, Arc::new(AtomicU64::new(0))) {
            Ok(archive) => Some(archive),
            Err(error) => {
                warn!(%error, "channel admitted without its own PCM archive; its audio is not retained");
                None
            }
        };
        let archive_feed = archive.as_ref().and_then(|archive| archive.feed().ok());
        let registration = self.register_channel_feed_archived(archive_feed);
        let capture_device_name = self.recorder.last_input_device().map(str::to_owned);
        let (layer1, _decision_receipt) = crate::asr_session::layer1_decision(&runtime_settings);
        let (lifecycle, lifecycle_events) = recorder_lifecycle_channel();
        let (last_window_tx, last_window_rx) = oneshot::channel();
        // The channel owes its session the same PCM receipt a take owes its
        // own; a missing archive is reported through the receipt, not by
        // leaving the session without a sender.
        let (terminal_tx, terminal_rx) = std::sync::mpsc::channel();
        let terminal_audio = archive.is_some().then_some(terminal_rx);
        let terminal_audio_sender = terminal_audio.is_some().then_some(terminal_tx);
        event_sink.on_capture_opened(&session_id, 1);
        let sink = event_sink;
        let feed = registration.receiver;
        let id = registration.id;
        let task_session_id = session_id.clone();
        let task = tokio::spawn(async move {
            transcription_session(
                feed,
                sink,
                SessionConfig {
                    session_id: task_session_id,
                    capture_epoch: 1,
                    runtime_settings,
                    live_formatting_agent: None,
                    acoustic_ledger,
                    sample_rate: actual_sample_rate,
                    capture_device_name,
                    language,
                    stream_log_path: None,
                    utterance_silence_sec,
                    capture_turn: CaptureTurnIntent::AgentChannel,
                    layer1,
                    lifecycle_events: Some(lifecycle_events),
                    terminal_audio,
                    last_window_closed: Some(last_window_tx),
                },
            )
            .await;
        });
        self.channel_tasks.push(ChannelTask {
            subscriber: id,
            task,
            lifecycle,
            last_window: last_window_rx,
            archive,
            terminal_audio_sender,
            session_id,
            sample_rate: actual_sample_rate,
        });
        Ok(id)
    }

    /// Release one channel subscription. The last subscriber closes the device.
    ///
    /// A dictation take that is still subscribed keeps the stream. This does
    /// not run the take's stop tail.
    pub async fn end_channel_session(
        &mut self,
        id: CaptureSubscriberId,
    ) -> Result<(bool, Option<std::path::PathBuf>)> {
        self.close_channel_session(id).await.finish().await
    }

    /// Close just this channel's capture interest. Recognition and archive
    /// joins transfer out of the physical recorder's slot before they wait.
    pub async fn close_channel_session(&mut self, id: CaptureSubscriberId) -> ClosedChannel {
        let position = self
            .channel_tasks
            .iter()
            .position(|task| task.subscriber == id);
        let removed = position.map(|index| self.channel_tasks.remove(index));
        let last = self.release_capture_subscriber(id);
        let physical_archive = if last && self.recorder.is_active() {
            self.recorder.close_capture().await;
            self.recorder.detach_closed_spill()
        } else {
            None
        };
        ClosedChannel {
            last,
            task: removed,
            physical_archive,
        }
    }

    /// Stop the session and return the accumulated transcript plus the WAV path.
    ///
    /// Ordered shutdown: release the take's subscription, stop capture when no
    /// subscriber remains, save its exact WAV even when capture remains shared,
    /// await the transcription task, let the presentation layer drain,
    /// then release the sink. Any chunks dropped to backpressure during the
    /// session are logged here — that counter is the signal that audio was
    /// actually lost.
    pub async fn stop(&mut self) -> Result<(String, Option<std::path::PathBuf>)> {
        info!("Stopping streaming recorder...");

        // Report any dropped audio chunks
        let drops = self.dropped_chunks.load(Ordering::Relaxed);
        if drops > 0 {
            warn!(
                "Recording session: dropped {} audio chunk(s) due to backpressure",
                drops
            );
        }

        let was_active = self.close_capture().await;
        self.finish_closed_capture(was_active).await
    }

    /// Close the take's capture interest independently of the session drain
    /// and WAV finalization.
    ///
    /// Releases the take subscription (closing its PCM feed), then closes the
    /// physical stream only when no subscriber remains. The returned flag
    /// belongs to this stop and must be passed to finalization; `false` covers
    /// both "no stream was active" and "the capture stays open for others".
    pub async fn close_capture(&mut self) -> bool {
        let was_active = match self.release_take_pcm_feed() {
            TakeFeedRelease::LastSubscriber | TakeFeedRelease::NoTakeFeed => {
                self.recorder.close_capture().await
            }
            TakeFeedRelease::CaptureShared => false,
        };
        self.closed_capture_was_active = Some(was_active);
        was_active
    }

    /// Ledger mode acknowledges painted Apple/Cloud end finals as before.
    /// Direct mode first hands off real terminal PCM, then waits for its selected
    /// recognizer's drain. Neither mode waits for formatting. The controller
    /// owns its stop deadline; failure closes the channel without a success ack.
    pub async fn wait_live_finals_admitted(&mut self) -> bool {
        if self
            .runtime_settings
            .as_ref()
            .is_some_and(|snapshot| !snapshot.seal_ledger_enabled())
        {
            if self.terminal_take.is_none()
                && let Some(was_active) = self.closed_capture_was_active
            {
                self.terminal_take = Some(self.detach_closed_take(was_active));
            }
            if let Some(take) = self.terminal_take.as_mut() {
                return take.wait_live_finals_admitted().await;
            }
        }
        let Some(receiver) = self.last_window_closed.as_mut() else {
            return false;
        };
        let admitted = receiver.await.is_ok();
        self.last_window_closed = None;
        admitted
    }

    /// Continue the owned stop tail after the microphone has closed.
    pub async fn finish_closed_capture(
        &mut self,
        was_active: bool,
    ) -> Result<(String, Option<std::path::PathBuf>)> {
        if self.terminal_take.is_none() {
            self.terminal_take = Some(self.detach_closed_take(was_active));
        }
        let take = self.terminal_take.as_mut().expect("closed take retained");
        let result = take.finish().await;
        if take.is_settled() {
            self.terminal_take = None;
            self.closed_capture_was_active = None;
        }
        result
    }

    /// Transfer exactly this take's terminal work out of the capture slot.
    /// The physical recorder and all channel interests stay here. The caller
    /// must first close_capture; releasing its feed fences archive PCM/count.
    pub fn detach_closed_take(&mut self, was_active: bool) -> ClosedTake {
        self.closed_capture_was_active = None;
        if let Some(take) = self.terminal_take.take() {
            return take;
        }
        let archive = self.take_archive.take();
        let physical_archive = if was_active {
            self.recorder.detach_closed_spill()
        } else {
            None
        };
        let stopped = self.prepared_capture_archive.take().or_else(|| {
            archive
                .is_none()
                .then(|| self.recorder.finalize_closed_capture(was_active))
        });
        self.detach_take_state(stopped, archive, physical_archive)
    }

    fn detach_take_state(
        &mut self,
        stopped: Option<Result<Option<std::path::PathBuf>>>,
        archive: Option<TakeArchive>,
        physical_archive: Option<ClosedCaptureArchive>,
    ) -> ClosedTake {
        ClosedTake {
            stopped,
            archive,
            physical_archive,
            archive_worker: None,
            task_failure: None,
            transcript_buffer: std::mem::replace(
                &mut self.transcript_buffer,
                Arc::new(Mutex::new(String::new())),
            ),
            transcription_handle: self.transcription_handle.take(),
            event_sink: self.event_sink.take(),
            authority_session_id: self.authority_session_id.clone(),
            acoustic_ledger: self.acoustic_ledger.clone(),
            capture_epoch: self.capture_epoch,
            sample_rate: self.sample_rate,
            seal_lane_armed: self.seal_lane_armed(),
            direct_transcript: self
                .runtime_settings
                .as_ref()
                .is_some_and(|snapshot| !snapshot.seal_ledger_enabled()),
            direct_decoder_complete: None,
            captured_samples: Arc::clone(&self.captured_samples),
            terminal_audio_sender: self.terminal_audio_sender.take(),
            lifecycle_handle: self.lifecycle_handle.take(),
            last_window_closed: self.last_window_closed.take(),
        }
    }

    /// A detached tail may own a complete saved file, never the only failed
    /// PCM in RAM. Freeze finalization without waiting for recognition. Failure
    /// stays queued for the ordinary typed stop/recovery consumer to handle.
    pub fn prepare_closed_capture_archive(&mut self, was_active: bool) -> Result<()> {
        anyhow::ensure!(
            self.take_subscriber.is_none(),
            "take PCM feed is still active"
        );
        if self.prepared_capture_archive.is_none() {
            self.prepared_capture_archive = Some(match self.take_archive.take() {
                Some(archive) => {
                    finalize_take_archive(archive, self.captured_samples.load(Ordering::Relaxed))
                }
                None => self.recorder.finalize_closed_capture(was_active),
            });
        }
        match self.prepared_capture_archive.as_ref() {
            Some(Ok(_)) => Ok(()),
            Some(Err(error)) => Err(anyhow!("closed capture archive unavailable: {error:#}")),
            None => Err(anyhow!("closed capture archive result is missing")),
        }
    }

    /// Inject an archive result into the same owned terminal settlement.
    #[cfg(test)]
    async fn complete_stop(
        &mut self,
        stopped: Result<Option<std::path::PathBuf>>,
    ) -> Result<(String, Option<std::path::PathBuf>)> {
        if self.terminal_take.is_none() {
            self.terminal_take = Some(self.detach_take_state(Some(stopped), None, None));
        } else if let Some(take) = self.terminal_take.as_mut() {
            // Retry keeps the original worker and sink rather than detaching
            // the now-empty live slot a second time.
            if take.archive_worker.is_none() {
                take.stopped = Some(stopped);
            }
        }
        self.finish_closed_capture(false).await
    }

    /// Stop the session and return only the transcript.
    ///
    /// Same ordered shutdown as [`Self::stop`], but the WAV path is dropped.
    /// The file itself is still written by the recorder — this discards the
    /// handle, it does not suppress the write.
    pub async fn stop_and_discard_path(&mut self) -> Result<String> {
        let (transcript, _audio_path) = self.stop().await?;
        Ok(transcript)
    }
}

/// A released channel's existing recognition task and optional device archive.
/// It cannot capture or mutate the controller's current UI/session state.
pub struct ClosedChannel {
    last: bool,
    task: Option<ChannelTask>,
    /// The device spill when this channel closed the device. Auxiliary: it
    /// spans every subscriber that shared the capture and belongs to no session.
    physical_archive: Option<ClosedCaptureArchive>,
}

impl ClosedChannel {
    /// The returned path is the channel's own exact archive, never the
    /// device span, so retention under the channel's session stays true.
    pub async fn finish(mut self) -> Result<(bool, Option<std::path::PathBuf>)> {
        let owned = self.task.as_mut().and_then(|task| task.archive.take());
        let physical = self.physical_archive.take();
        let expected_samples = owned.as_ref().map(TakeArchive::captured_samples);
        let archive = if owned.is_some() || physical.is_some() {
            tokio::task::spawn_blocking(move || {
                let owned = match owned {
                    Some(archive) => {
                        let expected = archive.captured_samples();
                        finalize_take_archive(archive, expected)
                    }
                    None => Ok(None),
                };
                if let Some(physical) = physical {
                    settle_auxiliary_physical_archive(physical, matches!(owned, Ok(Some(_))));
                }
                owned
            })
            .await
            .context("channel archive worker failed")
            .and_then(|result| result)
        } else {
            Ok(None)
        };
        // Hand the finalized archive to the session before joining it: its
        // stop tail waits on this receipt to run the terminal witness. A
        // missing or failed archive is reported, never silently withheld.
        if let Some(sender) = self
            .task
            .as_mut()
            .and_then(|task| task.terminal_audio_sender.take())
        {
            let task = self.task.as_ref().expect("sender was taken from the task");
            let receipt = match (&archive, expected_samples) {
                (Ok(Some(path)), Some(sample_count)) => Ok(
                    crate::pipeline::streaming::live_audio_buffer::FinalizedPcmArchive {
                        session_id: task.session_id.clone(),
                        capture_epoch: 1,
                        sample_rate: task.sample_rate,
                        sample_count,
                        path: path.clone(),
                    },
                ),
                (Ok(_), _) => Err("channel capture finalized without a WAV archive".into()),
                (Err(error), _) => Err(format!("channel archive finalization failed: {error}")),
            };
            let _ = sender.send(receipt);
        }
        // Archive failure still owes the original task's complete drain.
        let task = match self.task.take() {
            Some(removed) => {
                let joined = removed
                    .task
                    .await
                    .context("channel transcription task failed");
                drop(removed.lifecycle);
                drop(removed.last_window);
                joined
            }
            None => Ok(()),
        };
        match (archive, task) {
            (Ok(path), Ok(())) => Ok((self.last, path)),
            (archive, task) => {
                let (audio_path, cause, task_failure) = match archive {
                    Ok(path) => (path, task.expect_err("task failure selected"), None),
                    Err(error) => (None, error, task.err()),
                };
                Err(anyhow::Error::new(CaptureStopFailure {
                    session_id: None,
                    capture_epoch: 0,
                    audio_path,
                    cause,
                    task_failure,
                }))
            }
        }
    }
}

/// The existing take's detached stop tail, never a second capture owner.
/// Its ledger and sink are the original authority, moved with the worker.
pub struct ClosedTake {
    stopped: Option<Result<Option<std::path::PathBuf>>>,
    archive: Option<TakeArchive>,
    physical_archive: Option<ClosedCaptureArchive>,
    archive_worker: Option<JoinHandle<Result<Option<std::path::PathBuf>>>>,
    task_failure: Option<anyhow::Error>,
    transcript_buffer: Arc<Mutex<String>>,
    transcription_handle: Option<JoinHandle<()>>,
    event_sink: Option<Arc<dyn EventSink>>,
    authority_session_id: Option<String>,
    acoustic_ledger: Option<Arc<StdMutex<AcousticLedger>>>,
    capture_epoch: u64,
    sample_rate: u32,
    seal_lane_armed: bool,
    /// Projected once from this take's original immutable settings snapshot.
    direct_transcript: bool,
    /// Actual native completion receipt, retained across canceled stop waiters.
    direct_decoder_complete: Option<bool>,
    captured_samples: Arc<AtomicU64>,
    terminal_audio_sender: Option<
        std::sync::mpsc::Sender<
            Result<crate::pipeline::streaming::live_audio_buffer::FinalizedPcmArchive, String>,
        >,
    >,
    lifecycle_handle: Option<RecorderLifecycleHandle>,
    last_window_closed: Option<oneshot::Receiver<()>>,
}

impl ClosedTake {
    /// True only after archive, producer and publication ownership all settled.
    pub fn is_settled(&self) -> bool {
        self.archive.is_none()
            && self.physical_archive.is_none()
            && self.archive_worker.is_none()
            && self.transcription_handle.is_none()
            && self.event_sink.is_none()
    }

    pub fn capture_identity(&self) -> (Option<&str>, u64) {
        (self.authority_session_id.as_deref(), self.capture_epoch)
    }

    pub fn capture_ledger_handle(&self) -> Option<Arc<StdMutex<AcousticLedger>>> {
        self.acoustic_ledger.as_ref().map(Arc::clone)
    }

    pub fn acoustic_ledger_handle(&self) -> Option<Arc<StdMutex<AcousticLedger>>> {
        self.capture_ledger_handle()
    }

    pub fn seal_lane_armed(&self) -> bool {
        self.seal_lane_armed
    }

    pub async fn wait_live_finals_admitted(&mut self) -> bool {
        if self.direct_transcript
            && let Some(complete) = self.direct_decoder_complete
        {
            return complete;
        }
        if self.direct_transcript {
            self.prepare_terminal_audio().await;
        }
        let Some(receiver) = self.last_window_closed.as_mut() else {
            return false;
        };
        let admitted = receiver.await.is_ok();
        self.last_window_closed = None;
        if self.direct_transcript {
            self.direct_decoder_complete = Some(admitted);
        }
        admitted
    }

    /// Publish the real archive before direct decoder acknowledgement without
    /// joining its producer. Retained disk work makes cancellation retry-safe.
    async fn prepare_terminal_audio(&mut self) {
        if self.archive_worker.is_none()
            && (self.archive.is_some() || self.physical_archive.is_some())
        {
            let archive = self.archive.take();
            let physical_archive = self.physical_archive.take();
            let expected_samples = self.captured_samples.load(Ordering::Relaxed);
            let supplied = self.stopped.take();
            // Store before the first await: cancellation drops only the borrow,
            // never the disk worker or its eventual typed recovery evidence.
            self.archive_worker = Some(tokio::task::spawn_blocking(move || {
                let stopped = match archive {
                    Some(archive) => finalize_take_archive(archive, expected_samples),
                    None => supplied.unwrap_or(Ok(None)),
                };
                if let Some(archive) = physical_archive {
                    // This archive spans the physical device's lifetime. It
                    // must never be retained under the narrower take identity:
                    // beside an exact take WAV it is a duplicate and goes,
                    // without one it is the last complete evidence and stays.
                    let exact_take = matches!(stopped, Ok(Some(_)));
                    match archive.finalize() {
                        Ok((path, _)) if exact_take => discard_auxiliary_archive(&path),
                        Ok((path, samples)) => warn!(
                            path = %path.display(),
                            samples,
                            "auxiliary physical archive kept: no exact take WAV"
                        ),
                        Err(error) if exact_take => warn!(
                            %error,
                            "auxiliary physical archive failed beside an exact take WAV; its source stays"
                        ),
                        Err(error) => match error.recover_complete_archive() {
                            Ok(recovered) => {
                                error.acknowledge_recovery(&recovered)?;
                                warn!(source = ?error.source_path, recovered = %recovered.path().display(),
                                    "auxiliary physical archive recovered separately from owned take");
                            }
                            Err(recovery) => {
                                let audio_path = stopped.as_ref().ok().and_then(Clone::clone);
                                let cause = stopped.err().unwrap_or_else(|| anyhow!(
                                    "auxiliary physical archive recovery failed: {recovery:#}; exact take WAV remains separate"
                                ));
                                return Err(anyhow::Error::new(CaptureStopFailure {
                                    session_id: None,
                                    capture_epoch: 0,
                                    audio_path,
                                    cause,
                                    task_failure: Some(anyhow::Error::new(error)),
                                }));
                            }
                        },
                    }
                }
                stopped
            }));
        }
        if let Some(worker) = self.archive_worker.as_mut() {
            self.stopped = Some(match worker.await {
                Ok(stopped) => stopped,
                Err(error) => Err(anyhow!("take archive worker failed: {error}")),
            });
            self.archive_worker = None;
        }
        let stopped = self.stopped.get_or_insert(Ok(None));
        if let Some(sender) = self.terminal_audio_sender.take() {
            let owned_path = stopped
                .as_ref()
                .ok()
                .and_then(|path| path.as_ref())
                .or_else(|| {
                    stopped
                        .as_ref()
                        .err()
                        .and_then(|error| error.downcast_ref::<CaptureStopFailure>())
                        .and_then(|failure| failure.audio_path.as_ref())
                });
            let receipt = match (owned_path, &*stopped) {
                (Some(path), _) => Ok(
                    crate::pipeline::streaming::live_audio_buffer::FinalizedPcmArchive {
                        session_id: self.authority_session_id.clone().unwrap_or_default(),
                        capture_epoch: self.capture_epoch,
                        sample_rate: self.sample_rate,
                        sample_count: self.captured_samples.load(Ordering::Relaxed),
                        path: path.clone(),
                    },
                ),
                (None, Ok(_)) => Err("capture finalized without a WAV archive".into()),
                (None, Err(error)) => Err(format!("capture archive finalization failed: {error}")),
            };
            let _ = sender.send(receipt);
        }
    }

    /// Save the exact take PCM before terminal refinement and wait for the
    /// original task/reducer. Blocking disk work never holds capture's mutex.
    pub async fn finish(&mut self) -> Result<(String, Option<std::path::PathBuf>)> {
        self.prepare_terminal_audio().await;
        self.lifecycle_handle = None;

        // 2. Wait for worker to finish processing remaining chunks
        // Borrow until joined: cancellation of a caller must not detach the
        // handle. Named controller Stop keeps this future alive across expiry.
        if let Some(handle) = self.transcription_handle.as_mut() {
            debug!("Waiting for transcription session task to finish...");
            self.task_failure = handle
                .await
                .context("Transcription session task failed")
                .err();
            self.transcription_handle = None;
        }
        let direct_decoder_complete =
            self.direct_transcript && self.wait_live_finals_admitted().await;

        // 3. Fence the existing FIFO after the producer has joined. Neither
        // unchanged bytes nor an expired bound proves that publication ended.
        // Keep the sink on failure/cancellation so pending committed text is
        // still owned and recoverable; do not copy a partially published buffer.
        let drain_failure = match self.event_sink.as_ref() {
            Some(sink) => match self.authority_session_id.as_deref() {
                Some(session_id) => tokio::time::timeout(
                    std::time::Duration::from_secs(3),
                    sink.wait_presentation_published(session_id, self.capture_epoch),
                )
                .await
                .unwrap_or_else(|_| Err(anyhow!("presentation terminal drain timed out")))
                .err(),
                None => Some(anyhow!(
                    "presentation terminal drain has no capture identity"
                )),
            },
            None => None,
        };
        if drain_failure.is_none() {
            self.event_sink = None;
        }

        // This lock is also an await. Keep archive and task outcomes in the
        // owner until every cancelable operation has completed.
        let transcript = self.transcript_buffer.lock().await.clone();
        if let Some(drain) = drain_failure {
            // Publication is incomplete, so retain the original sink and
            // outcomes for retry. Typed PCM errors keep their recovery Arcs.
            let stopped = self.stopped.as_ref().expect("archive result retained");
            let task_failure = self.task_failure.as_ref().map(copy_stop_error);
            let (audio_path, cause, secondary) = match stopped {
                Ok(path) => (path.clone(), task_failure, None),
                Err(error) => match error.downcast_ref::<CaptureStopFailure>() {
                    Some(failure) => {
                        let secondary = match (
                            failure.task_failure.as_ref().map(copy_stop_error),
                            task_failure,
                        ) {
                            (Some(archive), Some(task)) => Some(
                                archive
                                    .context(format!("transcription task also failed: {task:#}")),
                            ),
                            (archive, task) => archive.or(task),
                        };
                        (
                            failure.audio_path.clone(),
                            Some(copy_stop_error(&failure.cause)),
                            secondary,
                        )
                    }
                    None => (None, Some(copy_stop_error(error)), task_failure),
                },
            };
            let cause = match cause {
                Some(cause) => cause.context(format!("presentation drain also failed: {drain:#}")),
                None => drain,
            };
            return Err(anyhow::Error::new(CaptureStopFailure {
                session_id: self.authority_session_id.clone(),
                capture_epoch: self.capture_epoch,
                audio_path,
                cause,
                task_failure: secondary,
            }));
        }

        // All awaits are over. Settled outcomes can now leave the owner.
        let stopped = self.stopped.take().expect("archive result retained");
        let task_failure = self.task_failure.take();
        let (audio_path, cause, task_failure) = match stopped {
            Ok(path) => (path, task_failure, None),
            Err(error) => match error.downcast::<CaptureStopFailure>() {
                Ok(failure) => {
                    let secondary = match (failure.task_failure, task_failure) {
                        (Some(archive), Some(task)) => Some(
                            archive.context(format!("transcription task also failed: {task:#}")),
                        ),
                        (archive, task) => archive.or(task),
                    };
                    (failure.audio_path, Some(failure.cause), secondary)
                }
                Err(error) => (None, Some(error), task_failure),
            },
        };
        if let Some(cause) = cause {
            return Err(anyhow::Error::new(CaptureStopFailure {
                session_id: self.authority_session_id.clone(),
                capture_epoch: self.capture_epoch,
                audio_path,
                cause,
                task_failure,
            }));
        }

        // Direct output owes a real producer completion and the publication
        // fence above, never an acoustic word/seal receipt. Do not turn source
        // loss, canceled work or a missing native receipt into a successful take.
        if self.direct_transcript {
            return if direct_decoder_complete {
                Ok((transcript, audio_path))
            } else {
                Err(anyhow::Error::new(CaptureStopFailure {
                    session_id: self.authority_session_id.clone(),
                    capture_epoch: self.capture_epoch,
                    audio_path,
                    cause: anyhow!("direct recognizer completion unavailable or failed"),
                    task_failure: None,
                }))
            };
        }

        // A gesture shorter than one complete speech window can contain PCM
        // while producing no Silero/ledger observation. It is still a take,
        // but cannot owe a terminal transcript receipt. A longer unobserved
        // capture remains a processing refusal.
        let captured_samples = self.captured_samples.load(Ordering::Relaxed);
        let empty_capture = captured_samples <= u64::from(self.sample_rate) * 3 / 10
            && transcript.is_empty()
            && self.acoustic_ledger.as_ref().is_none_or(|ledger| {
                ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .has_no_capture_facts()
            });
        if empty_capture {
            return Ok((transcript, audio_path));
        }
        let finality = self.authority_session_id.as_deref().and_then(|session| {
            self.acoustic_ledger.as_ref().map(|ledger| {
                ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .terminal_finality(session, self.capture_epoch)
            })
        });
        match finality {
            Some(crate::pipeline::acoustic_ledger::TerminalFinality::Sealed(_)) => {
                Ok((transcript, audio_path))
            }
            Some(crate::pipeline::acoustic_ledger::TerminalFinality::ObservedSilence(_))
                if transcript.is_empty() =>
            {
                Ok((transcript, audio_path))
            }
            Some(crate::pipeline::acoustic_ledger::TerminalFinality::Refused(finality)) => {
                Err(anyhow::Error::new(TerminalSealRefused {
                    finality,
                    audio_path,
                    committed_text: transcript,
                }))
            }
            _ => Err(anyhow::Error::new(CaptureStopFailure {
                session_id: self.authority_session_id.clone(),
                capture_epoch: self.capture_epoch,
                audio_path,
                cause: anyhow!("recording terminal authority unavailable or inconsistent"),
                task_failure: None,
            })),
        }
    }
}

/// Duplicate only a diagnostic returned while its stop owner remains pending.
/// Archive evidence shares its original recovery buffers and physical identity.
fn copy_stop_error(error: &anyhow::Error) -> anyhow::Error {
    if let Some(archive) = error.downcast_ref::<crate::audio::recorder::CaptureArchiveError>() {
        return anyhow::Error::new(archive.clone());
    }
    if let Some(failure) = error.downcast_ref::<CaptureStopFailure>() {
        return anyhow::Error::new(CaptureStopFailure {
            session_id: failure.session_id.clone(),
            capture_epoch: failure.capture_epoch,
            audio_path: failure.audio_path.clone(),
            cause: copy_stop_error(&failure.cause),
            task_failure: failure.task_failure.as_ref().map(copy_stop_error),
        });
    }
    anyhow!("{error:#}")
}

/// The device spill is never a session's evidence. Beside an exact owned
/// archive it is a duplicate and goes; without one it is the last complete
/// PCM and stays on disk, named in the log.
fn settle_auxiliary_physical_archive(archive: ClosedCaptureArchive, owned_exact: bool) {
    match archive.finalize() {
        Ok((path, _)) if owned_exact => discard_auxiliary_archive(&path),
        Ok((path, samples)) => warn!(
            path = %path.display(),
            samples,
            "auxiliary physical archive kept: no exact owned archive"
        ),
        Err(error) => warn!(
            %error,
            "auxiliary physical archive failed; its source stays as evidence"
        ),
    }
}

fn discard_auxiliary_archive(path: &std::path::Path) {
    match std::fs::remove_file(path) {
        Ok(()) => debug!(
            path = %path.display(),
            "auxiliary physical archive discarded beside an exact owned archive"
        ),
        Err(error) => warn!(
            %error,
            path = %path.display(),
            "auxiliary physical archive could not be discarded"
        ),
    }
}

fn finalize_take_archive(
    archive: TakeArchive,
    expected_samples: u64,
) -> Result<Option<std::path::PathBuf>> {
    let (path, count) = archive.finalize().map_err(|mut error| {
        // Queue refusal preserves an explicit gap, never a shortened success.
        error.captured_samples = usize::try_from(expected_samples).unwrap_or(usize::MAX);
        anyhow::Error::new(error)
    })?;
    anyhow::ensure!(
        count as u64 == expected_samples,
        "take archive sample count disagrees: captured={expected_samples}, written={count}; source={}",
        path.display(),
    );
    Ok(Some(path))
}

/// Offer one captured block to every subscriber feed.
///
/// Bounded and non-blocking — capture never branches on a consumer (the
/// `RecorderLayer1Lane::offer_pcm` contract): a full feed drops the block and
/// counts it on `dropped`, a closed feed is reaped from the registry.
fn offer_pcm_to_feeds(feeds: &mut Vec<PcmFeed>, data: &[f32], dropped: &AtomicU64) {
    let duck_channels = crate::audio::tts_duck::channel_capture_should_drop();
    let mut full = 0_u64;
    feeds.retain(|feed| {
        if duck_channels && feed.kind == CaptureSubscriberKind::Channel {
            return true;
        }
        if let Some(archive) = &feed.archive {
            archive
                .captured_samples
                .fetch_add(data.len() as u64, Ordering::Relaxed);
            let samples = convert_mono_f32_to_i16(data);
            let refused = if archive.send_failed.load(Ordering::SeqCst) {
                Some(samples)
            } else {
                archive.sender.send(samples).err()
            };
            if let Some(samples) = refused {
                // A full queue latches: the archive stays an ordered written
                // prefix plus this RAM tail, never a WAV with a hole. The tail
                // is kept under the registry lock the callback already holds
                // and merged into the recovery evidence at finalize.
                archive.send_failed.store(true, Ordering::SeqCst);
                archive
                    .refused
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .extend(samples);
            }
        }
        match feed.sender.try_send(data.to_vec()) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                full += 1;
                true
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    });
    if full > 0 {
        let n = dropped.fetch_add(full, Ordering::Relaxed);
        if n == 0 || (n + full).is_multiple_of(50) {
            tracing::warn!(
                "Audio callback: channel full, dropped {} chunk(s)",
                n + full
            );
        }
    }
}

/// RMS of one captured audio block (linear, 0..~1 for full-scale input).
/// Cheap enough for the CoreAudio callback thread (one pass + one sqrt).
fn block_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    // Accumulate in f64 so a malformed/out-of-range f32 block cannot overflow
    // the sum. Non-finite device samples are treated as silence; NaN/Inf must
    // never cross the typed audio-level transport into Swift.
    let sum_sq = samples.iter().fold(0.0_f64, |sum, sample| {
        let sample = if sample.is_finite() {
            f64::from(*sample)
        } else {
            0.0
        };
        sum + sample * sample
    });
    (sum_sq / samples.len() as f64).sqrt() as f32
}

/// Unit and opt-in e2e probes for RMS meters, VAD index sync, and corpus WER.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::chunker::{SpeechEvent, SpeechSession, VadGateMode};
    use crate::audio::load_audio_file;
    use crate::vad;
    use serial_test::serial;
    use std::fs;
    use tokio::time::Duration;

    #[tokio::test]
    #[serial(tts_duck)]
    async fn stop_pause_preserves_five_pcm_blocks_until_subscriber_eof() {
        crate::audio::tts_duck::clear();
        let mut recorder = StreamingRecorder::new().expect("recorder");
        let mut channel = recorder.register_channel_feed();
        let (take_tx, mut take_rx) = mpsc::channel(8);
        let take = recorder.acquire_capture_subscriber(CaptureSubscriberKind::Take, take_tx);
        recorder.recorder.set_active_for_test(true);
        let feeds = Arc::clone(&recorder.pcm_feeds);
        let dropped = Arc::clone(&recorder.dropped_chunks);
        let producer = tokio::spawn(async move {
            for occurrence in 1..=5 {
                tokio::time::sleep(Duration::from_millis(200)).await;
                let block = vec![occurrence as f32 / 10.0; 16];
                offer_pcm_to_feeds(&mut feeds.lock().expect("feeds"), &block, &dropped);
            }
        });
        let gesture = Instant::now();
        recorder.await_stop_pause(Some(channel.id), gesture).await;
        assert!(gesture.elapsed() >= STOP_CAPTURE_GRACE);
        producer.await.expect("producer");
        for occurrence in 1..=5 {
            let expected = vec![occurrence as f32 / 10.0; 16];
            assert_eq!(channel.receiver.try_recv().expect("channel PCM"), expected);
            assert_eq!(take_rx.try_recv().expect("take PCM"), expected);
        }
        assert!(matches!(
            channel.receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        assert_eq!(recorder.dropped_chunks.load(Ordering::Relaxed), 0);
        tokio::time::timeout(
            Duration::from_millis(200),
            recorder.await_stop_pause(Some(channel.id), gesture),
        )
        .await
        .expect("repeated Stop keeps original deadline");
        assert!(!recorder.release_capture_subscriber(channel.id));
        assert!(
            channel.receiver.recv().await.is_none(),
            "EOF only after release"
        );
        offer_pcm_to_feeds(
            &mut recorder.pcm_feeds.lock().expect("feeds"),
            &[0.7],
            &recorder.dropped_chunks,
        );
        assert_eq!(
            take_rx.try_recv().expect("other subscriber remains live"),
            vec![0.7]
        );
        assert!(recorder.release_capture_subscriber(take));
        assert!(take_rx.recv().await.is_none());
        recorder.recorder.set_active_for_test(false);
    }

    #[tokio::test]
    async fn stop_pause_skips_inactive_missing_released_and_expired_capture() {
        let mut recorder = StreamingRecorder::new().expect("recorder");
        tokio::time::timeout(
            Duration::from_millis(200),
            recorder.await_stop_pause(None, Instant::now()),
        )
        .await
        .expect("no take");
        let channel = recorder.register_channel_feed();
        tokio::time::timeout(
            Duration::from_millis(200),
            recorder.await_stop_pause(Some(channel.id), Instant::now()),
        )
        .await
        .expect("inactive capture");
        recorder.recorder.set_active_for_test(true);
        tokio::time::timeout(
            Duration::from_millis(200),
            recorder.await_stop_pause(Some(channel.id), Instant::now() - Duration::from_secs(2)),
        )
        .await
        .expect("elapsed gesture");
        recorder.release_capture_subscriber(channel.id);
        tokio::time::timeout(
            Duration::from_millis(200),
            recorder.await_stop_pause(Some(channel.id), Instant::now()),
        )
        .await
        .expect("released subscriber");
        recorder.recorder.set_active_for_test(false);
    }

    #[tokio::test]
    async fn stop_pause_cancelled_waiter_keeps_feed_and_original_deadline() {
        let mut recorder = StreamingRecorder::new().expect("recorder");
        let mut channel = recorder.register_channel_feed();
        recorder.recorder.set_active_for_test(true);
        let gesture = Instant::now();
        assert!(
            tokio::time::timeout(
                Duration::from_millis(30),
                recorder.await_stop_pause(Some(channel.id), gesture)
            )
            .await
            .is_err()
        );
        assert_eq!(recorder.capture_subscriber_count(), 1);
        assert!(matches!(
            channel.receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        recorder.await_stop_pause(Some(channel.id), gesture).await;
        assert!(gesture.elapsed() >= STOP_CAPTURE_GRACE);
        recorder.release_capture_subscriber(channel.id);
        assert!(channel.receiver.recv().await.is_none());
        recorder.recorder.set_active_for_test(false);
    }

    #[test]
    fn channel_silence_is_not_read_from_the_take_field() {
        let mut recorder = StreamingRecorder::new().expect("recorder");
        recorder.set_utterance_silence_sec(None);
        assert_eq!(channel_session_silence(1.25), Some(1.25));
        assert_eq!(recorder.utterance_silence_sec, None);
        recorder.set_utterance_silence_sec(Some(9.0));
        assert_eq!(channel_session_silence(0.4), Some(0.4));
        assert_eq!(recorder.utterance_silence_sec, Some(9.0));
    }

    #[test]
    #[serial(tts_duck)]
    fn channel_feed_owes_a_physical_open_and_shares_blocks_with_a_take() {
        crate::audio::tts_duck::clear();
        let mut recorder = StreamingRecorder::new().expect("recorder");
        let mut channel = recorder.register_channel_feed();
        assert!(channel.needs_physical_open);
        assert!(recorder.has_non_take_subscriber());
        assert!(!recorder.has_take_subscriber());
        assert_eq!(recorder.capture_subscriber_count(), 1);

        let (take_tx, mut take_rx) = mpsc::channel(4);
        recorder.acquire_capture_subscriber(CaptureSubscriberKind::Take, take_tx);
        let block = vec![0.2_f32, -0.2, 0.2];
        {
            let mut feeds = recorder.pcm_feeds.lock().expect("feeds");
            offer_pcm_to_feeds(&mut feeds, &block, &recorder.dropped_chunks);
        }
        assert_eq!(channel.receiver.try_recv().expect("channel block"), block);
        assert_eq!(take_rx.try_recv().expect("take block"), block);

        assert!(!recorder.release_capture_subscriber(channel.id));
        assert!(recorder.has_take_subscriber());
        assert!(!recorder.has_non_take_subscriber());
        assert_eq!(recorder.capture_subscriber_count(), 1);
    }

    #[test]
    #[serial(tts_duck)]
    fn channel_pcm_is_held_while_agent_speech_owns_the_speaker() {
        crate::audio::tts_duck::clear();
        let mut recorder = StreamingRecorder::new().expect("recorder");
        let mut channel = recorder.register_channel_feed();
        let (take_tx, mut take_rx) = mpsc::channel(4);
        recorder.acquire_capture_subscriber(CaptureSubscriberKind::Take, take_tx);
        let block = vec![0.25_f32, -0.25];
        crate::audio::tts_duck::arm_for(Duration::from_secs(30));
        {
            let mut feeds = recorder.pcm_feeds.lock().expect("feeds");
            offer_pcm_to_feeds(&mut feeds, &block, &recorder.dropped_chunks);
        }
        assert!(
            channel.receiver.try_recv().is_err(),
            "channel capture stays quiet while speech plays"
        );
        assert_eq!(
            take_rx.try_recv().expect("take still hears the room"),
            block
        );
        crate::audio::tts_duck::clear();
        {
            let mut feeds = recorder.pcm_feeds.lock().expect("feeds");
            offer_pcm_to_feeds(&mut feeds, &block, &recorder.dropped_chunks);
        }
        assert_eq!(
            channel
                .receiver
                .try_recv()
                .expect("channel hears after speech"),
            block
        );
        assert_eq!(take_rx.try_recv().expect("take still hears"), block);
    }

    struct TransportOnlyAgent;

    #[async_trait::async_trait]
    impl crate::ai_formatting::FormattingAgent for TransportOnlyAgent {
        async fn execute(
            &self,
            _turn_id: &str,
            _text: &str,
            _settings: &RuntimeSettingsSnapshot,
        ) -> Result<String> {
            panic!("transport must not execute an instruction");
        }
    }

    #[test]
    fn live_max_transport_preserves_owner_and_excludes_other_policies() {
        use crate::config::FormattingPolicy;
        let agent: Arc<dyn crate::ai_formatting::FormattingAgent> = Arc::new(TransportOnlyAgent);
        for intent in [CaptureTurnIntent::HandsFree, CaptureTurnIntent::SingleTurn] {
            for enabled in [false, true] {
                for policy in [
                    FormattingPolicy::Off,
                    FormattingPolicy::Correction,
                    FormattingPolicy::Smart,
                    FormattingPolicy::Max,
                ] {
                    let selected = live_max_capability(intent, enabled, policy, Some(&agent));
                    let admitted = intent == CaptureTurnIntent::HandsFree
                        && enabled
                        && policy == FormattingPolicy::Max;
                    assert_eq!(selected.is_some(), admitted);
                    if let Some(selected) = selected {
                        assert!(Arc::ptr_eq(&selected, &agent));
                    }
                }
            }
        }
        assert!(
            live_max_capability(
                CaptureTurnIntent::HandsFree,
                true,
                FormattingPolicy::Max,
                None,
            )
            .is_none()
        );
    }

    /// Empty/silence/full-scale blocks map to the 0 / 0 / ~1 energy ladder meters use.
    #[test]
    fn block_rms_measures_signal_energy() {
        assert_eq!(block_rms(&[]), 0.0, "empty block must read as silence");
        assert_eq!(block_rms(&[0.0; 512]), 0.0, "digital silence is 0 RMS");
        let full_scale = block_rms(&[1.0, -1.0, 1.0, -1.0]);
        assert!(
            (full_scale - 1.0).abs() < 1e-6,
            "full-scale square wave must read ~1.0, got {full_scale}"
        );
        let half = block_rms(&[0.5, -0.5, 0.5, -0.5]);
        assert!(
            (half - 0.5).abs() < 1e-6,
            "half-scale square wave must read ~0.5, got {half}"
        );
    }

    /// Quiet < loud stays finite; NaN/Inf capture samples must not poison level transport.
    #[test]
    fn block_rms_orders_quiet_and_loud_finite_levels() {
        let silence = block_rms(&[0.0; 512]);
        let quiet = block_rms(&[0.01, -0.01, 0.01, -0.01]);
        let loud = block_rms(&[0.8, -0.8, 0.8, -0.8]);

        assert!(silence.is_finite() && quiet.is_finite() && loud.is_finite());
        assert!(
            silence < quiet && quiet < loud,
            "expected monotonic energy, got silence={silence}, quiet={quiet}, loud={loud}"
        );
        assert_eq!(
            block_rms(&[f32::NAN, f32::INFINITY, f32::NEG_INFINITY]),
            0.0,
            "non-finite capture samples must not poison the meter transport"
        );
    }

    /// Delivery probe for the selected real input. During the nine-second run,
    /// keep 0-3s silent, speak quietly during 3-6s, then loudly during 6-9s.
    /// The test is ignored by default because it requires TCC microphone access
    /// and a human-marked acoustic sequence.
    #[tokio::test]
    #[ignore = "requires selected microphone + TCC and silence/quiet/loud operator input"]
    async fn real_input_rms_probe() {
        if !env_bool("CODESCRIBE_E2E_MIC") {
            eprintln!("Skipping real RMS probe (set CODESCRIBE_E2E_MIC=1 to enable)");
            return;
        }

        let started = std::time::Instant::now();
        let (level_tx, level_rx) = std::sync::mpsc::sync_channel::<(f32, f32)>(1024);
        let mut recorder = Recorder::new().expect("Failed to initialize selected microphone");
        recorder.set_callback(Box::new(move |samples| {
            let _ = level_tx.try_send((started.elapsed().as_secs_f32(), block_rms(samples)));
        }));

        eprintln!("RMS probe: 0-3s SILENCE, 3-6s QUIET SPEECH, 6-9s LOUD SPEECH");
        recorder
            .start()
            .await
            .expect("Failed to start selected microphone");
        tokio::time::sleep(Duration::from_secs(9)).await;
        let audio_path = recorder
            .stop()
            .await
            .expect("Failed to stop selected microphone");
        if let Some(path) = audio_path {
            let _ = std::fs::remove_file(path);
        }

        let mut windows = [Vec::<f32>::new(), Vec::<f32>::new(), Vec::<f32>::new()];
        for (elapsed, rms) in level_rx.try_iter() {
            assert!(rms.is_finite(), "real input emitted non-finite RMS: {rms}");
            let index = (elapsed / 3.0).floor() as usize;
            if let Some(window) = windows.get_mut(index) {
                window.push(rms);
            }
        }

        let means = windows.map(|window| {
            assert!(
                !window.is_empty(),
                "real input probe window captured no blocks"
            );
            window.iter().copied().sum::<f32>() / window.len() as f32
        });
        eprintln!(
            "RMS probe means: silence={:.6}, quiet={:.6}, loud={:.6}",
            means[0], means[1], means[2]
        );
        assert!(
            means[0] < means[1] && means[1] < means[2],
            "selected input did not produce ordered silence/quiet/loud energy: {means:?}"
        );
    }

    /// Five-minute delivery probe for capture/backpressure stability. This runs
    /// the production `StreamingRecorder` path against the selected input and
    /// reports callback, engine-event, and dropped-chunk counters. It is opt-in
    /// because it needs TCC microphone access and intentionally holds the real
    /// audio device for the full acceptance interval.
    #[tokio::test]
    #[ignore = "requires selected microphone + TCC and a five-minute foreground run"]
    async fn sustained_real_input_pressure_probe() {
        if !env_bool("CODESCRIBE_E2E_MIC") {
            eprintln!("Skipping sustained mic probe (set CODESCRIBE_E2E_MIC=1 to enable)");
            return;
        }

        let duration_sec = env_f32("CODESCRIBE_E2E_SUSTAIN_SEC", 300.0).max(300.0);
        let level_blocks = Arc::new(AtomicU64::new(0));
        let non_finite_levels = Arc::new(AtomicU64::new(0));
        let level_blocks_for_callback = Arc::clone(&level_blocks);
        let non_finite_for_callback = Arc::clone(&non_finite_levels);
        let sink = Arc::new(crate::pipeline::sinks::CollectorEventSink::new());
        let mut recorder = StreamingRecorder::new().expect("Failed to initialize selected input");
        recorder.set_level_callback(Some(Arc::new(move |rms| {
            level_blocks_for_callback.fetch_add(1, Ordering::Relaxed);
            if !rms.is_finite() {
                non_finite_for_callback.fetch_add(1, Ordering::Relaxed);
            }
        })));
        recorder.set_event_sink(Some(sink.clone()));

        eprintln!("Sustained mic probe: recording selected input for {duration_sec:.0}s");
        recorder
            .start_event_session(None)
            .await
            .expect("Failed to start streaming recorder");
        tokio::time::sleep(Duration::from_secs_f32(duration_sec)).await;
        let (_transcript, audio_path) = recorder
            .stop()
            .await
            .expect("Failed to stop streaming recorder");
        if let Some(path) = audio_path {
            let _ = std::fs::remove_file(path);
        }

        let levels = level_blocks.load(Ordering::Relaxed);
        let invalid = non_finite_levels.load(Ordering::Relaxed);
        let drops = recorder.dropped_chunks.load(Ordering::Relaxed);
        let events = sink.events().len();
        eprintln!(
            "Sustained mic counters: level_blocks={levels}, non_finite={invalid}, dropped_chunks={drops}, engine_events={events}"
        );
        assert!(levels > 0, "selected input produced no capture callbacks");
        assert_eq!(invalid, 0, "real input emitted non-finite RMS levels");
        assert_eq!(drops, 0, "sustained recording dropped audio chunks");
    }

    /// `start_event_session` must refuse when no event sink was configured.
    #[tokio::test]
    async fn start_event_session_requires_event_sink() {
        let mut recorder = StreamingRecorder::new().expect("Failed to create recorder");
        let err = recorder
            .start_event_session(Some("en".to_string()))
            .await
            .expect_err("start_event_session should fail when event sink is missing");
        assert!(
            err.to_string().contains("requires event_sink"),
            "unexpected error: {err:?}"
        );
    }

    /// Capture lifetime is refcounted over subscribers, never take-identical:
    /// the capture closes only when the last subscriber releases, and a take
    /// after the close binds a fresh ledger and epoch like any first take.
    ///
    /// `#[serial]`: the runtime-snapshot load resolves the process-global
    /// `CODESCRIBE_DATA_DIR`; off the serial lane it can consume or rewrite a
    /// serial settings test's private fixture (X-hermetic-test-config).
    #[tokio::test]
    #[serial]
    async fn capture_lifecycle_is_refcounted_over_subscribers() {
        let mut recorder = StreamingRecorder::new().expect("Failed to create recorder");
        let (channel_tx, mut channel_rx) = mpsc::channel::<Vec<f32>>(8);
        let (take_tx, mut take_rx) = mpsc::channel::<Vec<f32>>(8);

        assert_eq!(recorder.capture_subscriber_count(), 0);
        assert!(!recorder.has_capture_subscribers());

        // The agent channel (non-take) subscribes first.
        let channel =
            recorder.acquire_capture_subscriber(CaptureSubscriberKind::Channel, channel_tx);
        assert_eq!(recorder.capture_subscriber_count(), 1);
        assert!(recorder.has_non_take_subscriber());
        assert!(!recorder.has_take_subscriber());

        // A dictation take joins as the second subscriber.
        let take = recorder.acquire_capture_subscriber(CaptureSubscriberKind::Take, take_tx);
        assert_eq!(recorder.capture_subscriber_count(), 2);
        assert!(recorder.has_take_subscriber());

        // The take leaving while the channel stays must NOT close the capture.
        assert!(
            !recorder.release_capture_subscriber(take),
            "capture must stay open while the channel subscribes"
        );
        assert_eq!(recorder.capture_subscriber_count(), 1);
        // Releasing a subscription drops its feed: the take's session observes
        // end-of-capture, the untouched channel feed stays open.
        assert!(matches!(
            take_rx.try_recv(),
            Err(mpsc::error::TryRecvError::Disconnected)
        ));
        assert!(matches!(
            channel_rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));

        // The last subscriber releasing owes the physical close.
        assert!(
            recorder.release_capture_subscriber(channel),
            "last subscriber release must report capture close"
        );
        assert_eq!(recorder.capture_subscriber_count(), 0);

        // A take after the close binds a fresh ledger and restarts the epoch,
        // exactly the pre-refcount first-take contract.
        let runtime_settings = Arc::new(
            crate::config::Config::load_runtime_snapshot_without_keychain()
                .expect("keychain-free runtime snapshot"),
        );
        let ledger_a =
            recorder.bind_session_authority("take-a".to_string(), Arc::clone(&runtime_settings));
        assert_eq!(recorder.capture_identity(), (Some("take-a"), 0));
        let ledger_b = recorder.bind_session_authority("take-b".to_string(), runtime_settings);
        assert_eq!(recorder.capture_identity(), (Some("take-b"), 0));
        assert!(
            !Arc::ptr_eq(&ledger_a, &ledger_b),
            "every take binds its own ledger, never the capture's"
        );
    }

    #[tokio::test]
    #[serial]
    #[serial(tts_duck)]
    async fn shared_take_archive_is_exact_and_tail_leaves_channel_feed_available() {
        crate::audio::tts_duck::clear();
        let mut recorder = StreamingRecorder::new().unwrap();
        recorder.sample_rate = 16_000;
        let (channel_tx, mut channel_rx) = mpsc::channel(8);
        let channel =
            recorder.acquire_capture_subscriber(CaptureSubscriberKind::Channel, channel_tx);
        offer_pcm_to_feeds(
            &mut recorder.pcm_feeds.lock().unwrap(),
            &[0.1; 40],
            &recorder.dropped_chunks,
        );
        recorder.captured_samples = Arc::new(AtomicU64::new(0));
        let archive = TakeArchive::spawn(16_000, Arc::clone(&recorder.captured_samples)).unwrap();
        let (take_tx, mut take_rx) = mpsc::channel(8);
        let take_id = recorder.acquire_capture_subscriber(CaptureSubscriberKind::Take, take_tx);
        recorder.take_subscriber = Some(take_id);
        recorder
            .pcm_feeds
            .lock()
            .unwrap()
            .iter_mut()
            .find(|feed| feed.id == take_id)
            .unwrap()
            .archive = Some(archive.feed().unwrap());
        recorder.take_archive = Some(archive);
        offer_pcm_to_feeds(
            &mut recorder.pcm_feeds.lock().unwrap(),
            &[0.25; 160],
            &recorder.dropped_chunks,
        );
        assert_eq!(take_rx.recv().await.unwrap(), vec![0.25; 160]);
        let was_active = recorder.close_capture().await;
        assert!(
            !was_active,
            "remaining channel owns physical capture interest"
        );
        assert_eq!(recorder.capture_subscriber_count(), 1);
        let release = Arc::new(tokio::sync::Notify::new());
        let worker_release = Arc::clone(&release);
        recorder.transcription_handle = Some(tokio::spawn(async move {
            worker_release.notified().await;
        }));
        let closed = recorder.detach_closed_take(was_active);
        let count = Arc::clone(&closed.captured_samples);
        let (terminal_tx, terminal_rx) = std::sync::mpsc::channel();
        let mut closed = closed;
        closed.terminal_audio_sender = Some(terminal_tx);
        let tail = tokio::spawn(async move { closed.finish().await });
        offer_pcm_to_feeds(
            &mut recorder.pcm_feeds.lock().unwrap(),
            &[0.8; 320],
            &recorder.dropped_chunks,
        );
        assert_eq!(
            count.load(Ordering::Relaxed),
            160,
            "channel continuation cannot extend take archive"
        );
        // Replacing the next take's slots must not overwrite the detached owner.
        recorder.captured_samples = Arc::new(AtomicU64::new(900));
        *recorder.transcript_buffer.lock().await = "next take".into();
        let receipt =
            tokio::task::spawn_blocking(move || terminal_rx.recv_timeout(Duration::from_secs(2)))
                .await
                .unwrap()
                .unwrap()
                .unwrap();
        assert_eq!(receipt.sample_count, 160);
        assert!(!tail.is_finished(), "terminal worker still owns old take");
        let mut wav = hound::WavReader::open(&receipt.path).unwrap();
        assert_eq!(wav.spec().sample_rate, 16_000);
        let pcm: Vec<i16> = wav.samples().map(Result::unwrap).collect();
        assert_eq!(pcm, convert_mono_f32_to_i16(&[0.25; 160]));
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), channel_rx.recv())
                .await
                .unwrap()
                .unwrap(),
            vec![0.1; 40]
        );
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), channel_rx.recv())
                .await
                .unwrap()
                .unwrap(),
            vec![0.25; 160]
        );
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), channel_rx.recv())
                .await
                .unwrap()
                .unwrap(),
            vec![0.8; 320]
        );
        release.notify_one();
        let (text, path) = tail.await.unwrap().unwrap();
        assert!(
            text.is_empty(),
            "new take text never enters old terminal result"
        );
        assert_eq!(path.as_ref(), Some(&receipt.path));
        assert!(recorder.release_capture_subscriber(channel));
        std::fs::remove_file(path.unwrap()).unwrap();
    }

    #[tokio::test]
    #[serial]
    #[serial(tts_duck)]
    async fn auxiliary_physical_recovery_never_replaces_exact_take_pcm() {
        crate::audio::tts_duck::clear();
        let exact = crate::audio::recorder::spill_take_wav_for_tests(&[700; 160], 16_000).unwrap();
        let physical =
            crate::audio::recorder::failed_physical_archive_fixture(&[2100; 800], 16_000);
        let takes = crate::audio::recorder::takes_dir().unwrap();
        let take_files = |dir: &std::path::Path| -> std::collections::BTreeSet<std::path::PathBuf> {
            std::fs::read_dir(dir)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect()
        };
        let before = take_files(&takes);
        let mut recorder = StreamingRecorder::new().unwrap();
        recorder.sample_rate = 16_000;
        recorder.captured_samples.store(160, Ordering::Relaxed);
        let mut closed =
            recorder.detach_take_state(Some(Ok(Some(exact.clone()))), None, Some(physical));
        let (_, path) = closed.finish().await.unwrap();
        assert_eq!(path.as_ref(), Some(&exact));
        assert_eq!(
            take_files(&takes),
            before,
            "no recovery WAV is minted beside an exact take WAV; the failed source stays"
        );
        let mut wav = hound::WavReader::open(&exact).unwrap();
        let pcm: Vec<i16> = wav.samples().map(Result::unwrap).collect();
        assert_eq!(
            pcm,
            vec![700; 160],
            "broader physical PCM cannot become take evidence"
        );
        std::fs::remove_file(exact).unwrap();
    }

    #[tokio::test]
    #[serial]
    #[serial(tts_duck)]
    async fn channel_close_releases_feed_before_slow_worker_and_keeps_take_available() {
        crate::audio::tts_duck::clear();
        let mut recorder = StreamingRecorder::new().unwrap();
        let (channel_tx, mut channel_rx) = mpsc::channel(8);
        let channel =
            recorder.acquire_capture_subscriber(CaptureSubscriberKind::Channel, channel_tx);
        let (take_tx, mut take_rx) = mpsc::channel(8);
        let take = recorder.acquire_capture_subscriber(CaptureSubscriberKind::Take, take_tx);
        let release = Arc::new(tokio::sync::Notify::new());
        let worker_release = Arc::clone(&release);
        let (lifecycle, _) = recorder_lifecycle_channel();
        let (_, last_window) = oneshot::channel();
        recorder.channel_tasks.push(ChannelTask {
            subscriber: channel,
            task: tokio::spawn(async move {
                worker_release.notified().await;
            }),
            lifecycle,
            last_window,
            archive: None,
            terminal_audio_sender: None,
            session_id: String::new(),
            sample_rate: 16_000,
        });
        let closed = recorder.close_channel_session(channel).await;
        assert_eq!(recorder.capture_subscriber_count(), 1);
        assert!(recorder.channel_tasks.is_empty());
        assert!(matches!(
            channel_rx.try_recv(),
            Err(mpsc::error::TryRecvError::Disconnected)
        ));
        let tail = tokio::spawn(async move { closed.finish().await });
        offer_pcm_to_feeds(
            &mut recorder.pcm_feeds.lock().unwrap(),
            &[0.3; 40],
            &recorder.dropped_chunks,
        );
        assert_eq!(take_rx.recv().await.unwrap(), vec![0.3; 40]);
        assert!(
            !tail.is_finished(),
            "hangup admission does not wait for channel STT"
        );
        release.notify_one();
        let (last, path) = tail.await.unwrap().unwrap();
        assert!(!last);
        assert!(
            path.is_none(),
            "shared channel cannot claim a whole-device WAV"
        );
        assert!(recorder.release_capture_subscriber(take));
    }

    #[test]
    #[serial(tts_duck)]
    fn take_archive_backpressure_refuses_without_blocking_other_subscribers() {
        crate::audio::tts_duck::clear();
        let (archive_tx, _undrained_archive) = std::sync::mpsc::sync_channel(1);
        let (take_tx, mut take_rx) = mpsc::channel(8);
        let (channel_tx, mut channel_rx) = mpsc::channel(8);
        let samples = Arc::new(AtomicU64::new(0));
        let refused = Arc::new(AtomicBool::new(false));
        let refused_pcm = Arc::new(StdMutex::new(Vec::new()));
        let mut feeds = vec![
            PcmFeed {
                id: CaptureSubscriberId(1),
                kind: CaptureSubscriberKind::Take,
                sender: take_tx,
                archive: Some(TakeArchiveFeed {
                    sender: SpillSender::Take(archive_tx),
                    captured_samples: Arc::clone(&samples),
                    send_failed: Arc::clone(&refused),
                    refused: Arc::clone(&refused_pcm),
                }),
            },
            PcmFeed {
                id: CaptureSubscriberId(2),
                kind: CaptureSubscriberKind::Channel,
                sender: channel_tx,
                archive: None,
            },
        ];
        let dropped = AtomicU64::new(0);
        for _ in 0..3 {
            offer_pcm_to_feeds(&mut feeds, &[0.2; 40], &dropped);
        }
        assert!(refused.load(Ordering::SeqCst));
        assert_eq!(samples.load(Ordering::Relaxed), 120);
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
        // One block fit the queue; the two refused ones are kept, in order.
        let mut expected = convert_mono_f32_to_i16(&[0.2; 40]);
        expected.extend(convert_mono_f32_to_i16(&[0.2; 40]));
        assert_eq!(*refused_pcm.lock().unwrap(), expected);
        for _ in 0..3 {
            assert_eq!(take_rx.try_recv().unwrap(), vec![0.2; 40]);
            assert_eq!(channel_rx.try_recv().unwrap(), vec![0.2; 40]);
        }
    }

    /// A writer stall refuses blocks at the bounded take queue. The refused
    /// PCM is evidence, not loss: finalize reports it as the unconfirmed tail
    /// after the written prefix and the existing recovery rebuilds the take.
    #[test]
    #[serial]
    #[serial(tts_duck)]
    fn refused_take_archive_pcm_stays_recoverable_after_writer_stall() {
        crate::audio::tts_duck::clear();
        let captured = Arc::new(AtomicU64::new(0));
        let archive = TakeArchive::spawn(16_000, Arc::clone(&captured)).unwrap();
        let (take_tx, _take_rx) = mpsc::channel(8);
        let mut feeds = vec![PcmFeed {
            id: CaptureSubscriberId(1),
            kind: CaptureSubscriberKind::Take,
            sender: take_tx,
            archive: Some(archive.feed().unwrap()),
        }];
        let dropped = AtomicU64::new(0);
        // The writer admits the first block.
        offer_pcm_to_feeds(&mut feeds, &[0.2; 40], &dropped);
        // Then it stalls: a zero-capacity queue refuses every later block.
        let (stalled_tx, _stalled_rx) = std::sync::mpsc::sync_channel(0);
        feeds[0].archive.as_mut().unwrap().sender = SpillSender::Take(stalled_tx);
        offer_pcm_to_feeds(&mut feeds, &[0.4; 40], &dropped);
        offer_pcm_to_feeds(&mut feeds, &[0.6; 40], &dropped);
        assert_eq!(captured.load(Ordering::Relaxed), 120);
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
        drop(feeds);

        let error = finalize_take_archive(archive, 120)
            .unwrap_err()
            .downcast::<CaptureArchiveError>()
            .unwrap();
        assert_eq!(error.written_samples, 40);
        assert_eq!(error.unwritten_samples.len(), 80);
        assert_eq!(error.buffer_start_offset, 120);
        assert_eq!(error.captured_samples, 120);
        assert!(error.retained_samples.is_empty());

        let recovered = error.recover_complete_archive().unwrap();
        error.acknowledge_recovery(&recovered).unwrap();
        let mut wav = hound::WavReader::open(recovered.path()).unwrap();
        assert_eq!(wav.spec().sample_rate, 16_000);
        let pcm: Vec<i16> = wav.samples().map(Result::unwrap).collect();
        let mut expected = convert_mono_f32_to_i16(&[0.2; 40]);
        expected.extend(convert_mono_f32_to_i16(&[0.4; 40]));
        expected.extend(convert_mono_f32_to_i16(&[0.6; 40]));
        assert_eq!(pcm, expected, "written prefix then the refused tail");
        std::fs::remove_file(recovered.path()).unwrap();
        if let Some(source) = error.source_path.as_deref() {
            let _ = std::fs::remove_file(source);
        }
    }

    /// A live channel retains its own subscriber-scoped PCM. Blocks the
    /// device heard before the channel opened belong to the device span and
    /// never to the channel's session.
    #[tokio::test]
    #[serial]
    #[serial(tts_duck)]
    async fn live_channel_archives_only_its_own_subscription_span() {
        crate::audio::tts_duck::clear();
        let mut recorder = StreamingRecorder::new().unwrap();
        recorder.sample_rate = 16_000;
        let (take_tx, mut take_rx) = mpsc::channel(8);
        let take = recorder.acquire_capture_subscriber(CaptureSubscriberKind::Take, take_tx);
        offer_pcm_to_feeds(
            &mut recorder.pcm_feeds.lock().unwrap(),
            &[0.1; 40],
            &recorder.dropped_chunks,
        );
        let archive = TakeArchive::spawn(16_000, Arc::new(AtomicU64::new(0))).unwrap();
        let registration = recorder.register_channel_feed_archived(Some(archive.feed().unwrap()));
        let (lifecycle, _) = recorder_lifecycle_channel();
        let (_, last_window) = oneshot::channel();
        recorder.channel_tasks.push(ChannelTask {
            subscriber: registration.id,
            task: tokio::spawn(async {}),
            lifecycle,
            last_window,
            archive: Some(archive),
            terminal_audio_sender: None,
            session_id: String::new(),
            sample_rate: 16_000,
        });
        offer_pcm_to_feeds(
            &mut recorder.pcm_feeds.lock().unwrap(),
            &[0.5; 160],
            &recorder.dropped_chunks,
        );
        let closed = recorder.close_channel_session(registration.id).await;
        let (last, path) = closed.finish().await.unwrap();
        assert!(!last, "the take still owns the device");
        let path = path.expect("live channel owns an exact archive");
        let mut wav = hound::WavReader::open(&path).unwrap();
        assert_eq!(wav.spec().sample_rate, 16_000);
        let pcm: Vec<i16> = wav.samples().map(Result::unwrap).collect();
        assert_eq!(pcm, convert_mono_f32_to_i16(&[0.5; 160]));
        std::fs::remove_file(path).unwrap();
        assert_eq!(take_rx.recv().await.unwrap(), vec![0.1; 40]);
        assert_eq!(take_rx.recv().await.unwrap(), vec![0.5; 160]);
        assert!(recorder.release_capture_subscriber(take));
    }

    /// A channel seal hands its own finalized archive to its session before
    /// the session task is joined, under the identity the session's stop
    /// tail checks. Maciej's channel-0 take of 2026-10-16 20:43 ended with
    /// `dictation_witness_pcm_unavailable` because no receipt was ever sent;
    /// the terminal witness then had nothing to decode the 72 % of speech
    /// Apple never committed. The task below blocks on the receipt, so a
    /// receipt sent after the join (or never) fails this test by timeout.
    #[tokio::test]
    #[serial]
    #[serial(tts_duck)]
    async fn channel_close_hands_its_archive_to_the_session_before_joining_it() {
        crate::audio::tts_duck::clear();
        let mut recorder = StreamingRecorder::new().unwrap();
        recorder.sample_rate = 16_000;
        let (take_tx, mut take_rx) = mpsc::channel(8);
        let take = recorder.acquire_capture_subscriber(CaptureSubscriberKind::Take, take_tx);
        offer_pcm_to_feeds(
            &mut recorder.pcm_feeds.lock().unwrap(),
            &[0.1; 40],
            &recorder.dropped_chunks,
        );
        let archive = TakeArchive::spawn(16_000, Arc::new(AtomicU64::new(0))).unwrap();
        let registration = recorder.register_channel_feed_archived(Some(archive.feed().unwrap()));
        let (lifecycle, _) = recorder_lifecycle_channel();
        let (_, last_window) = oneshot::channel();
        let (terminal_tx, terminal_rx) = std::sync::mpsc::channel();
        let received = Arc::new(StdMutex::new(None));
        let sink = Arc::clone(&received);
        let session_id = "agent-channel-0-receipt-order".to_string();
        recorder.channel_tasks.push(ChannelTask {
            subscriber: registration.id,
            task: tokio::spawn(async move {
                // The session's stop tail: wait for the archive, as
                // `apple_live_session` does with `recv_timeout(30 s)`.
                let receipt = tokio::task::spawn_blocking(move || {
                    terminal_rx.recv_timeout(std::time::Duration::from_secs(5))
                })
                .await
                .unwrap()
                .expect("archive receipt must arrive before the session task is joined");
                *sink.lock().unwrap() = Some(receipt);
            }),
            lifecycle,
            last_window,
            archive: Some(archive),
            terminal_audio_sender: Some(terminal_tx),
            session_id: session_id.clone(),
            sample_rate: 16_000,
        });
        offer_pcm_to_feeds(
            &mut recorder.pcm_feeds.lock().unwrap(),
            &[0.5; 160],
            &recorder.dropped_chunks,
        );
        let closed = recorder.close_channel_session(registration.id).await;
        let (last, path) = closed.finish().await.unwrap();
        assert!(
            !last,
            "the other subscriber keeps its capture while the witness receives PCM"
        );
        assert_eq!(recorder.capture_subscriber_count(), 1);
        let path = path.expect("live channel owns an exact archive");
        let receipt = received
            .lock()
            .unwrap()
            .take()
            .expect("the session task stored the receipt")
            .expect("a finalized archive is an Ok receipt");
        assert_eq!(receipt.session_id, session_id);
        assert_eq!(receipt.capture_epoch, 1);
        assert_eq!(receipt.sample_rate, 16_000);
        assert_eq!(receipt.sample_count, 160);
        assert_eq!(receipt.path, path);
        // The session loads it under the same identity and sample count it
        // observed; a mismatch here would be `terminal_owned_pcm_unavailable`.
        receipt
            .load(&session_id, 1, 16_000, 160)
            .expect("the channel archive is the session's owned terminal PCM");
        let mut wav = hound::WavReader::open(&path).unwrap();
        let pcm: Vec<i16> = wav.samples().map(Result::unwrap).collect();
        assert_eq!(pcm, convert_mono_f32_to_i16(&[0.5; 160]));
        offer_pcm_to_feeds(
            &mut recorder.pcm_feeds.lock().unwrap(),
            &[0.75; 80],
            &recorder.dropped_chunks,
        );
        assert_eq!(take_rx.recv().await.unwrap(), vec![0.1; 40]);
        assert_eq!(take_rx.recv().await.unwrap(), vec![0.5; 160]);
        assert_eq!(take_rx.recv().await.unwrap(), vec![0.75; 80]);
        assert!(recorder.release_capture_subscriber(take));
        std::fs::remove_file(path).unwrap();
    }

    /// The capture callback's fan-out is bounded and non-blocking: a full feed
    /// drops the block and counts it, a closed feed is reaped, and neither
    /// blocks nor removes the healthy feeds.
    #[test]
    fn pcm_fanout_drops_full_feeds_and_reaps_closed_ones() {
        let dropped = AtomicU64::new(0);
        let mut feeds: Vec<PcmFeed> = Vec::new();

        let (open_tx, mut open_rx) = mpsc::channel::<Vec<f32>>(1);
        feeds.push(PcmFeed {
            archive: None,
            id: CaptureSubscriberId(1),
            kind: CaptureSubscriberKind::Take,
            sender: open_tx,
        });
        let (full_tx, _full_rx) = mpsc::channel::<Vec<f32>>(1);
        full_tx
            .try_send(vec![0.0])
            .expect("prefill the bounded feed to capacity");
        feeds.push(PcmFeed {
            archive: None,
            id: CaptureSubscriberId(2),
            kind: CaptureSubscriberKind::Take,
            sender: full_tx,
        });
        let (closed_tx, closed_rx) = mpsc::channel::<Vec<f32>>(1);
        drop(closed_rx);
        feeds.push(PcmFeed {
            archive: None,
            id: CaptureSubscriberId(3),
            kind: CaptureSubscriberKind::Take,
            sender: closed_tx,
        });

        offer_pcm_to_feeds(&mut feeds, &[1.0, 2.0, 3.0], &dropped);

        assert_eq!(
            open_rx.try_recv().expect("healthy feed receives the block"),
            vec![1.0, 2.0, 3.0]
        );
        assert_eq!(
            dropped.load(Ordering::Relaxed),
            1,
            "full feed drops, counted"
        );
        assert_eq!(
            feeds.len(),
            2,
            "closed feed reaped, full feed retained for the next block"
        );
        assert!(feeds.iter().any(|feed| feed.id == CaptureSubscriberId(1)));
        assert!(feeds.iter().any(|feed| feed.id == CaptureSubscriberId(2)));
    }

    /// One dictation take at a time: a second `start_event_session` is refused
    /// before any device work while a take subscription is live.
    ///
    /// `#[serial]`: the runtime-snapshot load resolves the process-global
    /// `CODESCRIBE_DATA_DIR`; off the serial lane it can consume or rewrite a
    /// serial settings test's private fixture (X-hermetic-test-config).
    #[tokio::test]
    #[serial]
    async fn start_event_session_refuses_a_second_take_subscriber() {
        let mut recorder = StreamingRecorder::new().expect("Failed to create recorder");
        recorder.set_event_sink(Some(Arc::new(
            crate::pipeline::sinks::CollectorEventSink::new(),
        )));
        let runtime_settings = Arc::new(
            crate::config::Config::load_runtime_snapshot_without_keychain()
                .expect("keychain-free runtime snapshot"),
        );
        recorder.bind_session_authority("first-take".to_string(), runtime_settings);
        let (take_tx, _take_rx) = mpsc::channel::<Vec<f32>>(8);
        recorder.acquire_capture_subscriber(CaptureSubscriberKind::Take, take_tx);

        let err = recorder
            .start_event_session(None)
            .await
            .expect_err("a second take must be refused while one subscribes");
        assert!(
            err.to_string().contains("already subscribes"),
            "unexpected error: {err:?}"
        );
    }

    /// Live mic: SpeechSession emits chunks for lt/eq/gt VAD block sizes (opt-in).
    #[test]
    #[ignore] // Manual: requires microphone + Silero model (set CODESCRIBE_E2E_MIC=1)
    fn test_vad_gate_live_chunk_sizes() {
        if !env_bool("CODESCRIBE_E2E_MIC") {
            eprintln!("Skipping mic gate test (set CODESCRIBE_E2E_MIC=1 to enable)");
            return;
        }

        let model_path = vad::default_model_path();
        if !model_path.exists() {
            eprintln!(
                "Skipping: Silero VAD model not found at {}",
                model_path.display()
            );
            return;
        }

        let record_sec = env_f32("CODESCRIBE_E2E_MIC_SEC", 6.0).max(2.0);
        println!("Speak now for ~{:.1}s...", record_sec);

        let mut recorder = Recorder::new().expect("Failed to create recorder");
        let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
        let wav_path = rt
            .block_on(async {
                recorder.start().await.expect("Failed to start recorder");
                tokio::time::sleep(Duration::from_secs_f32(record_sec)).await;
                recorder.stop().await.expect("Failed to stop recorder")
            })
            .expect("No WAV produced");

        let (samples, sample_rate) =
            load_audio_file(&wav_path).expect("Failed to load recorded audio");

        let mut resampler = vad::Resampler::new(sample_rate);
        let samples_16k = resampler.resample(&samples);
        let chunk_sec = 4.0f32;
        let chunk_limit = (vad::VAD_SAMPLE_RATE as f32 * chunk_sec) as usize;

        let cases = [
            ("lt", chunk_limit / 2),
            ("eq", chunk_limit),
            ("gt", chunk_limit * 2),
        ];

        for (label, block_len) in cases {
            let mut session = SpeechSession::new_stream(vad::VAD_SAMPLE_RATE, chunk_sec, 0.0);
            let mut chunk_events = 0usize;
            let mut idx = 0usize;
            while idx < samples_16k.len() {
                let end = (idx + block_len).min(samples_16k.len());
                let slice = &samples_16k[idx..end];
                for event in session.feed(slice, slice) {
                    if matches!(event, SpeechEvent::Chunk(_)) {
                        chunk_events += 1;
                    }
                }
                idx = end;
            }
            if let Some(SpeechEvent::Chunk(_)) = session.flush() {
                chunk_events += 1;
            }

            assert!(
                chunk_events > 0,
                "Expected at least one chunk for case {} (block_len={})",
                label,
                block_len
            );
        }

        let _ = fs::remove_file(&wav_path);
    }

    /// Opt-in flag: true only for `1` or case-insensitive `true`.
    fn env_bool(key: &str) -> bool {
        std::env::var(key)
            .ok()
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    }

    /// Parse env `f32`; unset or unparsable yields `default`.
    fn env_f32(key: &str, default: f32) -> f32 {
        std::env::var(key)
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(default)
    }

    /// Terminal no-speech / `*_failed.wav` names that must not score as VAD misses.
    fn is_terminal_no_speech_artifact(file_name: &str) -> bool {
        file_name.contains("no-speech") || file_name.ends_with("_failed.wav")
    }

    /// Name-bounded filter: only explicit no-speech / `*_failed.wav` names match.
    #[test]
    fn terminal_no_speech_artifact_filter_is_name_bounded() {
        assert!(is_terminal_no_speech_artifact(
            "20260709_120000_no-speech_raw.wav"
        ));
        assert!(is_terminal_no_speech_artifact(
            "20260709_120001_dictation_failed.wav"
        ));
        assert!(!is_terminal_no_speech_artifact(
            "20260709_120002_failed-but-recovered_raw.wav"
        ));
        assert!(!is_terminal_no_speech_artifact(
            "03_algorytm-ma-zlozonosc.wav"
        ));
        assert!(!is_terminal_no_speech_artifact("dictation_failed.m4a"));
    }

    /// One VAD chunk in input sample-rate space — max allowed index drift.
    fn vad_index_drift_tolerance(input_sr: u32) -> usize {
        ((vad::CHUNK_SIZE as f32 * input_sr as f32) / vad::VAD_SAMPLE_RATE as f32) as usize
    }

    /// Synthetic tone: VAD→raw index mapping stays within one-chunk tolerance.
    #[test]
    #[serial]
    fn test_vad_index_sync_no_drift() {
        let input_sr = 48000u32;
        let callback_size = 1024usize;
        let num_callbacks = 100usize;

        let mut session = SpeechSession::new_stream(input_sr, 15.0, 0.0);
        assert_eq!(
            session.gate_mode(),
            crate::audio::chunker::VadGateMode::Supervisor,
            "drift guard must explicitly validate Supervisor mode"
        );

        let freq = 440.0f32;
        let mut phase = 0.0f32;
        let phase_inc = 2.0 * std::f32::consts::PI * freq / input_sr as f32;

        for _ in 0..num_callbacks {
            let mut buf = Vec::with_capacity(callback_size);
            for _ in 0..callback_size {
                buf.push(phase.sin() * 0.5);
                phase += phase_inc;
            }
            let _ = session.feed(&buf, &buf);
        }

        let total_raw = num_callbacks * callback_size;
        assert_eq!(
            session.raw_cursor(),
            total_raw,
            "raw_cursor should equal total input samples"
        );

        let vad_sample = session
            .vad_current_sample()
            .expect("Supervisor mode should expose VAD sample index");
        let mapped = session.vad_to_raw_index_pub(vad_sample);
        let raw_cur = session.raw_cursor();
        let drift = mapped.abs_diff(raw_cur);
        let tolerance = vad_index_drift_tolerance(input_sr);
        assert!(
            drift <= tolerance,
            "VAD index drift too large: mapped={} raw_cursor={} drift={} tolerance={}",
            mapped,
            raw_cur,
            drift,
            tolerance
        );

        assert!(
            session.vad_resample_buf_len() < vad::CHUNK_SIZE,
            "Residual buffer should be < CHUNK_SIZE, got {}",
            session.vad_resample_buf_len()
        );
    }

    /// Busy Supervisor path: interim/final keep boundary and speech accounting.
    #[test]
    #[serial]
    fn test_supervisor_busy_flush_keeps_boundary_and_speech_accounting() {
        let input_sr = 48000u32;
        let callback_size = 1024usize;
        let num_callbacks = 210usize;
        // Pin the interim cadence: the env-driven default
        // (`CODESCRIBE_BUFFERED_INTERIM_SEC`) is process-global, and any
        // `Config::load()` test reading a real settings.json reseeds it for
        // the whole test process — a cadence above the fed sample count would
        // silently produce zero interim events (the 2026-09-30 flake).
        let interim_sec = 1.2f32;

        let mut session = SpeechSession::new_utterance_pinned_for_test(input_sr, interim_sec, 10.0);
        assert_eq!(
            session.gate_mode(),
            VadGateMode::Supervisor,
            "busy flush guard must explicitly validate Supervisor mode"
        );

        // Deterministic open segment even when VAD model is unavailable.
        session.set_vad_threshold_for_test(-1.0);

        let mut interim_events = 0usize;
        let mut accounted_speech_vad_samples = 0u64;

        for _ in 0..num_callbacks {
            let buf = vec![0.0f32; callback_size];
            for event in session.feed(&buf, &buf) {
                let event_speech = session.take_event_speech_vad_samples();
                accounted_speech_vad_samples =
                    accounted_speech_vad_samples.saturating_add(event_speech);
                match event {
                    SpeechEvent::Utterance => {
                        interim_events = interim_events.saturating_add(1);
                        assert!(
                            event_speech > 0,
                            "busy interim event should carry positive speech sample accounting"
                        );
                    }
                    SpeechEvent::UtteranceFinal => {
                        panic!("unexpected UtteranceFinal before flush in long-silence test")
                    }
                    SpeechEvent::Chunk(_) => {
                        panic!("unexpected Chunk event in utterance mode")
                    }
                }
            }
        }

        let total_raw = num_callbacks * callback_size;
        assert!(
            interim_events > 0,
            "busy callback run should emit at least one interim utterance before flush \
             (interim_events={interim_events}, fed_raw={total_raw}, \
             raw_cursor={}, interim_limit_raw={:?}, vad_current_sample={:?}, \
             accounted_speech_vad_samples={accounted_speech_vad_samples})",
            session.raw_cursor(),
            session.interim_limit_raw_for_test(),
            session.vad_current_sample(),
        );

        let flush = session.flush();
        let flush_speech = session.take_event_speech_vad_samples();
        accounted_speech_vad_samples = accounted_speech_vad_samples.saturating_add(flush_speech);

        match flush {
            Some(SpeechEvent::UtteranceFinal) => (),
            Some(SpeechEvent::Utterance) => {
                panic!("flush should emit final utterance event")
            }
            Some(SpeechEvent::Chunk(_)) => {
                panic!("flush should not emit stream chunk in utterance mode")
            }
            None => panic!("flush should preserve active Supervisor boundary under busy load"),
        };
        assert!(
            flush_speech > 0,
            "flush final event should carry pending speech sample accounting"
        );
        assert_eq!(
            session.take_event_speech_vad_samples(),
            0,
            "speech accounting queue should be empty after consuming flush event"
        );

        let total_raw = num_callbacks * callback_size;
        assert_eq!(
            session.raw_cursor(),
            total_raw,
            "raw cursor should stay aligned with callback sample count under busy load"
        );

        let vad_sample = session
            .vad_current_sample()
            .expect("Supervisor mode should expose VAD sample index");
        let mapped = session.vad_to_raw_index_pub(vad_sample);
        let raw_cur = session.raw_cursor();
        let drift = mapped.abs_diff(raw_cur);
        let tolerance = vad_index_drift_tolerance(input_sr);
        assert!(
            drift <= tolerance,
            "busy path drift too large: mapped={} raw_cursor={} drift={} tolerance={}",
            mapped,
            raw_cur,
            drift,
            tolerance
        );
        assert_eq!(
            accounted_speech_vad_samples as usize, vad_sample,
            "sum of emitted speech sample accounting should equal processed VAD samples"
        );
    }

    /// Run VAD on real WAV files and report segmentation quality.
    #[test]
    fn test_vad_supervisor_segments_real_audio() {
        let corpus_dir =
            std::path::PathBuf::from(shellexpand::tilde("~/.codescribe/transcriptions").as_ref());
        if !corpus_dir.exists() {
            eprintln!("Skipping: no transcriptions dir");
            return;
        }
        let model_path = vad::default_model_path();
        if !model_path.exists() {
            eprintln!("Skipping: no Silero model");
            return;
        }

        let edge_cases = [
            "192322_nie-zmienia-to_raw.wav",
            "133135_no-dobra-teraz_raw.wav",
            "182340_klaudiusz-zacznijmy-od_raw.wav",
            "001615_dziekuje---dziekuje_raw.wav",
            "184818_dzien-dobry-chcialem_raw.wav",
        ];

        let mut wavs: Vec<std::path::PathBuf> = Vec::new();
        if let Ok(dirs) = fs::read_dir(&corpus_dir) {
            for dir_entry in dirs.flatten() {
                if !dir_entry.path().is_dir() {
                    continue;
                }
                for case in &edge_cases {
                    let candidate = dir_entry.path().join(case);
                    if candidate.exists() {
                        wavs.push(candidate);
                    }
                }
            }
        }
        if wavs.is_empty() {
            let mut dirs: Vec<_> = fs::read_dir(&corpus_dir)
                .unwrap()
                .flatten()
                .filter(|e| e.path().is_dir())
                .collect();
            dirs.sort_by_key(|e| e.file_name());
            dirs.reverse();
            for dir in dirs.iter().take(2) {
                if let Ok(entries) = fs::read_dir(dir.path()) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.extension().and_then(|s| s.to_str()) == Some("wav") {
                            let fname = p.file_name().unwrap_or_default().to_string_lossy();
                            // Terminal failed/no-speech artifacts should not be scored as VAD segmentation misses.
                            if is_terminal_no_speech_artifact(&fname) {
                                continue;
                            }
                            wavs.push(p);
                            if wavs.len() >= 5 {
                                break;
                            }
                        }
                    }
                }
            }
        }

        println!("\n╭─── VAD v5 Segmentation Test ───────────────────────╮");
        let mut all_pass = true;

        for wav_path in &wavs {
            let fname = wav_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let (samples, sample_rate) = match load_audio_file(wav_path) {
                Ok(v) => v,
                Err(e) => {
                    println!("│ SKIP {} — {}", fname, e);
                    continue;
                }
            };
            let audio_sec = samples.len() as f32 / sample_rate as f32;

            let vad_config = vad::VadConfig {
                threshold: 0.50,
                min_speech_duration_sec: 0.05,
                max_silence_duration_sec: 0.20,
                max_utterance_sec: 300.0,
                pre_roll_sec: 0.064,
                ..vad::VadConfig::default()
            };
            let mut silero = vad::SileroVad::new(&model_path, vad_config).expect("load Silero");
            let mut resampler = vad::Resampler::new(sample_rate);
            let samples_16k = resampler.resample(&samples);

            let mut above = 0usize;
            let mut total = 0usize;
            for chunk in samples_16k.chunks(vad::CHUNK_SIZE) {
                if chunk.len() < vad::CHUNK_SIZE {
                    break;
                }
                total += 1;
                if silero.predict(chunk).unwrap_or(0.0) >= 0.5 {
                    above += 1;
                }
            }

            let callback_size = 1024usize;
            let mut session = SpeechSession::new_utterance(sample_rate);
            let mut events = Vec::new();
            let mut offset = 0usize;
            while offset < samples.len() {
                let end = (offset + callback_size).min(samples.len());
                for event in session.feed(&samples[offset..end], &samples[offset..end]) {
                    events.push(event);
                }
                offset = end;
            }
            if let Some(event) = session.flush() {
                events.push(event);
            }

            let n_segments = events.len();
            let speech_samples: usize = events
                .iter()
                .map(|e| match e {
                    SpeechEvent::Utterance | SpeechEvent::UtteranceFinal => 0,
                    SpeechEvent::Chunk(s) => s.len(),
                })
                .sum();
            let speech_sec = speech_samples as f32 / sample_rate as f32;
            let silence_cut = audio_sec - speech_sec;
            let cut_pct = if audio_sec > 0.0 {
                silence_cut / audio_sec * 100.0
            } else {
                0.0
            };

            let raw_txt = wav_path.to_string_lossy().replace("_raw.wav", "_raw.txt");
            let old_len = fs::read_to_string(&raw_txt).map(|s| s.len()).unwrap_or(0);

            println!("│");
            println!("│ 📁 {}", fname);
            println!(
                "│    Audio: {:.1}s | VAD speech: {:.0}% ({}/{} frames)",
                audio_sec,
                if total > 0 {
                    above as f32 / total as f32 * 100.0
                } else {
                    0.0
                },
                above,
                total,
            );
            println!(
                "│    Segments: {} | Speech: {:.1}s | Silence cut: {:.1}s ({:.0}%)",
                n_segments, speech_sec, silence_cut, cut_pct,
            );
            println!("│    Old transcript: {} chars", old_len,);

            let old_text = fs::read_to_string(&raw_txt).unwrap_or_default();
            let halluc_count = old_text.matches("Thank you").count()
                + old_text.matches("Dziękuję.").count()
                + old_text.matches(".com/").count();
            if halluc_count > 2 {
                println!(
                    "│    ⚠ Old transcript had {} hallucination markers (Thank you/Dziękuję./.com/)",
                    halluc_count,
                );
                println!(
                    "│    ✅ VAD v5 would cut {:.1}s silence → these tails eliminated",
                    silence_cut,
                );
            }

            if above == 0 && audio_sec > 1.0 {
                println!("│    ❌ VAD detected NO speech — possible model issue");
                all_pass = false;
            }
        }

        println!("│");
        println!("╰────────────────────────────────────────────────────╯\n");

        assert!(all_pass, "Some files had zero speech detection");
    }
}

#[cfg(test)]
mod terminal_seal_refusal_tests {
    use super::TerminalSealRefused;
    use crate::pipeline::acoustic_ledger::{SealCoverageReceipt, SealCoverageStatus};

    fn refusal(audio_path: Option<std::path::PathBuf>) -> TerminalSealRefused {
        let receipt = SealCoverageReceipt {
            sample_rate_hz: None,
            session_id: "e4060d87-fe0f-49fd-bbd5-eaea7e89ca17".to_string(),
            capture_epoch: 0,
            speech_samples: 2_696_704,
            covered_samples: 585_216,
            uncovered_speech_ranges: Vec::new(),
            max_uncovered_samples: 2_111_488,
            incomplete_threshold_samples: 12_000,
            status: SealCoverageStatus::Incomplete,
            speech_producer: "capture_energy".to_string(),
            availability: "observed".to_string(),
            observed_samples: Some(2_696_704),
        };
        let mut ledger = crate::pipeline::acoustic_ledger::AcousticLedger::new();
        assert!(ledger.record_seal_coverage(receipt.clone()));
        TerminalSealRefused {
            finality: ledger
                .terminal_finality(&receipt.session_id, receipt.capture_epoch)
                .into_refusal()
                .unwrap(),
            audio_path,
            committed_text: String::new(),
        }
    }

    /// The controller tells a refused seal apart from a failed mic by type,
    /// and the take WAV path survives the trip through `anyhow`.
    #[test]
    fn refusal_downcasts_through_anyhow_with_its_audio_path() {
        let path = std::path::PathBuf::from("/tmp/codescribe_recording_1788315408813.wav");
        let err = anyhow::Error::new(refusal(Some(path.clone())));
        let refused = err
            .downcast::<TerminalSealRefused>()
            .expect("typed refusal survives anyhow");
        assert_eq!(refused.audio_path.as_deref(), Some(path.as_path()));
        assert_eq!(
            refused.finality.coverage().unwrap().status,
            SealCoverageStatus::Incomplete
        );
    }

    /// The message names the refused seal, never the recorder.
    #[test]
    fn refusal_message_names_the_seal_not_the_mic() {
        let text = refusal(None).to_string();
        assert!(text.starts_with("terminal transcript refused"), "{text}");
        assert!(text.contains("585216/2696704"), "{text}");
        assert!(!text.to_lowercase().contains("recorder"), "{text}");
    }
}

/// W2 source contracts: UNRUN. Only capture/archive ingress is injected;
/// complete_stop is the same notification/join/drain/error path used by stop.
#[cfg(test)]
mod capture_stop_failure_tests {
    use super::*;
    use crate::pipeline::acoustic_ledger::SealCoverageStatus;

    /// Synchronous test presentation: on_event has no queued work. Acknowledgement
    /// is owned by this fixture and still requires the exact capture identity.
    struct SynchronousTestPresentation;

    impl crate::pipeline::contracts::EventSink for SynchronousTestPresentation {
        fn on_event(&self, _event: &EngineEvent) {}
        fn wait_presentation_published<'a>(
            &'a self,
            session_id: &'a str,
            capture_epoch: u64,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
            Box::pin(async move {
                anyhow::ensure!(
                    session_id == "capture-owner" && capture_epoch == 7,
                    "foreign capture"
                );
                Ok(())
            })
        }
    }

    struct ControlledTestPresentation {
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }

    impl crate::pipeline::contracts::EventSink for ControlledTestPresentation {
        fn on_event(&self, _event: &EngineEvent) {}
        fn wait_presentation_published<'a>(
            &'a self,
            session_id: &'a str,
            capture_epoch: u64,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
            Box::pin(async move {
                anyhow::ensure!(
                    session_id == "capture-owner" && capture_epoch == 7,
                    "foreign capture"
                );
                self.entered.notify_one();
                self.release.notified().await;
                Ok(())
            })
        }
    }

    fn recorder() -> StreamingRecorder {
        let mut recorder = StreamingRecorder::new().unwrap();
        recorder.authority_session_id = Some("capture-owner".into());
        recorder.capture_epoch = 7;
        recorder.captured_samples.store(4, Ordering::Relaxed);
        recorder.lifecycle_handle = Some(recorder_lifecycle_channel().0);
        recorder
    }

    #[tokio::test]
    async fn direct_stop_supplies_archive_before_completion_and_survives_cancelled_wait() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("direct-tail.wav");
        let bytes = write_wav(&path);
        let mut recorder = recorder();
        recorder.sample_rate = 16_000;
        *recorder.transcript_buffer.lock().await = "English survives Stop".into();
        recorder.set_event_sink(Some(Arc::new(SynchronousTestPresentation)));
        let (archive_tx, archive_rx) = std::sync::mpsc::channel();
        recorder.terminal_audio_sender = Some(archive_tx);
        let (done_tx, done_rx) = oneshot::channel();
        recorder.last_window_closed = Some(done_rx);
        let mut closed = recorder.detach_take_state(Some(Ok(Some(path.clone()))), None, None);
        closed.direct_transcript = true;
        assert!(
            tokio::time::timeout(
                Duration::from_millis(10),
                closed.wait_live_finals_admitted()
            )
            .await
            .is_err()
        );
        let archive = archive_rx
            .try_recv()
            .expect("archive must precede completion")
            .unwrap();
        assert_eq!(
            (
                archive.session_id.as_str(),
                archive.capture_epoch,
                archive.sample_count
            ),
            ("capture-owner", 7, 4)
        );
        assert_eq!(archive.path, path);
        assert!(
            closed.last_window_closed.is_some(),
            "cancellation retains completion receiver"
        );
        assert!(closed.direct_decoder_complete.is_none());
        done_tx.send(()).unwrap();
        assert!(closed.wait_live_finals_admitted().await);
        assert!(
            closed.wait_live_finals_admitted().await,
            "cached completion survives its consumer"
        );
        assert!(
            closed.acoustic_ledger.is_none(),
            "no invented ledger authority"
        );
        assert_eq!(
            closed.finish().await.unwrap(),
            ("English survives Stop".into(), Some(path.clone()))
        );
        assert!(closed.is_settled());
        assert_eq!(std::fs::read(path).unwrap(), bytes);
        assert!(archive_rx.try_recv().is_err(), "one archive publication");
    }

    #[tokio::test]
    async fn direct_stop_requires_native_completion_and_preserves_failed_take_audio() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("failed-direct-tail.wav");
        let bytes = write_wav(&path);
        let mut recorder = recorder();
        *recorder.transcript_buffer.lock().await = "recoverable English".into();
        recorder.set_event_sink(Some(Arc::new(SynchronousTestPresentation)));
        let (done_tx, done_rx) = oneshot::channel();
        recorder.last_window_closed = Some(done_rx);
        drop(done_tx);
        let mut closed = recorder.detach_take_state(Some(Ok(Some(path.clone()))), None, None);
        closed.direct_transcript = true;
        let error = closed.finish().await.unwrap_err();
        let failure = error.downcast_ref::<CaptureStopFailure>().unwrap();
        assert_eq!(failure.audio_path.as_ref(), Some(&path));
        assert!(
            failure
                .cause
                .to_string()
                .contains("direct recognizer completion")
        );
        assert_eq!(
            &*closed.transcript_buffer.lock().await,
            "recoverable English"
        );
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }

    #[tokio::test]
    async fn direct_stop_waits_for_original_presentation_after_decoder_completion() {
        let mut recorder = recorder();
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        recorder.set_event_sink(Some(Arc::new(ControlledTestPresentation {
            entered: entered.clone(),
            release: release.clone(),
        })));
        let text = recorder.transcript_buffer.clone();
        let (done_tx, done_rx) = oneshot::channel();
        recorder.last_window_closed = Some(done_rx);
        done_tx.send(()).unwrap();
        let mut closed = recorder.detach_take_state(Some(Ok(None)), None, None);
        closed.direct_transcript = true;
        let task = tokio::spawn(async move { closed.finish().await });
        entered.notified().await;
        assert!(
            !task.is_finished(),
            "native completion is not presentation completion"
        );
        *text.lock().await = "final English".into();
        release.notify_one();
        assert_eq!(task.await.unwrap().unwrap(), ("final English".into(), None));
    }

    #[tokio::test]
    async fn short_capture_without_ledger_speech_ends_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("take.wav");
        write_wav(&path);
        let mut recorder = recorder();
        recorder.captured_samples.store(2_560, Ordering::Relaxed);
        let (text, audio) = recorder
            .complete_stop(Ok(Some(path.clone())))
            .await
            .unwrap();
        assert!(text.is_empty());
        assert_eq!(audio.as_deref(), Some(path.as_path()));
    }

    #[tokio::test]
    async fn live_finals_do_not_wait_for_slow_refinement_or_recovery() {
        let mut recorder = recorder();
        let (closed_tx, closed_rx) = oneshot::channel();
        recorder.last_window_closed = Some(closed_rx);
        let l1 =
            tokio::spawn(async { tokio::time::sleep(std::time::Duration::from_millis(650)).await });
        let recovery =
            tokio::spawn(async { tokio::time::sleep(std::time::Duration::from_millis(700)).await });
        let close_start = std::time::Instant::now();
        let (elapsed_tx, elapsed_rx) = oneshot::channel();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(55)).await;
            elapsed_tx.send(close_start.elapsed().as_millis()).unwrap();
            closed_tx.send(()).unwrap();
        });
        assert!(recorder.wait_live_finals_admitted().await);
        let last_window_close_ms = elapsed_rx.await.unwrap();
        let stop_to_ack_ms = close_start.elapsed().as_millis();
        eprintln!("last_window_close_ms={last_window_close_ms} stop_to_ack_ms={stop_to_ack_ms}");
        assert!(stop_to_ack_ms < last_window_close_ms + 300);
        assert!(!l1.is_finished());
        assert!(!recovery.is_finished());
        l1.await.unwrap();
        recovery.await.unwrap();
    }

    #[tokio::test]
    async fn live_final_timeout_keeps_terminal_delivery_available() {
        let mut recorder = recorder();
        let (_closed_tx, closed_rx) = oneshot::channel();
        recorder.last_window_closed = Some(closed_rx);
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(10),
                recorder.wait_live_finals_admitted(),
            )
            .await
            .is_err()
        );
        assert!(
            recorder.last_window_closed.is_some(),
            "caller cancellation must retain the close receipt"
        );
    }

    #[tokio::test]
    async fn archive_worker_and_saved_result_survive_cancelled_stop_waits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cancelled-archive.wav");
        let bytes = write_wav(&path);
        let mut recorder = recorder();
        recorder.acoustic_ledger = Some(Arc::new(StdMutex::new(stop_finality_ledger(true, false))));
        *recorder.transcript_buffer.lock().await = "committed words".into();
        let mut closed = recorder.detach_take_state(None, None, None);
        let (release_tx, release_rx) = oneshot::channel();
        let saved = path.clone();
        closed.archive_worker = Some(tokio::spawn(async move {
            release_rx.await.unwrap();
            Ok(Some(saved))
        }));
        recorder.terminal_take = Some(closed);
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(10),
                recorder.finish_closed_capture(false)
            )
            .await
            .is_err()
        );
        assert!(
            recorder
                .terminal_take
                .as_ref()
                .unwrap()
                .archive_worker
                .is_some()
        );
        let old_text = Arc::clone(&recorder.terminal_take.as_ref().unwrap().transcript_buffer);
        let text_guard = old_text.lock().await;
        release_tx.send(()).unwrap();
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(10),
                recorder.finish_closed_capture(false)
            )
            .await
            .is_err()
        );
        let take = recorder.terminal_take.as_ref().unwrap();
        assert!(take.archive_worker.is_none());
        assert_eq!(
            take.stopped.as_ref().unwrap().as_ref().unwrap().as_ref(),
            Some(&path)
        );
        drop(text_guard);
        assert_eq!(
            recorder.finish_closed_capture(false).await.unwrap(),
            ("committed words".into(), Some(path.clone()))
        );
        assert_eq!(std::fs::read(path).unwrap(), bytes);
        assert_released(&recorder);
    }

    fn write_wav(path: &std::path::Path) -> Vec<u8> {
        let mut writer = hound::WavWriter::create(
            path,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for sample in [123_i16, -456, 789, -321] {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
        std::fs::read(path).unwrap()
    }

    fn assert_released(recorder: &StreamingRecorder) {
        assert!(recorder.terminal_take.is_none());
        assert!(recorder.transcription_handle.is_none());
        assert!(recorder.terminal_audio_sender.is_none());
        assert!(recorder.lifecycle_handle.is_none());
        assert!(recorder.event_sink.is_none());
        assert!(!recorder.is_recording());
    }

    #[tokio::test(start_paused = true)]
    async fn saved_wav_survives_task_panic_with_exact_identity_and_cause() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("take.wav");
        let bytes = write_wav(&path);
        let mut recorder = recorder();
        recorder.sample_rate = 16_000;
        let sink = Arc::new(SynchronousTestPresentation);
        let weak_sink = Arc::downgrade(&sink);
        recorder.set_event_sink(Some(sink));
        // Neither preview nor a raw buffer can become recovery transcript.
        *recorder.transcript_buffer.lock().await = "UNAUTHENTICATED PREVIEW".into();
        let (sender, receiver) = std::sync::mpsc::channel();
        recorder.terminal_audio_sender = Some(sender);
        recorder.transcription_handle = Some(tokio::spawn(async {
            panic!("controlled transcription failure");
        }));
        let error = recorder
            .complete_stop(Ok(Some(path.clone())))
            .await
            .unwrap_err();
        let failure = error.downcast_ref::<CaptureStopFailure>().unwrap();
        assert_eq!(failure.session_id.as_deref(), Some("capture-owner"));
        assert_eq!(failure.capture_epoch, 7);
        assert_eq!(failure.audio_path.as_deref(), Some(path.as_path()));
        assert!(
            failure
                .cause
                .downcast_ref::<tokio::task::JoinError>()
                .unwrap()
                .is_panic()
        );
        assert!(
            error
                .to_string()
                .contains("controlled transcription failure")
        );
        assert!(!format!("{error:?}").contains("UNAUTHENTICATED PREVIEW"));
        assert!(error.downcast_ref::<TerminalSealRefused>().is_none());
        let archive = receiver.try_recv().unwrap().unwrap();
        assert_eq!(archive.session_id, "capture-owner");
        assert_eq!(archive.capture_epoch, 7);
        assert_eq!(archive.path, path);
        assert_eq!(archive.sample_count, 4);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_released(&recorder);
        assert!(weak_sink.upgrade().is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn archive_failure_reaps_worker_and_preserves_both_errors_without_a_path() {
        let mut recorder = recorder();
        recorder.set_event_sink(Some(Arc::new(SynchronousTestPresentation)));
        let (sender, receiver) = std::sync::mpsc::channel();
        recorder.terminal_audio_sender = Some(sender);
        recorder.transcription_handle = Some(tokio::spawn(async {
            panic!("secondary worker failure");
        }));
        let error = recorder
            .complete_stop(Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "archive denied",
            )
            .into()))
            .await
            .unwrap_err();
        let failure = error.downcast_ref::<CaptureStopFailure>().unwrap();
        assert_eq!(
            failure
                .cause
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert!(
            failure
                .task_failure
                .as_ref()
                .unwrap()
                .downcast_ref::<tokio::task::JoinError>()
                .unwrap()
                .is_panic()
        );
        assert!(failure.audio_path.is_none());
        assert!(
            receiver
                .try_recv()
                .unwrap()
                .unwrap_err()
                .contains("archive denied")
        );
        assert_released(&recorder);
    }

    #[tokio::test]
    async fn public_stop_routes_worker_failure_through_the_cleanup_tail() {
        let mut recorder = recorder();
        recorder.transcription_handle = Some(tokio::spawn(async {
            panic!("public stop worker failure");
        }));
        // A never-opened device returns no archive. This exercises public stop
        // without a microphone; the saved-WAV case injects archive ingress above.
        let error = recorder.stop().await.unwrap_err();
        let failure = error.downcast_ref::<CaptureStopFailure>().unwrap();
        assert!(failure.audio_path.is_none());
        assert_eq!(failure.session_id.as_deref(), Some("capture-owner"));
        assert_eq!(failure.capture_epoch, 7);
        assert!(
            failure
                .cause
                .downcast_ref::<tokio::task::JoinError>()
                .unwrap()
                .is_panic()
        );
        assert_released(&recorder);
    }

    #[tokio::test]
    async fn archive_failure_still_joins_a_successful_worker() {
        let mut recorder = recorder();
        let joined = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let completed = Arc::clone(&joined);
        recorder.transcription_handle = Some(tokio::spawn(async move {
            completed.store(true, Ordering::SeqCst);
        }));
        let error = recorder
            .complete_stop(Err(anyhow!("archive failed")))
            .await
            .unwrap_err();
        assert!(joined.load(Ordering::SeqCst));
        assert!(
            error
                .downcast_ref::<CaptureStopFailure>()
                .unwrap()
                .task_failure
                .is_none()
        );
        assert_released(&recorder);
    }

    #[tokio::test]
    async fn local_execution_join_survives_stop_retry_and_preserves_refused_wav() {
        use crate::pipeline::streaming::session::LocalExecutionOwner;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owned-local.wav");
        let bytes = write_wav(&path);
        let mut recorder = recorder();
        let mut ledger = AcousticLedger::new();
        let receipt = SealCoverageReceipt {
            sample_rate_hz: None,
            session_id: "capture-owner".into(),
            capture_epoch: 7,
            speech_samples: 4,
            covered_samples: 0,
            uncovered_speech_ranges: vec![crate::stt::tail_provider::TailSampleRange {
                session: "capture-owner".into(),
                capture_epoch: 7,
                sample_start: 0,
                sample_end: 4,
            }],
            max_uncovered_samples: 4,
            incomplete_threshold_samples: 1,
            status: SealCoverageStatus::Incomplete,
            speech_producer: "capture_energy".to_string(),
            availability: "observed".to_string(),
            observed_samples: Some(4),
        };
        assert!(ledger.record_seal_coverage(receipt.clone()));
        recorder.acoustic_ledger = Some(Arc::new(StdMutex::new(ledger)));
        let execution = Arc::new(LocalExecutionOwner::default());
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let result = execution
            .spawn(move |_| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok("late label")
            })
            .unwrap();
        entered_rx.await.unwrap();
        drop(result); // closed ledger no longer consumes local labels
        recorder.transcription_handle = Some(tokio::spawn(async move {
            execution.close_and_join().await;
        }));
        let pending = tokio::time::timeout(
            std::time::Duration::from_millis(20),
            recorder.complete_stop(Ok(Some(path.clone()))),
        )
        .await;
        let retained = recorder
            .terminal_take
            .as_ref()
            .is_some_and(|take| take.transcription_handle.is_some());
        release_tx.send(()).unwrap();
        let error = recorder
            .complete_stop(Ok(Some(path.clone())))
            .await
            .unwrap_err();
        assert!(
            pending.is_err(),
            "Stop must await actual retained execution"
        );
        assert!(
            retained,
            "caller expiry must leave the session handle available for retry"
        );
        let refused = error.downcast_ref::<TerminalSealRefused>().unwrap();
        assert_eq!(refused.finality.coverage(), Some(&receipt));
        assert_eq!(refused.audio_path.as_ref(), Some(&path));
        assert_eq!(std::fs::read(path).unwrap(), bytes);
        assert_released(&recorder);
    }

    #[tokio::test]
    async fn clean_stop_and_seal_refusal_keep_their_existing_outcomes() {
        let mut recorder = recorder();
        recorder.captured_samples.store(0, Ordering::Relaxed);
        let stopped = recorder.complete_stop(Ok(None)).await.unwrap();
        assert_eq!(stopped, (String::new(), None));
        let mut ledger = AcousticLedger::new();
        assert!(ledger.record_seal_coverage(SealCoverageReceipt {
            sample_rate_hz: None,
            session_id: "capture-owner".into(),
            capture_epoch: 7,
            speech_samples: 100,
            covered_samples: 0,
            uncovered_speech_ranges: Vec::new(),
            max_uncovered_samples: 100,
            incomplete_threshold_samples: 10,
            status: SealCoverageStatus::Incomplete,
            speech_producer: "capture_energy".to_string(),
            availability: "observed".to_string(),
            observed_samples: Some(100),
        }));
        recorder.acoustic_ledger = Some(Arc::new(StdMutex::new(ledger)));
        let error = recorder.complete_stop(Ok(None)).await.unwrap_err();
        assert!(error.downcast_ref::<TerminalSealRefused>().is_some());
        assert!(error.downcast_ref::<CaptureStopFailure>().is_none());
        assert_released(&recorder);
    }

    fn stop_finality_ledger(issued: bool, silence: bool) -> AcousticLedger {
        stop_finality_ledger_with_label(issued, silence, true)
    }

    fn stop_finality_ledger_with_label(issued: bool, silence: bool, whole: bool) -> AcousticLedger {
        use crate::audio::capture_receipt::{
            AcousticAvailability, AcousticSpeechEvidence, CaptureEvidenceIdentity,
        };
        use crate::pipeline::acoustic_ledger::{
            AcousticEvidence, EnergyCalibration, ObservationIdentity, ObservationProducer,
            OccurrenceIdentity,
        };
        let mut ledger = AcousticLedger::new();
        let occurrence = OccurrenceIdentity::new("capture-owner", 7, 0, 4);
        if !silence {
            let calibration = EnergyCalibration::new("stop-synthetic", 1.0, 1);
            assert!(
                ledger
                    .qualify(
                        &AcousticEvidence {
                            occurrence: occurrence.clone(),
                            duration_ms: 1.0,
                            energy_integral: 100.0,
                            mean_rms_dbfs: -20.0,
                            peak_dbfs: -10.0,
                            vad_open_sample: Some(0),
                            vad_close_sample: Some(4),
                            evidence_calibration_version: calibration.version.clone(),
                        },
                        &calibration
                    )
                    .is_qualified()
            );
            ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Whisper]);
            ledger.admit(
                &ObservationIdentity::new(
                    ObservationProducer::Whisper,
                    1,
                    0,
                    if whole {
                        occurrence.clone()
                    } else {
                        OccurrenceIdentity::new("capture-owner", 7, 0, 2)
                    },
                ),
                "committed words",
            );
            ledger.note_frontier_return(&occurrence, ObservationProducer::Whisper);
            if whole {
                ledger.seal(&occurrence).unwrap();
            }
        }
        let speech = AcousticSpeechEvidence::measured(
            CaptureEvidenceIdentity::new("capture-owner", 7),
            crate::audio::capture_receipt::CAPTURE_ENERGY_PRODUCER,
            AcousticAvailability::Observed {
                observed_samples: 4,
            },
            if silence {
                vec![]
            } else {
                vec![crate::stt::tail_provider::TailSampleRange {
                    session: "capture-owner".into(),
                    capture_epoch: 7,
                    sample_start: 0,
                    sample_end: if whole { 4 } else { 2 },
                }]
            },
        );
        ledger.record_seal_coverage(ledger.assess_seal_coverage("capture-owner", 7, &speech, 0));
        if issued {
            ledger.seal_terminal("capture-owner", 7).unwrap();
        }
        ledger
    }

    #[tokio::test]
    async fn complete_stop_refuses_label_missing_despite_complete_coverage() {
        use crate::pipeline::acoustic_ledger::SealRefusal;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("label-missing.wav");
        let bytes = write_wav(&path);
        let mut ledger = stop_finality_ledger_with_label(false, false, false);
        assert!(ledger.latest_seal_coverage().unwrap().status.is_complete());
        assert_eq!(
            ledger.seal_terminal("capture-owner", 7),
            Err(SealRefusal::LabelMissing)
        );
        let mut recorder = recorder();
        recorder.acoustic_ledger = Some(Arc::new(StdMutex::new(ledger)));
        *recorder.transcript_buffer.lock().await = "committed words".into();
        let error = recorder
            .complete_stop(Ok(Some(path.clone())))
            .await
            .unwrap_err();
        let refusal = error.downcast_ref::<TerminalSealRefused>().unwrap();
        assert!(refusal.finality.coverage().unwrap().status.is_complete());
        assert_eq!(refusal.audio_path.as_ref(), Some(&path));
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }

    #[tokio::test]
    async fn complete_stop_requires_issued_terminal_receipt_and_preserves_wav() {
        use crate::pipeline::acoustic_ledger::TerminalFinalityRefusalReason;
        for issued in [false, true] {
            let mut recorder = recorder();
            let ledger = Arc::new(StdMutex::new(stop_finality_ledger(issued, false)));
            recorder.acoustic_ledger = Some(ledger.clone());
            *recorder.transcript_buffer.lock().await = "committed words".into();
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("real-stop-tail.wav");
            let bytes = write_wav(&path);
            let result = recorder.complete_stop(Ok(Some(path.clone()))).await;
            if issued {
                assert_eq!(
                    result.unwrap(),
                    ("committed words".into(), Some(path.clone()))
                );
            } else {
                let error = result.unwrap_err();
                let refusal = error.downcast_ref::<TerminalSealRefused>().unwrap();
                assert_eq!(
                    refusal.finality.reason(),
                    TerminalFinalityRefusalReason::TerminalReceiptMissing
                );
                assert!(refusal.finality.coverage().unwrap().status.is_complete());
                assert_eq!(refusal.audio_path.as_ref(), Some(&path));
                assert_eq!(refusal.committed_text, "committed words");
                assert!(
                    ledger
                        .lock()
                        .unwrap()
                        .terminal_finality("capture-owner", 7)
                        .into_refusal()
                        .is_some(),
                    "Stop must not mint the missing terminal receipt"
                );
            }
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            assert_released(&recorder);
        }
    }

    #[tokio::test]
    async fn complete_stop_empty_text_is_not_proof_of_silence_or_authority() {
        for has_measurement in [false, true] {
            for text in ["", "unattributed words"] {
                let mut recorder = recorder();
                // Longer than a gesture: the quiet short-take end
                // (`short_capture_without_ledger_speech_ends_cleanly`) must not apply.
                let past_gesture = u64::from(recorder.sample_rate) * 3 / 10 + 1;
                recorder
                    .captured_samples
                    .store(past_gesture, Ordering::Relaxed);
                *recorder.transcript_buffer.lock().await = text.into();
                if has_measurement {
                    recorder.acoustic_ledger =
                        Some(Arc::new(StdMutex::new(stop_finality_ledger(false, true))));
                }
                let result = recorder.complete_stop(Ok(None)).await;
                if has_measurement && text.is_empty() {
                    assert_eq!(result.unwrap(), (String::new(), None));
                } else {
                    assert!(result.unwrap_err().is::<CaptureStopFailure>());
                }
                assert_released(&recorder);
            }
        }
        let mut recorder = recorder();
        recorder.acoustic_ledger =
            Some(Arc::new(StdMutex::new(stop_finality_ledger(false, false))));
        assert!(
            recorder
                .complete_stop(Ok(None))
                .await
                .unwrap_err()
                .is::<TerminalSealRefused>(),
            "empty rendering cannot erase measured speech"
        );
    }

    #[tokio::test]
    async fn complete_stop_foreign_epoch_seal_cannot_authorize_current_capture() {
        let mut recorder = recorder();
        recorder.capture_epoch = 8;
        recorder.acoustic_ledger = Some(Arc::new(StdMutex::new(stop_finality_ledger(true, false))));
        *recorder.transcript_buffer.lock().await = "committed words".into();
        let error = recorder.complete_stop(Ok(None)).await.unwrap_err();
        let refusal = error.downcast_ref::<TerminalSealRefused>().unwrap();
        assert_eq!(refusal.finality.capture_epoch(), 8);
        assert!(refusal.finality.coverage().is_none());
        assert_released(&recorder);
    }

    /// Missing acoustic measurement refuses the terminal transcript on the same
    /// typed path as measured uncovered speech — and the committed words and
    /// the take WAV survive it. A guard pinned to `Incomplete` alone would let
    /// this outcome through as a success and lose the words.
    #[tokio::test]
    async fn unavailable_measurement_refuses_through_the_same_typed_path() {
        use crate::pipeline::acoustic_ledger::AcousticEvidenceGap;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("unavailable.wav");
        let bytes = write_wav(&path);
        let mut recorder = recorder();
        *recorder.transcript_buffer.lock().await = "słowa które przetrwały".to_string();
        let mut ledger = AcousticLedger::new();
        let receipt = SealCoverageReceipt {
            sample_rate_hz: None,
            session_id: "capture-owner".into(),
            capture_epoch: 7,
            speech_samples: 0,
            covered_samples: 0,
            uncovered_speech_ranges: Vec::new(),
            max_uncovered_samples: 0,
            incomplete_threshold_samples: 4_000,
            status: SealCoverageStatus::Unavailable(AcousticEvidenceGap::NotObserved),
            speech_producer: "capture_energy".to_string(),
            availability: "not_observed".to_string(),
            observed_samples: None,
        };
        assert!(ledger.record_seal_coverage(receipt.clone()));
        recorder.acoustic_ledger = Some(Arc::new(StdMutex::new(ledger)));

        let error = recorder
            .complete_stop(Ok(Some(path.clone())))
            .await
            .unwrap_err();
        let refused = error
            .downcast_ref::<TerminalSealRefused>()
            .expect("unavailable measurement is a typed seal refusal");
        assert_eq!(refused.finality.coverage(), Some(&receipt));
        assert!(!refused.finality.coverage().unwrap().status.is_complete());
        assert_eq!(refused.committed_text, "słowa które przetrwały");
        assert_eq!(refused.audio_path.as_ref(), Some(&path));
        assert_eq!(std::fs::read(path).unwrap(), bytes);
        assert_released(&recorder);
    }

    #[tokio::test(start_paused = true)]
    async fn unacknowledged_presentation_cannot_return_an_early_prefix_as_complete() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("retained.wav");
        let bytes = write_wav(&path);
        let mut recorder = recorder();
        recorder.set_event_sink(Some(Arc::new(
            crate::pipeline::sinks::CollectorEventSink::new(),
        )));
        *recorder.transcript_buffer.lock().await = "first".into();
        let buffer = Arc::clone(&recorder.transcript_buffer);
        let delayed = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            // Same byte length: size stability cannot prove publication.
            *buffer.lock().await = "final".into();
        });
        let result = recorder.complete_stop(Ok(Some(path.clone()))).await;
        assert!(
            result.is_err(),
            "unacknowledged publication returned {result:?}"
        );
        let error = result.unwrap_err();
        let failure = error.downcast_ref::<CaptureStopFailure>().unwrap();
        assert_eq!(failure.audio_path.as_deref(), Some(path.as_path()));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert!(
            recorder
                .terminal_take
                .as_ref()
                .is_some_and(|take| take.event_sink.is_some()),
            "unfinished publication must remain owned"
        );
        delayed.await.unwrap();
        assert_eq!(
            &*recorder
                .terminal_take
                .as_ref()
                .unwrap()
                .transcript_buffer
                .lock()
                .await,
            "final"
        );
    }
    #[tokio::test]
    async fn explicit_publication_ack_preserves_a_same_length_terminal_correction() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("take.wav");
        let wav = write_wav(&path);
        let mut recorder = recorder();
        recorder.acoustic_ledger = Some(Arc::new(StdMutex::new(stop_finality_ledger(true, false))));
        *recorder.transcript_buffer.lock().await = "committed worts".into();
        let text = Arc::clone(&recorder.transcript_buffer);
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        recorder.set_event_sink(Some(Arc::new(ControlledTestPresentation {
            entered: entered.clone(),
            release: release.clone(),
        })));
        let audio = path.clone();
        let stopping = tokio::spawn(async move {
            let result = recorder.complete_stop(Ok(Some(audio))).await;
            (recorder, result)
        });
        entered.notified().await;
        assert!(!stopping.is_finished(), "publication is still pending");
        *text.lock().await = "committed words".into();
        release.notify_one();
        let (recorder, result) = stopping.await.unwrap();
        let (committed, saved) = result.unwrap();
        assert_eq!(committed, "committed words");
        assert_eq!(saved.as_deref(), Some(path.as_path()));
        assert_eq!(std::fs::read(path).unwrap(), wav);
        assert_released(&recorder);
    }

    #[tokio::test(start_paused = true)]
    async fn publication_timeout_retains_sink_and_audio_without_success() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("take.wav");
        let wav = write_wav(&path);
        let mut recorder = recorder();
        let entered = Arc::new(tokio::sync::Notify::new());
        recorder.set_event_sink(Some(Arc::new(ControlledTestPresentation {
            entered: entered.clone(),
            release: Arc::new(tokio::sync::Notify::new()),
        })));
        let audio = path.clone();
        let stopping = tokio::spawn(async move {
            let result = recorder.complete_stop(Ok(Some(audio))).await;
            (recorder, result)
        });
        entered.notified().await;
        tokio::time::advance(std::time::Duration::from_millis(3001)).await;
        let (recorder, result) = stopping.await.unwrap();
        let error = result.unwrap_err();
        let failure = error.downcast_ref::<CaptureStopFailure>().unwrap();
        assert!(failure.cause.to_string().contains("timed out"));
        assert_eq!(failure.audio_path.as_deref(), Some(path.as_path()));
        assert!(
            recorder
                .terminal_take
                .as_ref()
                .is_some_and(|take| take.event_sink.is_some()),
            "uncompleted publication remains owned"
        );
        assert_eq!(std::fs::read(path).unwrap(), wav);
    }

    #[tokio::test]
    async fn a_foreign_capture_cannot_acknowledge_terminal_publication() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("take.wav");
        let wav = write_wav(&path);
        let mut recorder = recorder();
        recorder.capture_epoch = 8;
        recorder.set_event_sink(Some(Arc::new(SynchronousTestPresentation)));
        let error = recorder
            .complete_stop(Ok(Some(path.clone())))
            .await
            .unwrap_err();
        let failure = error.downcast_ref::<CaptureStopFailure>().unwrap();
        assert!(failure.cause.to_string().contains("foreign capture"));
        assert_eq!(failure.capture_epoch, 8);
        assert!(
            recorder
                .terminal_take
                .as_ref()
                .is_some_and(|take| take.event_sink.is_some())
        );
        assert_eq!(std::fs::read(path).unwrap(), wav);
    }
}

#[cfg(test)]
mod recorded_replay_tests {
    use super::*;
    use crate::config::{CapturedRuntimeInputs, Config};

    const SESSION: &str = "agent-channel-3-0123456789";

    fn write_mono_archive(path: &std::path::Path, rate: u32, frames: usize) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for index in 0..frames {
            writer
                .write_sample(((index % 200) as i16 - 100) * 50)
                .unwrap();
        }
        writer.finalize().unwrap();
    }

    /// A channel take's card, round-tripped through its JSON form.
    fn channel_card(root: &std::path::Path) -> ReplayCard {
        let snapshot = Arc::new(Config::runtime_snapshot_from_captured(
            CapturedRuntimeInputs::defaults_at(root.to_path_buf(), 7),
        ));
        let (layer1, _) = crate::asr_session::layer1_decision(&snapshot);
        let (_archive_sender, archive) = std::sync::mpsc::channel();
        let config = SessionConfig {
            session_id: SESSION.into(),
            capture_epoch: 1,
            runtime_settings: snapshot,
            live_formatting_agent: None,
            acoustic_ledger: Arc::new(StdMutex::new(AcousticLedger::new())),
            sample_rate: 16_000,
            capture_device_name: Some("MacBook Pro Microphone".into()),
            language: Some("pl".into()),
            stream_log_path: None,
            utterance_silence_sec: Some(2.0),
            capture_turn: CaptureTurnIntent::AgentChannel,
            layer1,
            lifecycle_events: None,
            terminal_audio: Some(archive),
            last_window_closed: None,
        };
        let json = serde_json::to_vec(&ReplayCard::of_session(&config)).unwrap();
        serde_json::from_slice(&json).unwrap()
    }

    #[test]
    fn a_carded_take_replays_with_its_own_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let card = channel_card(dir.path());
        let wav = dir.path().join("take.wav");
        write_mono_archive(&wav, 16_000, 1_600);

        let replay = prepare_recorded_replay(
            Some(SESSION),
            Some(&card),
            &wav,
            2,
            &ReplayScenario::default(),
        )
        .unwrap();

        let config = &replay.config;
        assert_eq!(config.session_id, format!("replay-{SESSION}-r2"));
        assert_eq!(
            config.capture_device_name.as_deref(),
            Some("MacBook Pro Microphone")
        );
        assert_eq!(config.capture_turn, CaptureTurnIntent::AgentChannel);
        assert_eq!(config.language.as_deref(), Some("pl"));
        assert_eq!(config.utterance_silence_sec, Some(2.0));
        assert_eq!(
            config.runtime_settings.digest().as_str(),
            card.settings_digest
        );
        assert_eq!(replay.samples.len(), 1_600);
        let archive = config
            .terminal_audio
            .as_ref()
            .expect("the archive is handed over like a stop")
            .try_recv()
            .unwrap()
            .unwrap();
        assert_eq!(archive.session_id, config.session_id);
        assert_eq!(archive.sample_count, 1_600);
        let diverged: Vec<_> = replay.divergences().map(|input| input.input).collect();
        assert_eq!(diverged, ["engine_env", "data_dir_files"]);
    }

    #[test]
    fn a_take_without_a_card_names_where_every_input_came_from() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("take.wav");
        write_mono_archive(&wav, 16_000, 1_600);

        let replay =
            prepare_recorded_replay(Some(SESSION), None, &wav, 1, &ReplayScenario::default())
                .unwrap();

        assert_eq!(replay.config.capture_turn, CaptureTurnIntent::AgentChannel);
        assert!(replay.config.terminal_audio.is_some());
        for input in &replay.inputs {
            assert!(input.recorded.is_none(), "{}", input.input);
            assert!(input.reason.is_some(), "{} names no source", input.input);
        }
    }

    #[test]
    fn a_scenario_names_every_input_it_changes() {
        let dir = tempfile::tempdir().unwrap();
        let card = channel_card(dir.path());
        let wav = dir.path().join("take.wav");
        write_mono_archive(&wav, 16_000, 1_600);
        let scenario = ReplayScenario {
            capture_device: Some("USB Mic (2)".into()),
            seal_ledger: Some(true),
        };

        let replay =
            prepare_recorded_replay(Some(SESSION), Some(&card), &wav, 1, &scenario).unwrap();

        let config = &replay.config;
        let tag = config
            .session_id
            .strip_prefix(&format!("replay-{SESSION}-lon-d"))
            .and_then(|rest| rest.strip_suffix("-r1"))
            .expect("the scenario tag sits between the take's id and the run");
        assert_eq!(tag.len(), 6);
        assert!(config.runtime_settings.seal_ledger_enabled());
        assert_eq!(config.capture_device_name.as_deref(), Some("USB Mic (2)"));
        let diverged: Vec<_> = replay.divergences().map(|input| input.input).collect();
        assert_eq!(
            diverged,
            [
                "settings_digest",
                "seal_ledger",
                "capture_device",
                "engine_env",
                "data_dir_files"
            ]
        );
        let device = replay
            .inputs
            .iter()
            .find(|input| input.input == "capture_device")
            .unwrap();
        assert!(device.reason.as_deref().unwrap().contains("no profile"));
    }

    #[test]
    fn a_scenario_id_fits_the_trail_limit_for_a_channel_take() {
        let scenario = ReplayScenario {
            capture_device: Some("A very long external USB microphone name".into()),
            seal_ledger: Some(false),
        };
        let session = "agent-channel-0-0b47f573-d80d-4d87-8f26-7faaf85955cc";
        let id = format!("replay-{session}-{}-r20", scenario.session_tag().unwrap());
        assert!(
            crate::pipeline::trail::trail_path(std::path::Path::new("/"), &id).is_ok(),
            "{id}"
        );
    }

    #[test]
    fn a_scenario_that_matches_the_take_is_no_divergence() {
        let dir = tempfile::tempdir().unwrap();
        let card = channel_card(dir.path());
        let wav = dir.path().join("take.wav");
        write_mono_archive(&wav, 16_000, 1_600);
        let scenario = ReplayScenario {
            capture_device: Some("MacBook Pro Microphone".into()),
            seal_ledger: Some(false),
        };

        let replay =
            prepare_recorded_replay(Some(SESSION), Some(&card), &wav, 1, &scenario).unwrap();

        assert_eq!(
            replay.config.runtime_settings.digest().as_str(),
            card.settings_digest
        );
        let diverged: Vec<_> = replay.divergences().map(|input| input.input).collect();
        assert_eq!(diverged, ["engine_env", "data_dir_files"]);
    }
}
