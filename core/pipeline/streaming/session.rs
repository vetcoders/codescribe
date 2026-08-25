//! Event-based transcription session: VAD-fed utterance ingestion, the
//! pipelined Whisper inference loop, boundary/final emission, and the
//! in-memory batch helpers built on the same runtime path.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use anyhow::{Result, anyhow};
use tokio::sync::mpsc;
use tokio::time::Duration;
use tracing::{debug, info, warn};

use crate::asr_session::recorder::{Layer1Decision, RecorderLifecycleEvents};
use crate::config::{Config, RuntimeSettingsSnapshot};
use crate::pipeline::acoustic_ledger::AcousticLedger;
use crate::pipeline::contracts::{EngineEvent, EventSink, LayerSource, LayerSummary};
use crate::stt::tail_patcher::{
    TailPatchConfig, TailPatchOutcome, UnderCommit, compute_tail_patch_with_context, layered_phase,
};
#[cfg(test)]
use crate::stt::tail_provider::{
    TailEvidenceSource, TailEvidenceStability, TailProviderId, TailTimingQuality, TimedTailSegment,
};
use crate::stt::tail_provider::{TailProviderPayload, TailProviderRequest};

/// Maximum audio retained in the Refine correction buffer, in seconds.
///
/// The Refine lane re-transcribes `correction_audio_buf` to correct the recent
/// suffix of an utterance. Without a cap the buffer grows for the whole
/// utterance, so each Refine re-decodes from the very start (O(n) per pass).
/// Bounding it to a trailing window keeps Refine focused on the fresh tail —
/// which is all `strip_overlap` needs — at constant cost. Sized to comfortably
/// exceed the partial-pass cadence so no spoken tail is ever dropped before a
/// Refine consumes it.
#[cfg(any())]
const CORRECTION_WINDOW_SEC: f32 = 0.0;
/// Maximum text retained for Refine's window baseline.
///
/// Text has no exact timestamps here, so keep a conservative character tail
/// that comfortably covers 18s of dense speech while bounding clone/compare
/// work in long sessions.
const CORRECTION_WINDOW_TEXT_MAX_CHARS: usize = 4096;

/// Trim `buf` in place so it retains at most `window_sec` of trailing audio at
/// `sample_rate`. Returns the number of leading samples drained.
#[cfg(any())]
fn cap_correction_buffer(buf: &mut Vec<f32>, sample_rate: u32, window_sec: f32) -> usize {
    let cap = (window_sec * sample_rate as f32) as usize;
    if cap == 0 || buf.len() <= cap {
        return 0;
    }
    let drain_n = buf.len() - cap;
    buf.drain(..drain_n);
    drain_n
}

/// Trim `text` in place to at most `max_chars` **characters**, dropping from
/// the front. Returns how many characters were dropped.
///
/// Counts characters, not bytes: this text is Polish and a byte cut would
/// split a diacritic. Any word fragment left dangling at the new start is
/// trimmed away.
#[cfg(any())]
fn cap_correction_window_text(text: &mut String, max_chars: usize) -> usize {
    let char_count = text.chars().count();
    if max_chars == 0 || char_count <= max_chars {
        return 0;
    }

    let drain_chars = char_count - max_chars;
    let drain_bytes = text
        .char_indices()
        .nth(drain_chars)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len());
    text.drain(..drain_bytes);
    let trimmed = text.trim_start();
    if trimmed.len() != text.len() {
        *text = trimmed.to_string();
    }
    drain_chars
}

/// Append one piece of transcript to the Refine window's text mirror, keeping
/// it capped.
///
/// The mirror tracks `correction_audio_buf`: previews and finals append here as
/// they append there, so the two stay describable as one slice.
#[cfg(any())]
fn append_to_correction_window_text(window_text: &mut String, text: &str, max_chars: usize) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    if !window_text.is_empty() {
        window_text.push(' ');
    }
    window_text.push_str(text);
    cap_correction_window_text(window_text, max_chars);
}

// ── Unified session config ───────────────────────────────────────────────────

/// Configuration for a transcription session.
///
/// No presentation parameters — this is pure engine config.
pub struct SessionConfig {
    /// Controller-minted capture identity. Engines may observe it but may not
    /// replace it with a lane-local UUID.
    pub session_id: String,
    /// Capture clock epoch shared by every observation in this session.
    pub capture_epoch: u64,
    /// One immutable settings read for the entire session.
    pub runtime_settings: Arc<RuntimeSettingsSnapshot>,
    /// The single PCM/evidence/admission owner shared by capture and engines.
    pub acoustic_ledger: Arc<StdMutex<AcousticLedger>>,
    pub sample_rate: u32,
    pub language: Option<String>,
    pub stream_log_path: Option<std::path::PathBuf>,
    /// VAD silence threshold for utterance boundary (None = use default).
    pub utterance_silence_sec: Option<f32>,
    /// Injected, already-authorized Layer 1 refiner decision (C1).
    ///
    /// The pipeline only consumes this — construction, consent, and mode
    /// persistence belong to the settings owner. The decision distinguishes
    /// Apple-only, local exact-span Whisper, and an injected provider.
    pub layer1: Layer1Decision,
    /// Per-recording host lifecycle boundaries. Present only for a live
    /// recorder; buffered/offline helpers have no system observer owner.
    pub lifecycle_events: Option<RecorderLifecycleEvents>,
}

/// What happened to one enqueue attempt.
///
/// `enqueued` and `dropped` are independent: making room for a final by
/// evicting something reports both, and `evicted_final` distinguishes the worst
/// case — a committed boundary was sacrificed — from evicting a replaceable
/// interim.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct EnqueueOutcome {
    pub(crate) enqueued: bool,
    pub(crate) dropped: u64,
    pub(crate) evicted_final: bool,
}

/// Queue one work item for inference, applying the backpressure policy when the
/// queue is full.
///
/// Finals and interims are not equal under pressure. An interim is a draft that
/// a later pass will supersede, so a full queue simply drops an incoming one. A
/// final is a committed utterance boundary, so it always gets in: first by
/// evicting the oldest interim, and only if the queue is all finals by evicting
/// the oldest of those. The invariant is that the *newest* boundary survives —
/// losing it would truncate the transcript at the end, where the user is
/// looking.
pub(crate) fn enqueue_pending_utterance(
    pending: &mut VecDeque<PendingUtteranceWorkItem>,
    item: PendingUtteranceWorkItem,
    max_pending: usize,
) -> EnqueueOutcome {
    if max_pending == 0 {
        return EnqueueOutcome {
            enqueued: false,
            dropped: 1,
            evicted_final: false,
        };
    }

    if pending.len() < max_pending {
        pending.push_back(item);
        return EnqueueOutcome {
            enqueued: true,
            dropped: 0,
            evicted_final: false,
        };
    }

    if !item.is_final {
        return EnqueueOutcome {
            enqueued: false,
            dropped: 1,
            evicted_final: false,
        };
    }

    if let Some(pos) = pending.iter().position(|queued| !queued.is_final) {
        pending.remove(pos);
        pending.push_back(item);
        return EnqueueOutcome {
            enqueued: true,
            dropped: 1,
            evicted_final: false,
        };
    }

    let evicted_final = pending.pop_front().is_some();
    pending.push_back(item);
    EnqueueOutcome {
        enqueued: true,
        dropped: u64::from(evicted_final),
        evicted_final,
    }
}

/// Legacy compatibility override used only to detect an explicitly requested
/// patcher on the unfenced VAD route and refuse it with typed evidence.
///
/// Apple progressive consumes the recording-start [`Layer1Decision`] instead;
/// no live session re-resolves product mode. This remains orthogonal to final
/// pass routing: final-pass off never disables live patching.
pub(super) fn tail_patch_enabled() -> bool {
    layered_phase().is_some_and(|phase| phase >= 1)
}

/// Stable event code carrying the typed local tail-patch session receipt.
pub const TAIL_PATCH_SESSION_RECEIPT_WARNING_CODE: &str = "tail_patch_session_receipt";

/// Final stop-drain disposition for the local Whisper lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TailPatchDrainDisposition {
    /// Local tail patching was not armed for this recording.
    NotArmed,
    /// Every submitted job reached a terminal disposition before seal.
    Completed,
    /// The bounded drain expired with admitted work still outstanding.
    TimedOut,
    /// Admitted work was lost for a non-timeout reason (for example worker
    /// failure) before reaching a terminal patch verdict.
    Abandoned,
}

impl TailPatchDrainDisposition {
    fn as_token(self) -> &'static str {
        match self {
            Self::NotArmed => "not_armed",
            Self::Completed => "completed",
            Self::TimedOut => "timed_out",
            Self::Abandoned => "abandoned",
        }
    }

    fn from_token(token: &str) -> Option<Self> {
        match token {
            "not_armed" => Some(Self::NotArmed),
            "completed" => Some(Self::Completed),
            "timed_out" => Some(Self::TimedOut),
            "abandoned" => Some(Self::Abandoned),
            _ => None,
        }
    }
}

/// Content-free proof of local Whisper arming, work admission, application,
/// and bounded stop drainage for one recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TailPatchSessionReceipt {
    pub armed: bool,
    pub submitted: u64,
    pub applied: u64,
    pub skipped: u64,
    /// Jobs whose terminal disposition is bounded stop-drain expiry.
    pub timed_out: u64,
    /// Jobs discarded for a non-timeout reason after admission.
    pub abandoned: u64,
    pub drain: TailPatchDrainDisposition,
}

impl TailPatchSessionReceipt {
    /// Construct a receipt. The caller owns counter provenance; this type owns
    /// the invariant checks and stable event encoding.
    pub fn new(
        armed: bool,
        submitted: u64,
        applied: u64,
        skipped: u64,
        timed_out: u64,
        abandoned: u64,
        drain: TailPatchDrainDisposition,
    ) -> Self {
        let receipt = Self {
            armed,
            submitted,
            applied,
            skipped,
            timed_out,
            abandoned,
            drain,
        };
        assert!(
            receipt.is_reconciled(),
            "tail-patch terminal buckets must reconcile exactly to submitted jobs"
        );
        receipt
    }

    /// Build the production stop receipt. Every job still outstanding after
    /// the worker's real bounded closure loop is classified as timed out.
    /// `abandoned` is reserved for a distinct non-timeout discard path.
    pub fn from_stop(
        armed: bool,
        submitted: u64,
        applied: u64,
        skipped: u64,
        timeout_residue: u64,
    ) -> Self {
        Self::new(
            armed,
            submitted,
            applied,
            skipped,
            timeout_residue,
            0,
            if !armed {
                TailPatchDrainDisposition::NotArmed
            } else if timeout_residue > 0 {
                TailPatchDrainDisposition::TimedOut
            } else {
                TailPatchDrainDisposition::Completed
            },
        )
    }

    /// An armed lane that submitted no work is a failed runtime witness, not
    /// proof that Layered worked.
    pub fn armed_without_submissions(self) -> bool {
        self.armed && self.submitted == 0
    }

    /// Whether every submitted job has exactly one terminal bucket.
    pub fn is_reconciled(self) -> bool {
        self.applied
            .saturating_add(self.skipped)
            .saturating_add(self.timed_out)
            .saturating_add(self.abandoned)
            == self.submitted
    }

    pub(crate) fn as_event(self) -> EngineEvent {
        EngineEvent::Warning {
            code: TAIL_PATCH_SESSION_RECEIPT_WARNING_CODE.to_string(),
            message: format!(
                "armed={} submitted={} applied={} skipped={} timed_out={} abandoned={} drain={}",
                self.armed,
                self.submitted,
                self.applied,
                self.skipped,
                self.timed_out,
                self.abandoned,
                self.drain.as_token(),
            ),
        }
    }

    /// Recover the typed receipt from the production ordered event evidence.
    pub fn from_events(events: &[EngineEvent]) -> Option<Self> {
        let message = events.iter().rev().find_map(|event| match event {
            EngineEvent::Warning { code, message }
                if code == TAIL_PATCH_SESSION_RECEIPT_WARNING_CODE =>
            {
                Some(message.as_str())
            }
            _ => None,
        })?;
        let fields = message
            .split_whitespace()
            .filter_map(|field| field.split_once('='))
            .collect::<std::collections::BTreeMap<_, _>>();
        let receipt = Self {
            armed: fields.get("armed")?.parse().ok()?,
            submitted: fields.get("submitted")?.parse().ok()?,
            applied: fields.get("applied")?.parse().ok()?,
            skipped: fields.get("skipped")?.parse().ok()?,
            timed_out: fields.get("timed_out")?.parse().ok()?,
            abandoned: fields.get("abandoned")?.parse().ok()?,
            drain: TailPatchDrainDisposition::from_token(fields.get("drain")?)?,
        };
        receipt.is_reconciled().then_some(receipt)
    }
}

/// Re-transcribe a sealed utterance's audio and diff it against the text
/// already committed, producing Layer 1 patch events.
///
/// Runs the Whisper pass on a blocking worker so the session loop keeps
/// draining. `committed_text` must be the exact string that was emitted as
/// `UtteranceFinal.text`: the resulting `ReplaceRange` offsets are computed
/// against it, so a differently-trimmed copy would produce patches that land at
/// the wrong characters. The debug assertion pins that contract in test builds.
#[derive(Debug)]
pub(super) struct TailPatchJobResult {
    pub utterance_id: u64,
    pub outcome: TailPatchOutcome,
    pub payload: TailProviderPayload,
}

impl TailPatchJobResult {
    pub fn into_outcome(self) -> (u64, TailPatchOutcome) {
        (self.utterance_id, self.outcome)
    }
}

pub(super) async fn compute_tail_patch_job(
    utterance_id: u64,
    committed_text: String,
    neighbour_context: String,
    audio: Vec<f32>,
    request: TailProviderRequest,
    config: TailPatchConfig,
) -> Result<TailPatchJobResult> {
    compute_tail_patch_job_with(
        utterance_id,
        committed_text,
        neighbour_context,
        audio,
        request,
        config,
        crate::stt::tail_provider::transcribe_configured,
    )
    .await
}

async fn compute_tail_patch_job_with<F>(
    utterance_id: u64,
    committed_text: String,
    neighbour_context: String,
    audio: Vec<f32>,
    request: TailProviderRequest,
    config: TailPatchConfig,
    transcribe: F,
) -> Result<TailPatchJobResult>
where
    F: FnOnce(&TailProviderRequest, &[f32]) -> Result<TailProviderPayload> + Send + 'static,
{
    debug_assert_eq!(
        committed_text.trim(),
        committed_text,
        "tail-patch committed_text must be the exact, pre-trimmed UtteranceFinal text \
         (single trim owner: final_text at the emit site)"
    );
    tokio::task::spawn_blocking(move || {
        let payload = transcribe(&request, &audio)?;
        let outcome = compute_tail_patch_with_context(
            &committed_text,
            &payload.text,
            &neighbour_context,
            utterance_id,
            &config,
        );
        Ok(TailPatchJobResult {
            utterance_id,
            outcome,
            payload,
        })
    })
    .await
    .map_err(|e| anyhow!("tail patch worker task failed: {e}"))?
}

/// Stable engine-event code carrying a Layer-1 under-commit outward.
///
/// The stop path keys on this to require residual gap fill for a session whose
/// live canvas is known to be starved, independently of the committed-density
/// floor that judges the same session from the audio side.
pub const UNDER_COMMIT_WARNING_CODE: &str = "tail_patch_under_commit";

/// The legacy VAD/scheduler route has no pending-span rewrite fence. Explicit
/// Layer 1 requests fail closed here instead of mutating an emitted final.
pub const TAIL_PATCH_ROUTE_UNBOUND_WARNING_CODE: &str = "tail_patch_route_unbound";

/// Build the outward escalation for an under-commit that could not be placed
/// live.
///
/// Counts only — the message crosses the IPC boundary and reaches the log, and
/// the transcript is the user's speech. A `Warning` rather than a new event
/// variant on purpose: every sink, the IPC wire and the Swift bridge already
/// carry it, so the escalation costs no FFI surface.
fn under_commit_warning(under: &UnderCommit) -> EngineEvent {
    EngineEvent::Warning {
        code: UNDER_COMMIT_WARNING_CODE.to_string(),
        message: format!(
            "residual gap fill required: committed_chars={} retranscribed_chars={} \
             committed_tokens={} retranscribed_tokens={} commit_ratio={:.2} gap_appends={}",
            under.committed_chars,
            under.retranscribed_chars,
            under.committed_tokens,
            under.retranscribed_tokens,
            under.commit_ratio,
            under.appends.len(),
        ),
    }
}

/// Forward a tail-patch job's outcome to the sink and report how many
/// replacements were emitted.
///
/// Only `ReplaceRange` events from the tail-patch layer are counted, so the
/// session's layer summary stays attributable. A failed job is not fatal: the
/// Layer 0 committed text is already correct enough to keep, so the error
/// becomes a warning event and the count stays zero.
pub(super) fn emit_tail_patch_result(
    event_sink: &dyn EventSink,
    result: Result<(u64, TailPatchOutcome)>,
) -> u64 {
    match result {
        Ok((utterance_id, TailPatchOutcome::Patches(events))) => {
            let mut emitted = 0u64;
            for event in events {
                if matches!(
                    event,
                    EngineEvent::ReplaceRange {
                        source: LayerSource::TailPatch,
                        ..
                    }
                ) {
                    emitted = emitted.saturating_add(1);
                }
                event_sink.on_event(&event);
            }
            debug!(utterance_id, emitted, "Applied tail patch replacements");
            emitted
        }
        Ok((utterance_id, TailPatchOutcome::NoChange)) => {
            debug!(utterance_id, "Tail patch found no changes");
            0
        }
        Ok((utterance_id, TailPatchOutcome::UnderCommit(under))) => {
            let mut emitted = 0u64;
            for event in &under.appends {
                if matches!(
                    event,
                    EngineEvent::ReplaceRange {
                        source: LayerSource::TailPatch,
                        ..
                    }
                ) {
                    emitted = emitted.saturating_add(1);
                }
                event_sink.on_event(event);
            }
            info!(
                utterance_id,
                reason = under.reason(),
                committed_chars = under.committed_chars,
                retranscribed_chars = under.retranscribed_chars,
                committed_tokens = under.committed_tokens,
                retranscribed_tokens = under.retranscribed_tokens,
                gap_appends = emitted,
                residual_required = under.residual_required,
                "Layer 1 under-commit"
            );
            if under.residual_required {
                event_sink.on_event(&under_commit_warning(&under));
            }
            emitted
        }
        Ok((utterance_id, TailPatchOutcome::Skipped { code, reason })) => {
            // INFO, not debug: a skipped patch is text Whisper had in hand and
            // the canvas never received. The counts belong to the receipt the
            // patcher already logs; this line proves the sink saw the same
            // verdict for this utterance.
            info!(
                utterance_id,
                code = code.as_str(),
                reason,
                "Tail patch skipped"
            );
            0
        }
        Err(e) => {
            warn!("Tail patch failed; keeping Layer 0 committed text: {}", e);
            event_sink.on_event(&EngineEvent::Warning {
                code: "tail_patch_error".to_string(),
                message: format!("{}", e),
            });
            0
        }
    }
}

/// Emit the session's closing event with its layer accounting.
///
/// Only the tail-patch count is populated here — the other layers are applied
/// outside this session path, so reporting zeros for them is honest rather
/// than incomplete.
pub(super) fn emit_session_finalised(
    event_sink: &dyn EventSink,
    session_id: String,
    tail_patch_replacements: u64,
) {
    event_sink.on_event(&EngineEvent::SessionFinalised {
        session_id,
        layer_summary: LayerSummary {
            tail_patch_replacements,
            ..LayerSummary::default()
        },
    });
}

/// Per-session skip count at which a zero-application session is an alarm.
///
/// One or two skips with nothing applied can be honest divergence (noise, a
/// throat-clear window). Three computed corrections all rejected is the gate
/// eating the lane's entire output — the 2026-08-12 audit found 116 skips and
/// 0 applied patches across the log's whole history, and not one line said so
/// out loud.
pub(super) const TAIL_PATCH_STARVED_MIN_SKIPS: u64 = 3;

/// Whether this session's Layer 1 lane was starved: corrections were computed
/// and every single one was rejected.
pub(super) fn tail_patch_lane_starved(applied: u64, skipped: u64) -> bool {
    applied == 0 && skipped >= TAIL_PATCH_STARVED_MIN_SKIPS
}

/// One session-level receipt for the Layer 1 lane, emitted at finalise.
///
/// The per-utterance skip receipts diagnose a single verdict; this line
/// diagnoses the lane. A starved session — Whisper burned inference on every
/// sealed utterance and the canvas received none of it — is a WARN, because
/// that is the lane not doing its one job, silently.
pub(super) fn log_tail_patch_session_receipt(receipt: TailPatchSessionReceipt) {
    if receipt.timed_out > 0 || receipt.abandoned > 0 {
        warn!(
            armed = receipt.armed,
            submitted = receipt.submitted,
            applied = receipt.applied,
            skipped = receipt.skipped,
            timed_out = receipt.timed_out,
            abandoned = receipt.abandoned,
            drain = receipt.drain.as_token(),
            "tail_patch_session_degraded: accepted work missed the bounded stop drain"
        );
    } else if receipt.armed_without_submissions() {
        warn!(
            armed = receipt.armed,
            submitted = receipt.submitted,
            drain = receipt.drain.as_token(),
            "tail_patch_lane_unexercised: armed session submitted zero Whisper windows"
        );
    } else if tail_patch_lane_starved(receipt.applied, receipt.skipped) {
        warn!(
            applied = receipt.applied,
            skipped = receipt.skipped,
            "tail_patch_lane_starved: every computed Whisper correction this session was rejected"
        );
    } else {
        info!(
            armed = receipt.armed,
            submitted = receipt.submitted,
            applied = receipt.applied,
            skipped = receipt.skipped,
            timed_out = receipt.timed_out,
            abandoned = receipt.abandoned,
            drain = receipt.drain.as_token(),
            "tail_patch_session_receipt"
        );
    }
}

// ── Unified transcription session (event-based) ─────────────────────────────

/// Unified transcription session exposed as a single event-emitting pipeline.
///
/// The engine processes audio → VAD → Whisper → PostProcess and emits
/// `EngineEvent`s. No presentation logic (typing animation, buffer delay,
/// etc.) — that's the consumer's responsibility.
///
/// When the active STT engine is Apple and progressive stream mode is on
/// (default; escape hatch `CODESCRIBE_APPLE_STT_LIVE_MODE=wav`), the session
/// takes the system-dictation path: one long-lived SFSpeech stream whose
/// phrase-level `isFinal` events become multi-seal `UtteranceFinal`s. That is
/// the CORE ENGINE freezed+append contract — not a Whisper hybrid mid-live.
///
/// Local Layer 1 tail-patch is deliberately Apple-progressive-only: that path
/// owns the exact pending-span rewrite fence. The VAD/scheduler path already
/// runs Whisper as its primary engine and refuses any second, unbound mutation
/// lane with typed evidence. Smart/final-pass routing stays orthogonal.
pub(crate) async fn transcription_session(
    chunk_receiver: mpsc::Receiver<Vec<f32>>,
    event_sink: Arc<dyn EventSink>,
    config: SessionConfig,
) {
    super::apple_live_session::apple_stream_transcription_session(
        chunk_receiver,
        event_sink,
        config,
    )
    .await;
}

/// Legacy VAD + per-window scheduler session body. Tests that assert the
/// VAD-path contract (`vad_no_speech_detected`, final Stats) target this
/// directly: routing in [`transcription_session`] reads process-global engine
/// state, which sibling engine-selection tests mutate via `set_var` — going
/// through the router makes the contract dependent on test scheduling.
#[cfg(any())]
pub(crate) async fn vad_transcription_session(
    mut chunk_receiver: mpsc::Receiver<Vec<f32>>,
    event_sink: Arc<dyn EventSink>,
    config: SessionConfig,
) {
    let SessionConfig {
        sample_rate,
        language,
        stream_log_path,
        utterance_silence_sec,
        layer1,
        lifecycle_events: _,
    } = config;

    let local_tail_patch_requested = layer1
        .local_tail_patch_disposition()
        .is_some_and(|disposition| disposition.is_armed());

    // C1 wires the live provider Layer 1 lane on the Apple progressive path only. On
    // this canvas an armed decision is disarmed explicitly: a refiner that
    // cannot run is a missing improvement, never an error, and never a reason
    // to load anything heavier.
    if layer1.is_provider_armed() {
        warn!(
            "Layer 1 live lane is not wired on the VAD/scheduler path; \
             proceeding canvas + lexicon"
        );
    }
    drop(layer1);

    info!("Transcription session started (event-based pipeline)");
    let session_id = uuid::Uuid::new_v4().to_string();
    let mut capture_level = CaptureLevelAccumulator::new();
    begin_session_energy_clock();

    let mut session = if let Some(sec) = utterance_silence_sec {
        SpeechSession::new_utterance_with_silence(sample_rate, sec)
    } else {
        SpeechSession::new_utterance(sample_rate)
    };
    let output_sample_rate = session.output_sample_rate();
    let stt_scheduler = SttScheduler::new();
    // This route emits its final before async tail work can complete and owns
    // no pending-span fence. Until it adopts the progressive seal owner, an
    // explicit Layer 1 request is evidence only — never post-final mutation.
    let tail_patch_requested = local_tail_patch_requested || tail_patch_enabled();
    let tail_patch_enabled = false;
    let tail_patch_config = TailPatchConfig::from_env();
    if tail_patch_requested {
        warn!(
            phase = layered_phase().unwrap_or(0),
            "Layered transcription refused on VAD session path without a pending-span fence"
        );
        event_sink.on_event(&EngineEvent::Warning {
            code: TAIL_PATCH_ROUTE_UNBOUND_WARNING_CODE.to_string(),
            message: "VAD/scheduler Layer 1 has no exact pending-span rewrite fence; primary text preserved"
                .to_string(),
        });
    }

    let mut pipeline = TranscriptionPipeline::new(language);
    let mut preview_rev: u64 = 0;
    let mut utterance_id: u64 = 0;
    let mut scheduler_utterance_id: u64 = 1;
    let mut total_utterances: u64 = 0;
    let mut filtered_empty_drops: u64 = 0;
    let mut corrections_applied: u64 = 0;
    let mut tail_patch_replacements: u64 = 0;
    let mut tail_patch_skips: u64 = 0;
    let mut partial_telemetry = PartialPassTelemetry::default();
    let mut vad_started = false;
    let mut speech_activity_observed = false;

    // Accumulate text for the current "run" of utterances (between corrections).
    let mut accumulated_text = String::new();
    // Track last raw Whisper output for final flush UtteranceFinal.
    let mut last_raw_text = String::new();
    let mut last_segments: Vec<TranscriptSegment> = Vec::new();
    // Track per-utterance confidence metadata for UtteranceFinal.
    let mut utterance_vad_speech_samples: u64 = 0;
    let mut utterance_avg_logprob: Option<f32> = None;
    let mut utterance_compression_ratio: Option<f32> = None;
    // Accumulate segment timestamps for the current utterance across interim slices.
    let mut utterance_segments: Vec<TranscriptSegment> = Vec::new();

    // Track audio position for UtteranceFinal timestamps (seconds).
    let mut utterance_start_s: f32 = 0.0;
    let mut utterance_audio_samples: usize = 0;

    // Phase 2 correction state
    let mut correction_audio_buf: Vec<f32> = Vec::new();
    let mut rolling_correction_window = RollingCorrectionWindow::default();
    let mut partial_trigger_state = PartialPassTriggerState::new(Instant::now());
    let mut suffix_snapshot = String::new();

    // Fix A: Snapshot pipeline.last_suffix at utterance boundary so FINAL
    // compares against previous utterance's tail, not intermediate non-final
    // chunk suffixes that advanced during Phase 1 preview processing.
    let mut utterance_boundary_suffix = String::new();

    // Fix D: Speech-window-scoped text and boundary revision for partial-pass stale guard.
    // window_text mirrors correction_audio_buf in lockstep: previews append to
    // both, and schedule_partial_pass takes (and clears) both together — so
    // correction_expected_text always describes exactly the audio slice that a
    // Refine pass re-decodes, which is what lets merge_corrected_window splice
    // the correction into accumulated_text instead of replacing it wholesale.
    let mut window_text = String::new();
    let mut boundary_rev: u64 = 0;

    // Decouple audio ingestion from Whisper inference.
    /// Cap on queued utterance work items between audio ingest and Whisper inference.
    const MAX_PENDING_UTTERANCES: usize = 64;
    let mut pending_utterances: VecDeque<PendingUtteranceWorkItem> = VecDeque::new();
    let mut dropped_utterances: u64 = 0;
    let mut audio_closed = false;
    // Full utterance audio buffer used for per-utterance commit requests.
    // Live slices still drive preview; final commit re-transcribes the utterance.
    // Scheduler enforces unconditional Commit-lane VAD prefilter before inference.
    let mut current_utterance_audio: Vec<f32> = Vec::new();

    // VAD-first accumulation buffer: collects interim audio chunks and only
    // submits to Whisper after running extract_speech on the accumulated buffer.
    // This eliminates hallucinations by never feeding silence to Whisper.
    let interim_vad_threshold = interim_vad_accumulate_samples(output_sample_rate);
    let mut interim_vad_buf: Vec<f32> = Vec::with_capacity(interim_vad_threshold);
    let mut interim_vad_speech_samples: u64 = 0;
    debug!(
        interim_vad_sec = interim_vad_threshold as f32 / output_sample_rate as f32,
        "VAD-first accumulation configured"
    );

    // Phase 1 (streaming preview/commit) — Pipelined execution using FuturesOrdered.
    // This allows submitting multiple chunks to the Scheduler (up to concurrency limit)
    // to utilize the worker queue and avoid backpressure on the VAD/Audio thread.
    // Results are guaranteed to be returned in submission order.
    let max_inference_concurrency = inference_max_concurrency();
    debug!(
        max_inference_concurrency,
        "Phase 1 inference pipeline configured"
    );
    let mut inference_pipeline = FuturesOrdered::new();
    let mut tail_patch_pipeline: FuturesUnordered<
        futures_util::future::BoxFuture<'static, Result<TailPatchJobResult>>,
    > = FuturesUnordered::new();

    // Phase 2 (buffered correction) — request tracked for stale guards.
    let mut correction_in_flight: Option<SttTaskHandle> = None;
    let mut correction_expected_window_id: Option<u64> = None;
    let mut correction_expected_boundary_rev: Option<u64> = None;
    let mut correction_expected_text: Option<String> = None;
    let mut correction_suffix_snapshot: Option<String> = None;
    let mut previous_window_prompt: Option<String> = None;

    loop {
        // ── Fill the Pipe ────────────────────────────────────────────────────
        // Drain pending utterances into the scheduler up to the concurrency limit.
        // This decouples ingestion (Supervisor) from inference (Whisper).
        while inference_pipeline.len() < max_inference_concurrency {
            let Some(item) = pending_utterances.pop_front() else {
                break;
            };
            let PendingUtteranceWorkItem {
                audio,
                gate_audio_samples,
                inference_audio,
                is_final,
                scheduler_utterance_id: work_utterance_id,
                max_speech_prob,
                speech_vad_samples,
            } = item;

            if should_drop_short_utterance(gate_audio_samples, output_sample_rate, max_speech_prob)
            {
                pipeline.hallucination_drops = pipeline.hallucination_drops.saturating_add(1);
                event_sink.on_event(&EngineEvent::Drop {
                    kind: DropKind::Hallucination,
                    text: String::new(),
                    reason: format!(
                        "Short utterance dropped: {:.3}s with low VAD prob {:.2}",
                        gate_audio_samples as f32 / output_sample_rate as f32,
                        max_speech_prob
                    ),
                });
                continue;
            }

            // Categorical speech-ratio gate (Silero as binary SoTA classifier).
            // Interim chunks with insufficient speech are pure silence — skip
            // Whisper inference entirely to prevent hallucinations.
            if should_drop_silence_chunk(
                gate_audio_samples,
                output_sample_rate,
                speech_vad_samples,
                is_final,
            ) {
                let audio_16k = (gate_audio_samples as f64 * f64::from(vad::VAD_SAMPLE_RATE)
                    / f64::from(output_sample_rate)) as u64;
                let ratio = if audio_16k > 0 {
                    speech_vad_samples as f32 / audio_16k as f32
                } else {
                    0.0
                };
                debug!(
                    "Silence gate: dropping {:.3}s chunk (speech_ratio={:.1}%, vad_samples={}, threshold={:.0}%)",
                    gate_audio_samples as f32 / output_sample_rate as f32,
                    ratio * 100.0,
                    speech_vad_samples,
                    MIN_SPEECH_RATIO_FOR_INFERENCE * 100.0,
                );
                pipeline.hallucination_drops = pipeline.hallucination_drops.saturating_add(1);
                event_sink.on_event(&EngineEvent::Drop {
                    kind: DropKind::Hallucination,
                    text: String::new(),
                    reason: format!(
                        "Silence chunk dropped: speech_ratio={:.1}% < {:.0}% in {:.3}s",
                        ratio * 100.0,
                        MIN_SPEECH_RATIO_FOR_INFERENCE * 100.0,
                        gate_audio_samples as f32 / output_sample_rate as f32,
                    ),
                });
                continue;
            }

            let lang = pipeline.language.clone();
            let lane = if is_final {
                SttLane::Commit
            } else {
                SttLane::Live
            };
            let item = UtteranceWorkItem {
                audio,
                inference_audio_len: inference_audio.len(),
                is_final,
                tail_patch_audio: if is_final && tail_patch_enabled {
                    Some(inference_audio.clone())
                } else {
                    None
                },
                speech_vad_samples,
            };

            match stt_scheduler.submit_for_utterance(
                lane,
                inference_audio,
                output_sample_rate,
                lang,
                work_utterance_id,
            ) {
                Ok(mut handle) => {
                    // Wrap the handle and item into a future for FuturesOrdered.
                    // This preserves the item context (is_final, audio len) for the result.
                    inference_pipeline.push_back(async move {
                        let res = handle.recv().await;
                        (res, item)
                    });
                }
                Err(e) => {
                    error!("Failed to submit STT request to scheduler: {}", e);
                    event_sink.on_event(&EngineEvent::Warning {
                        code: "scheduler_submit_error".to_string(),
                        message: format!("{}", e),
                    });
                    // If submission fails, we break the fill loop.
                    // The item is lost (popped), but if the scheduler is broken, we have bigger problems.
                    break;
                }
            }
        }

        if correction_in_flight.is_none() && !correction_audio_buf.is_empty() {
            let now = Instant::now();
            let trigger_flags = partial_trigger_state.evaluate(now);
            if let Some(trigger) = classify_partial_trigger(trigger_flags)
                && schedule_partial_pass(
                    &stt_scheduler,
                    output_sample_rate,
                    pipeline.language.clone(),
                    &mut correction_audio_buf,
                    &mut rolling_correction_window,
                    &mut correction_in_flight,
                    &mut correction_expected_window_id,
                    &mut correction_expected_boundary_rev,
                    &mut correction_expected_text,
                    &mut correction_suffix_snapshot,
                    &suffix_snapshot,
                    boundary_rev,
                    &window_text,
                    previous_window_prompt.clone(),
                    partial_trigger_state.silero_speech_ms_since_partial,
                    trigger,
                    &mut partial_telemetry,
                    &event_sink,
                )
            {
                partial_trigger_state.reset_after_success(now);
            }
        }

        // If audio is closed and there is no work left, finish.
        if audio_closed
            && pending_utterances.is_empty()
            && inference_pipeline.is_empty()
            && correction_in_flight.is_none()
            && tail_patch_pipeline.is_empty()
        {
            break;
        }

        tokio::select! {
            maybe_data = chunk_receiver.recv(), if !audio_closed => {
                match maybe_data {
                    Some(data) => {
                        capture_level.push_samples(&data);
                        let speech_events = session.feed(&data, sample_rate);
                        // This legacy session has no Silero sideband consumer.
                        // Drain observations on every callback so a long take
                        // cannot retain one VecDeque entry per VAD edge.
                        drop(session.take_vad_boundaries());
                        for event in speech_events {
                            let speech_vad_samples = session.take_event_speech_vad_samples();
                            let max_speech_prob = session.segment_speech_prob();
                            match event {
                                SpeechEvent::Utterance(u) => {
                                    current_utterance_audio.extend_from_slice(&u);
                                    interim_vad_buf.extend_from_slice(&u);
                                    interim_vad_speech_samples += speech_vad_samples;
                                    speech_activity_observed = true;

                                    if !vad_started {
                                        event_sink.on_event(&EngineEvent::VadStart {
                                            speech_prob: session.boundary_prob(),
                                            ts_ms: session.session_elapsed_ms(),
                                        });
                                        vad_started = true;
                                    }

                                    // Accumulate until threshold, then extract_speech + submit.
                                    if interim_vad_buf.len() >= interim_vad_threshold {
                                        let buf = std::mem::take(&mut interim_vad_buf);
                                        let buf_vad = interim_vad_speech_samples;
                                        interim_vad_speech_samples = 0;
                                        let buf_len = buf.len();
                                        let (speech, stats) = vad::extract_speech(&buf, output_sample_rate);
                                        if speech.is_empty() {
                                            debug!(
                                                "VAD-first: dropping {:.1}s accumulated buffer (0% speech, {} windows)",
                                                buf_len as f32 / output_sample_rate as f32,
                                                stats.total_windows,
                                            );
                                            pipeline.hallucination_drops = pipeline.hallucination_drops.saturating_add(1);
                                            event_sink.on_event(&EngineEvent::Drop {
                                                kind: DropKind::Hallucination,
                                                text: String::new(),
                                                reason: format!(
                                                    "VAD-first: no speech in {:.1}s buffer ({} windows analysed)",
                                                    buf_len as f32 / output_sample_rate as f32,
                                                    stats.total_windows,
                                                ),
                                            });
                                            continue;
                                        }
                                        debug!(
                                            "VAD-first: {:.1}s speech / {:.1}s buffer ({:.0}% speech, {}/{} windows)",
                                            speech.len() as f32 / output_sample_rate as f32,
                                            buf_len as f32 / output_sample_rate as f32,
                                            stats.speech_pct,
                                            stats.speech_windows,
                                            stats.total_windows,
                                        );
                                        let outcome = enqueue_pending_utterance(
                                            &mut pending_utterances,
                                            PendingUtteranceWorkItem {
                                                audio: buf,
                                                gate_audio_samples: buf_len,
                                                inference_audio: speech,
                                                is_final: false,
                                                scheduler_utterance_id,
                                                max_speech_prob,
                                                speech_vad_samples: buf_vad,
                                            },
                                            MAX_PENDING_UTTERANCES,
                                        );
                                        if outcome.dropped > 0 {
                                            dropped_utterances = dropped_utterances.saturating_add(outcome.dropped);
                                            warn!(
                                                queue_len = pending_utterances.len(),
                                                enqueued = outcome.enqueued,
                                                dropped = outcome.dropped,
                                                "Pending utterance backpressure (interim VAD-first)"
                                            );
                                        }
                                    }
                                }
                                SpeechEvent::UtteranceFinal(u) => {
                                    current_utterance_audio.extend_from_slice(&u);
                                    // Flush any accumulated interim audio + this final chunk
                                    // into a single Commit-lane request (extract_speech in prefilter).
                                    let full = std::mem::take(&mut current_utterance_audio);
                                    // Accounting tail: sub-threshold interim residue + this
                                    // final chunk — the only samples no interim work item has
                                    // delivered yet. Carrying `full` as the item audio would
                                    // double-count duration/VAD stats and re-feed the Refine
                                    // buffer with audio it already holds.
                                    let mut tail = std::mem::take(&mut interim_vad_buf);
                                    tail.extend_from_slice(&u);
                                    interim_vad_speech_samples = 0;
                                    speech_activity_observed = true;

                                    if !vad_started {
                                        event_sink.on_event(&EngineEvent::VadStart {
                                            speech_prob: session.boundary_prob(),
                                            ts_ms: session.session_elapsed_ms(),
                                        });
                                        vad_started = true;
                                    }

                                    // Gates (short-utterance duration) must measure the *full*
                                    // sealed segment, not only the trailing silence-pad slice
                                    // `u` — hence gate_audio_samples, while `audio` stays the
                                    // not-yet-accounted tail.
                                    let outcome = enqueue_pending_utterance(
                                        &mut pending_utterances,
                                        PendingUtteranceWorkItem {
                                            audio: tail,
                                            gate_audio_samples: full.len(),
                                            inference_audio: full,
                                            is_final: true,
                                            scheduler_utterance_id,
                                            max_speech_prob,
                                            speech_vad_samples,
                                        },
                                        MAX_PENDING_UTTERANCES,
                                    );
                                    scheduler_utterance_id =
                                        scheduler_utterance_id.saturating_add(1);
                                    if outcome.dropped > 0 {
                                        dropped_utterances = dropped_utterances.saturating_add(outcome.dropped);
                                        let message = if outcome.enqueued {
                                            if outcome.evicted_final {
                                                format!(
                                                    "Pending utterance queue full (limit={}): evicted an older final item to preserve latest final boundary",
                                                    MAX_PENDING_UTTERANCES
                                                )
                                            } else {
                                                format!(
                                                    "Pending utterance queue full (limit={}): evicted a non-final item to preserve latest final boundary",
                                                    MAX_PENDING_UTTERANCES
                                                )
                                            }
                                        } else {
                                            format!(
                                                "Pending utterance queue full (limit={}): dropped incoming non-final item",
                                                MAX_PENDING_UTTERANCES
                                            )
                                        };
                                        warn!(
                                            queue_len = pending_utterances.len(),
                                            is_final = true,
                                            enqueued = outcome.enqueued,
                                            evicted_final = outcome.evicted_final,
                                            dropped = outcome.dropped,
                                            "{}",
                                            message
                                        );
                                        event_sink.on_event(&EngineEvent::Warning {
                                            code: "pending_utterance_backpressure".to_string(),
                                            message,
                                        });
                                    }
                                }
                                _ => continue,
                            };
                        }
                        emit_vad_warning(&event_sink, &mut session);
                    }
                    None => {
                        audio_closed = true;
                        // Sub-threshold interim residue was never enqueued; it still
                        // belongs to this utterance's accounting tail (its samples also
                        // sit in current_utterance_audio, which feeds the Commit lane).
                        let mut flush_tail = std::mem::take(&mut interim_vad_buf);
                        interim_vad_speech_samples = 0;

                        if let Some(event) = session.flush() {
                            let speech_vad_samples = session.take_event_speech_vad_samples();
                            let max_speech_prob = session.segment_speech_prob();
                            // On flush, always treat as final (Commit lane with extract_speech).
                            let (had_flush_audio, inference_audio) = match event {
                                SpeechEvent::Utterance(u) | SpeechEvent::UtteranceFinal(u) => {
                                    current_utterance_audio.extend_from_slice(&u);
                                    let full = std::mem::take(&mut current_utterance_audio);
                                    flush_tail.extend_from_slice(&u);
                                    (!u.is_empty(), full)
                                }
                                _ => (false, Vec::new()),
                            };

                            if had_flush_audio {
                                speech_activity_observed = true;
                                if !vad_started {
                                    event_sink.on_event(&EngineEvent::VadStart {
                                        speech_prob: session.boundary_prob(),
                                        ts_ms: session.session_elapsed_ms(),
                                    });
                                    vad_started = true;
                                }
                                let outcome = enqueue_pending_utterance(
                                    &mut pending_utterances,
                                    PendingUtteranceWorkItem {
                                        // Same contract as the toggle-final: accounting
                                        // tail in `audio`, full sealed segment behind the
                                        // gates and the Commit-lane inference.
                                        audio: flush_tail,
                                        gate_audio_samples: inference_audio.len(),
                                        inference_audio,
                                        is_final: true,
                                        scheduler_utterance_id,
                                        max_speech_prob,
                                        speech_vad_samples,
                                    },
                                    MAX_PENDING_UTTERANCES,
                                );
                                scheduler_utterance_id =
                                    scheduler_utterance_id.saturating_add(1);
                                if outcome.dropped > 0 {
                                    dropped_utterances = dropped_utterances.saturating_add(outcome.dropped);
                                    let message = if outcome.enqueued {
                                        if outcome.evicted_final {
                                            format!(
                                                "Pending utterance queue full (limit={}): evicted an older final item to preserve flush-final boundary",
                                                MAX_PENDING_UTTERANCES
                                            )
                                        } else {
                                            format!(
                                                "Pending utterance queue full (limit={}): evicted a non-final item to preserve flush-final boundary",
                                                MAX_PENDING_UTTERANCES
                                            )
                                        }
                                    } else {
                                        format!(
                                            "Pending utterance queue full (limit={}): dropped flush-final boundary",
                                            MAX_PENDING_UTTERANCES
                                        )
                                    };
                                    warn!(
                                        queue_len = pending_utterances.len(),
                                        is_final = true,
                                        enqueued = outcome.enqueued,
                                        evicted_final = outcome.evicted_final,
                                        dropped = outcome.dropped,
                                        "{}",
                                        message
                                    );
                                    event_sink.on_event(&EngineEvent::Warning {
                                        code: "pending_utterance_backpressure".to_string(),
                                        message,
                                    });
                                }
                            }
                        }
                        emit_vad_warning(&event_sink, &mut session);
                    }
                }
            }
            _ = tokio::time::sleep_until(
                partial_trigger_state.timer_baseline
                    + Duration::from_millis(PARTIAL_PASS_TRIGGER_TIMER_MS)
            ), if correction_in_flight.is_none() && !correction_audio_buf.is_empty() => {
                let now = Instant::now();
                let trigger_flags = partial_trigger_state.evaluate(now);
                if let Some(trigger) = classify_partial_trigger(trigger_flags)
                    && schedule_partial_pass(
                        &stt_scheduler,
                        output_sample_rate,
                        pipeline.language.clone(),
                        &mut correction_audio_buf,
                        &mut rolling_correction_window,
                        &mut correction_in_flight,
                        &mut correction_expected_window_id,
                        &mut correction_expected_boundary_rev,
                        &mut correction_expected_text,
                        &mut correction_suffix_snapshot,
                        &suffix_snapshot,
                        boundary_rev,
                        &window_text,
                        previous_window_prompt.clone(),
                        partial_trigger_state.silero_speech_ms_since_partial,
                        trigger,
                        &mut partial_telemetry,
                        &event_sink,
                    )
                {
                    partial_trigger_state.reset_after_success(now);
                }
            }
            result = async {
                correction_in_flight.as_mut().unwrap().recv().await
            }, if correction_in_flight.is_some() => {
                let expected_boundary_rev =
                    correction_expected_boundary_rev.take().unwrap_or(boundary_rev);
                let expected_window_id = correction_expected_window_id.take().unwrap_or_default();
                let expected_text = correction_expected_text.take().unwrap_or_default();
                let suffix_snapshot = correction_suffix_snapshot.take().unwrap_or_default();
                match result {
                    Ok(raw) => {
                        // Fix D: Compare against window-scoped state (survives utterance boundaries).
                        if correction_is_stale(
                            expected_boundary_rev,
                            boundary_rev,
                            &expected_text,
                            &window_text,
                        ) {
                            partial_telemetry.record_stale();
                            debug!(
                                expected_boundary_rev,
                                boundary_rev,
                                expected_window_id,
                                expected_len = expected_text.chars().count(),
                                current_len = window_text.chars().count(),
                                "Suppressing stale correction (boundary advanced)"
                            );
                        } else {
                            previous_window_prompt = rolling_correction_window
                                .merge_context(raw.clone())
                                .or_else(|| (!raw.text.trim().is_empty()).then(|| raw.text.clone()));
                            match postprocess_correction_with_snapshot(
                                &mut pipeline,
                                &raw.text,
                                &suffix_snapshot,
                            ) {
                                Ok(cleaned) => {
                                    let (previous_text, correction_after_boundary) =
                                        correction_baseline_text(
                                            &accumulated_text,
                                            &expected_text,
                                            &window_text,
                                        );
                                    // `cleaned` re-decodes only the audio slice taken by
                                    // schedule_partial_pass — splice it into the full
                                    // baseline instead of replacing the whole preview
                                    // (long hold sessions lost everything before the
                                    // correction window otherwise).
                                    let merged = if correction_after_boundary {
                                        Some(cleaned)
                                    } else {
                                        merge_corrected_window(
                                            &previous_text,
                                            &expected_text,
                                            &cleaned,
                                        )
                                    };
                                    match merged {
                                        Some(merged) if merged != previous_text => {
                                            preview_rev += 1;
                                            corrections_applied += 1;
                                            debug!(
                                                rev = preview_rev,
                                                previous_len = previous_text.chars().count(),
                                                corrected_len = merged.chars().count(),
                                                "BOUNDARY correction"
                                            );
                                            event_sink.on_event(&EngineEvent::Correction {
                                                rev: preview_rev,
                                                text: merged.clone(),
                                                previous_text,
                                            });
                                            if correction_after_boundary {
                                                debug!(
                                                    "Applied correction after boundary without reopening utterance-local preview state"
                                                );
                                            } else {
                                                // Update accumulated text so next Preview builds from corrected state.
                                                accumulated_text = merged;
                                            }
                                        }
                                        Some(_) => {
                                            debug!("Skipping correction emit: no text delta after postprocess");
                                        }
                                        None => {
                                            partial_telemetry.record_stale();
                                            debug!(
                                                expected_len = expected_text.chars().count(),
                                                baseline_len = previous_text.chars().count(),
                                                "Suppressing correction: window snapshot no longer anchored in preview baseline"
                                            );
                                        }
                                    }
                                }
                                Err(PostprocessDrop::Hallucination) => {
                                    // Already counted in postprocess_with_reason.
                                    debug!("Correction dropped as hallucination");
                                }
                                Err(PostprocessDrop::OverlapEmpty) => {
                                    // Already counted in postprocess_with_reason.
                                    debug!("Correction dropped as overlap-empty");
                                }
                                Err(PostprocessDrop::FilteredEmpty) => {
                                    filtered_empty_drops += 1;
                                    debug!("Correction dropped as filtered-empty");
                                }
                            }
                        }
                    }
                    Err(e) => {
                        partial_telemetry.record_dropped();
                        if e.to_string().contains("superseded") {
                            debug!("Skipping superseded correction request: {}", e);
                        } else if e.to_string().contains("shutting down") {
                            debug!("Ignoring correction during scheduler shutdown: {}", e);
                        } else {
                            warn!("Re-transcription failed; keeping Phase 1 draft: {}", e);
                        }
                    }
                }
                correction_in_flight = None;
            }
            // Drain the pipeline. FuturesOrdered guarantees results arrive in the order submitted.
            // This is critical for timestamp calculation and text accumulation.
            Some(result) = tail_patch_pipeline.next() => {
                if matches!(&result, Ok(job) if matches!(job.outcome, TailPatchOutcome::Skipped { .. }))
                    || result.is_err()
                {
                    tail_patch_skips = tail_patch_skips.saturating_add(1);
                }
                let result = result.map(TailPatchJobResult::into_outcome);
                tail_patch_replacements = tail_patch_replacements
                    .saturating_add(emit_tail_patch_result(event_sink.as_ref(), result));
            }
            // Drain the pipeline. FuturesOrdered guarantees results arrive in the order submitted.
            // This is critical for timestamp calculation and text accumulation.
            Some((result, mut item)) = inference_pipeline.next() => {
                // Track audio duration for timestamp computation.
                let chunk_start_samples = utterance_audio_samples;
                utterance_audio_samples += item.audio.len();
                let chunk_start_ts =
                    utterance_start_s + chunk_start_samples as f32 / output_sample_rate as f32;
                if correction_audio_buf.is_empty() {
                    suffix_snapshot = pipeline.last_suffix.clone();
                }
                rolling_correction_window.observe_samples(item.audio.len());
                correction_audio_buf.extend_from_slice(&item.audio);
                // Bound the Refine buffer to a trailing window so corrections
                // re-decode the fresh suffix, not the whole utterance (P2.17).
                cap_correction_buffer(
                    &mut correction_audio_buf,
                    output_sample_rate,
                    CORRECTION_WINDOW_SEC,
                );
                partial_trigger_state.observe_speech_event(item.is_final, item.speech_vad_samples);
                utterance_vad_speech_samples = utterance_vad_speech_samples
                    .saturating_add(item.speech_vad_samples);

                match result {
                    Ok(raw_transcript) => {
                        let raw_avg_logprob = raw_transcript.avg_logprob;
                        let raw_compression_ratio = raw_transcript.compression_ratio;
                        if item.is_final {
                            utterance_avg_logprob = raw_avg_logprob;
                            utterance_compression_ratio = raw_compression_ratio;
                        } else if utterance_avg_logprob.is_none() {
                            utterance_avg_logprob = raw_avg_logprob;
                            utterance_compression_ratio = raw_compression_ratio;
                        }
                        let raw_text = raw_transcript.text;
                        let mut raw_segments = raw_transcript.segments;
                        let segment_offset_ts = if item.is_final {
                            // Commit lane for final boundary is always VAD-prefiltered by scheduler.
                            // Segment timestamps are still utterance-relative.
                            utterance_start_s
                        } else {
                            chunk_start_ts
                        };
                        if !raw_segments.is_empty() {
                            for segment in &mut raw_segments {
                                segment.start_ts += segment_offset_ts;
                                segment.end_ts += segment_offset_ts;
                            }
                        }
                        last_raw_text = raw_text.clone();
                        last_segments = raw_segments.clone();
                        if item.is_final {
                            if !raw_segments.is_empty() {
                                utterance_segments = raw_segments.clone();
                            }
                        } else {
                            utterance_segments.extend(raw_segments.clone());
                        }

                        // Fix A: Restore suffix to utterance-boundary snapshot before
                        // FINAL processing so strip_overlap sees the correct tail.
                        if item.is_final {
                            pipeline.last_suffix = utterance_boundary_suffix.clone();
                        }

                        if let Some(words_per_sec) =
                            text_words_per_second(&raw_text, item.inference_audio_len, output_sample_rate)
                                .filter(|wps| *wps > MAX_WORDS_PER_SEC)
                        {
                            pipeline.hallucination_drops =
                                pipeline.hallucination_drops.saturating_add(1);
                            event_sink.on_event(&EngineEvent::Drop {
                                kind: DropKind::Hallucination,
                                text: raw_text.clone(),
                                reason: format!(
                                    "Word-rate anomaly: {:.1} words/s exceeds {:.1} words/s limit",
                                    words_per_sec, MAX_WORDS_PER_SEC
                                ),
                            });
                        } else {
                            match pipeline.postprocess_with_reason_and_segments_with_quality(
                                &raw_text,
                                &raw_segments,
                                raw_avg_logprob,
                            ) {
                                Ok(cleaned) => {
                                    if item.is_final {
                                        let cleaned_final = cleaned.trim();
                                        if apply_final_boundary_text(&mut accumulated_text, cleaned_final) {
                                            if !cleaned_final.is_empty() {
                                                // Fix D: Append FINAL text to window-scoped state
                                                // (not replace — window spans multiple utterances).
                                                append_to_correction_window_text(
                                                    &mut window_text,
                                                    cleaned_final,
                                                    CORRECTION_WINDOW_TEXT_MAX_CHARS,
                                                );
                                                boundary_rev += 1;
                                            } else {
                                                // Keep the latest preview when FINAL postprocess is empty.
                                                // Otherwise silence boundary may never emit UtteranceFinal,
                                                // which breaks auto-send on pause in toggle mode.
                                                debug!(
                                                    preview_len = accumulated_text.chars().count(),
                                                    "Final cleaned text empty; preserving latest preview for boundary commit"
                                                );
                                            }
                                        }
                                    } else {
                                        preview_rev += 1;
                                        if !accumulated_text.is_empty() {
                                            accumulated_text.push(' ');
                                        }
                                        accumulated_text.push_str(cleaned.trim());

                                        // Fix D: Mirror into window-scoped state for partial-pass baseline.
                                        // Do not bump boundary_rev here: interim previews are same-boundary
                                        // drafts and must not stale a live Refine correction.
                                        append_to_correction_window_text(
                                            &mut window_text,
                                            cleaned.trim(),
                                            CORRECTION_WINDOW_TEXT_MAX_CHARS,
                                        );

                                        debug!(
                                            rev = preview_rev,
                                            text_len = accumulated_text.chars().count(),
                                            "BOUNDARY preview"
                                        );
                                        event_sink.on_event(&EngineEvent::Preview {
                                            rev: preview_rev,
                                            text: accumulated_text.clone(),
                                        });

                                        if let Some(path) = stream_log_path.as_deref() {
                                            let _ = append_to_stream_log(path, cleaned.trim());
                                        }
                                    }
                                }
                                Err(PostprocessDrop::Hallucination) => {
                                    event_sink.on_event(&EngineEvent::Drop {
                                        kind: DropKind::Hallucination,
                                        text: raw_text.clone(),
                                        reason: format!(
                                            "Hallucination pattern: '{}'",
                                            raw_text.trim()
                                        ),
                                    });
                                }
                                Err(PostprocessDrop::OverlapEmpty) => {
                                    event_sink.on_event(&EngineEvent::Drop {
                                        kind: DropKind::OverlapEmpty,
                                        text: raw_text.clone(),
                                        reason: "Overlap dedup produced empty result".to_string(),
                                    });
                                }
                                Err(PostprocessDrop::FilteredEmpty) => {
                                    filtered_empty_drops += 1;
                                    event_sink.on_event(&EngineEvent::Drop {
                                        kind: DropKind::FilteredEmpty,
                                        text: raw_text.clone(),
                                        reason: "Empty after lexicon/cleanup (not semantic gate)".to_string(),
                                    });
                                }
                            }
                        }

                        if item.is_final {
                            utterance_id += 1;
                            total_utterances += 1;
                            // TRIM CONTRACT (single owner): this .trim() is the ONE place that
                            // guarantees UtteranceFinal.text == tail-patch committed_text ==
                            // already-trimmed. ReplaceRange char offsets are computed against
                            // THIS string; the SwiftUI sink stores it verbatim (its own trim is
                            // an idempotent no-op). Do not emit untrimmed text from any path.
                            let final_text = accumulated_text.trim().to_string();
                            let end_ts = utterance_start_s
                                + utterance_audio_samples as f32 / output_sample_rate as f32;
                            let had_content = !final_text.is_empty();
                            if had_content {
                                debug!(
                                    utterance_id,
                                    text_len = final_text.chars().count(),
                                    start_ts = utterance_start_s,
                                    end_ts,
                                    "BOUNDARY final"
                                );
                                let avg_logprob = utterance_avg_logprob.take();
                                let compression_ratio = utterance_compression_ratio.take();
                                let vad_speech_pct = utterance_vad_speech_pct(
                                    utterance_audio_samples,
                                    output_sample_rate,
                                    utterance_vad_speech_samples,
                                );
                                let confidence_flags =
                                    collect_confidence_flags(vad_speech_pct, avg_logprob);
                                event_sink.on_event(&EngineEvent::UtteranceFinal {
                                    utterance_id,
                                    text: final_text.clone(),
                                    raw_text: raw_text.clone(),
                                    start_ts: utterance_start_s,
                                    end_ts,
                                    segments: std::mem::take(&mut utterance_segments),
                                    vad_speech_pct,
                                    avg_logprob,
                                    compression_ratio,
                                    confidence_flags,
                                });
                                if tail_patch_enabled
                                    && let Some(audio) = item.tail_patch_audio.take()
                                {
                                    let sample_end = sample_start.saturating_add(audio.len() as u64);
                                    let request = TailProviderRequest {
                                        identity: TailRequestIdentity {
                                            request_id: utterance_id,
                                            range: TailSampleRange {
                                                session: session_id.clone(),
                                                capture_epoch: 0,
                                                sample_start,
                                                sample_end,
                                            },
                                        },
                                        sample_rate: output_sample_rate,
                                        language: pipeline.language.clone(),
                                    };
                                    tail_patch_pipeline.push(Box::pin(compute_tail_patch_job(
                                        utterance_id,
                                        final_text,
                                        // VAD lane: no sealed-prefix accumulator on this
                                        // path, so the anti-duplication check falls back
                                        // to the utterance's own canvas (pre-2026-08-14
                                        // behaviour, no regression).
                                        String::new(),
                                        audio,
                                        request,
                                        tail_patch_config,
                                    )));
                                }
                            } else {
                                utterance_segments.clear();
                            }
                            accumulated_text.clear();
                            // A committed utterance is immutable to the rolling
                            // Refine lane. Exact-ID tail patches above own any
                            // later fill/replacement for this span.
                            correction_audio_buf.clear();
                            window_text.clear();
                            previous_window_prompt = None;
                            rolling_correction_window.seal_utterance();
                            utterance_vad_speech_samples = 0;
                            utterance_avg_logprob = None;
                            utterance_compression_ratio = None;
                            // Fix A: Save current suffix as utterance-boundary snapshot
                            // for the next FINAL to restore from.
                            utterance_boundary_suffix = pipeline.last_suffix.clone();
                            // Advance start_ts for next utterance.
                            utterance_start_s = end_ts;
                            utterance_audio_samples = 0;

                            // Only emit VadEnd if UtteranceFinal was emitted — avoids
                            // spurious VadEnd without preceding UtteranceFinal.
                            if vad_started && had_content {
                                event_sink.on_event(&EngineEvent::VadEnd {
                                    speech_prob: session.boundary_prob(),
                                    ts_ms: session.session_elapsed_ms(),
                                });
                                vad_started = false;
                            }
                        }
                        let now = Instant::now();
                        let trigger_flags = partial_trigger_state.evaluate(now);
                        if correction_in_flight.is_none()
                            && let Some(trigger) = classify_partial_trigger(trigger_flags)
                            && schedule_partial_pass(
                                &stt_scheduler,
                                output_sample_rate,
                                pipeline.language.clone(),
                                &mut correction_audio_buf,
                                &mut rolling_correction_window,
                                &mut correction_in_flight,
                                &mut correction_expected_window_id,
                                &mut correction_expected_boundary_rev,
                                &mut correction_expected_text,
                                &mut correction_suffix_snapshot,
                                &suffix_snapshot,
                                boundary_rev,
                                &window_text,
                                previous_window_prompt.clone(),
                                partial_trigger_state.silero_speech_ms_since_partial,
                                trigger,
                                &mut partial_telemetry,
                                &event_sink,
                            )
                        {
                            partial_trigger_state.reset_after_success(now);
                        }
                    }
                    Err(e) => {
                        error!("Transcription failed: {}", e);
                        event_sink.on_event(&EngineEvent::Warning {
                            code: "transcription_error".to_string(),
                            message: format!("{}", e),
                        });
                    }
                }
            }
            else => {
                if audio_closed
                    && !pending_utterances.is_empty()
                    && inference_pipeline.is_empty()
                    && correction_in_flight.is_none()
                {
                    let abandoned = pending_utterances.len() as u64;
                    dropped_utterances = dropped_utterances.saturating_add(abandoned);
                    pending_utterances.clear();
                    warn!(
                        abandoned,
                        "Dropping pending utterances after audio closed because inference pipeline is idle"
                    );
                }
            }
        }
    }

    if let Err(e) = stt_scheduler.shutdown().await {
        error!("Failed to shutdown STT scheduler: {}", e);
        event_sink.on_event(&EngineEvent::Warning {
            code: "scheduler_shutdown_error".to_string(),
            message: format!("{}", e),
        });
    }

    // Emit any remaining accumulated text as final utterance.
    let remaining = accumulated_text.trim().to_string();
    if !remaining.is_empty() {
        utterance_id += 1;
        total_utterances += 1;
        let end_ts = utterance_start_s + utterance_audio_samples as f32 / output_sample_rate as f32;
        let segments = if utterance_segments.is_empty() {
            last_segments
        } else {
            utterance_segments
        };
        debug!(
            utterance_id,
            text_len = remaining.chars().count(),
            start_ts = utterance_start_s,
            end_ts,
            "BOUNDARY final_flush"
        );
        let vad_speech_pct = utterance_vad_speech_pct(
            utterance_audio_samples,
            output_sample_rate,
            utterance_vad_speech_samples,
        );
        let confidence_flags =
            collect_confidence_flags(vad_speech_pct, utterance_avg_logprob);
        event_sink.on_event(&EngineEvent::UtteranceFinal {
            utterance_id,
            text: remaining,
            raw_text: last_raw_text,
            start_ts: utterance_start_s,
            end_ts,
            segments,
            vad_speech_pct,
            avg_logprob: utterance_avg_logprob,
            compression_ratio: utterance_compression_ratio,
            confidence_flags,
        });
    }

    if total_utterances == 0 {
        if vad_started {
            event_sink.on_event(&EngineEvent::VadEnd {
                speech_prob: session.boundary_prob(),
                ts_ms: session.session_elapsed_ms(),
            });
        }
        let reason = if speech_activity_observed
            || pipeline.hallucination_drops > 0
            || filtered_empty_drops > 0
            || dropped_utterances > 0
        {
            "speech_observed_without_committed_text"
        } else {
            "vad_no_speech_detected"
        };
        event_sink.on_event(&EngineEvent::NoSpeech {
            reason: reason.to_string(),
        });
    }

    // Emit session stats.
    event_sink.on_event(&EngineEvent::Stats {
        dropped_audio_chunks: dropped_utterances,
        hallucination_drops: pipeline.hallucination_drops,
        filtered_empty_drops,
        corrections_applied,
        total_utterances,
        partial_runs_total: partial_telemetry.runs_total,
        trigger_utterance_count: partial_telemetry.trigger_utterance_count,
        trigger_speech_count: partial_telemetry.trigger_speech_count,
        trigger_timer_count: partial_telemetry.trigger_timer_count,
        partial_stale_count: partial_telemetry.stale_count,
        partial_coalesced_count: partial_telemetry.coalesced_count,
        partial_dropped_count: partial_telemetry.dropped_count,
    });

    let tail_patch_receipt = TailPatchSessionReceipt::from_stop(false, 0, 0, 0, 0);
    log_tail_patch_session_receipt(tail_patch_receipt);
    event_sink.on_event(&tail_patch_receipt.as_event());
    emit_capture_level_receipt(
        event_sink.as_ref(),
        &capture_level.finalize(CapturePathMeta::resolve(sample_rate, 1, None)),
    );
    emit_session_finalised(event_sink.as_ref(), session_id, tail_patch_replacements);

    if dropped_utterances > 0 {
        warn!(
            "Session dropped {} utterance(s) due to backpressure or scheduler stalls",
            dropped_utterances
        );
    }

    info!(
        "Transcription session finished: {} utterances, {} hallucination drops, {} filtered empty drops, {} tail patches, partial_runs={} (utterance={}, speech={}, watchdog={}, stale={}, coalesced={}, dropped={})",
        total_utterances,
        pipeline.hallucination_drops,
        filtered_empty_drops,
        tail_patch_replacements,
        partial_telemetry.runs_total,
        partial_telemetry.trigger_utterance_count,
        partial_telemetry.trigger_speech_count,
        partial_telemetry.trigger_timer_count,
        partial_telemetry.stale_count,
        partial_telemetry.coalesced_count,
        partial_telemetry.dropped_count
    );
}

/// One unit of work waiting to enter the inference pipeline.
///
/// Carries three different views of the same speech on purpose — `audio` for
/// accounting, `gate_audio_samples` for the drop gates, `inference_audio` for
/// Whisper — because a final boundary must be measured on the whole sealed
/// segment while only its unaccounted tail may be added to running totals.
#[derive(Debug)]
pub(crate) struct PendingUtteranceWorkItem {
    /// Accounting/correction audio: ONLY samples the consumer has not yet seen
    /// via earlier work items of the same utterance. The consumer sums
    /// `audio.len()` into utterance duration and appends it to the Refine
    /// buffer, so carrying already-enqueued samples here double-counts both.
    pub(crate) audio: Vec<f32>,
    /// Full-segment length the drop gates measure. Distinct from `audio`:
    /// a final boundary must be gated on the whole sealed segment, while its
    /// `audio` carries only the not-yet-accounted tail.
    pub(crate) gate_audio_samples: usize,
    pub(crate) inference_audio: Vec<f32>,
    pub(crate) is_final: bool,
    pub(crate) scheduler_utterance_id: u64,
    pub(crate) max_speech_prob: f32,
    pub(crate) speech_vad_samples: u64,
}

/// Context carried alongside an in-flight inference so the result can be
/// interpreted when it lands.
///
/// The pending item is consumed at submit time, but its outcome is handled
/// later and out of that scope; this is what survives the crossing. Only the
/// *length* of the inference audio is kept — the samples themselves are gone —
/// except for `tail_patch_audio`, retained solely when Layer 1 will need to
/// re-transcribe.
#[derive(Debug)]
#[cfg(any())]
struct UtteranceWorkItem {
    audio: Vec<f32>,
    inference_audio_len: usize,
    is_final: bool,
    tail_patch_audio: Option<Vec<f32>>,
    speech_vad_samples: u64,
}

/// [`EventSink`] that keeps only the finalized transcript, joining every
/// `UtteranceFinal` into one string. Backs [`transcribe_buffered_samples`].
struct SessionTranscriptCollector {
    transcript: std::sync::Mutex<String>,
}

impl SessionTranscriptCollector {
    /// Collector holding an empty transcript.
    fn new() -> Self {
        Self {
            transcript: std::sync::Mutex::new(String::new()),
        }
    }

    /// Append one finalized utterance, space-separated. Empty text is ignored
    /// so a dropped utterance leaves no gap.
    fn append_utterance(&self, text: &str) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        let mut transcript = self.transcript.lock().unwrap_or_else(|e| e.into_inner());
        if !transcript.is_empty() {
            transcript.push(' ');
        }
        transcript.push_str(trimmed);
    }

    /// Snapshot of everything collected so far.
    fn transcript(&self) -> String {
        self.transcript
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl EventSink for SessionTranscriptCollector {
    /// Append only `UtteranceFinal` text into the session transcript buffer.
    fn on_event(&self, event: &EngineEvent) {
        if let EngineEvent::UtteranceFinal { text, .. } = event {
            self.append_utterance(text);
        }
    }
}

/// [`EventSink`] that records the raw event stream in order, drops included.
/// Backs [`collect_buffered_engine_events`] and this module's own tests.
struct SessionEventCollector {
    events: std::sync::Mutex<Vec<EngineEvent>>,
}

impl SessionEventCollector {
    /// Collector with no events recorded.
    fn new() -> Self {
        Self {
            events: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Snapshot of the events recorded so far, in emission order.
    fn events(&self) -> Vec<EngineEvent> {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl EventSink for SessionEventCollector {
    /// Clone every engine event (including drops) into the ordered collector buffer.
    fn on_event(&self, event: &EngineEvent) {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(event.clone());
    }
}

/// Public helper: run the event session pipeline on in-memory samples.
///
/// Uses the same runtime path as live recording (`transcription_session`) and
/// collects utterance finals into a session transcript for test/CLI comparisons.
pub async fn transcribe_buffered_samples(
    samples: &[f32],
    sample_rate: u32,
    language: Option<String>,
) -> Result<String> {
    if samples.is_empty() {
        return Ok(String::new());
    }

    // Simulate live callback cadence (~100ms) to keep VAD/utterance behavior realistic.
    let chunk_size = ((sample_rate as f32) * 0.1).round().max(1.0) as usize;

    let (tx, rx) = mpsc::channel::<Vec<f32>>(8);
    let collector = Arc::new(SessionTranscriptCollector::new());
    let event_sink: Arc<dyn EventSink> = collector.clone();
    let runtime_settings = Arc::new(
        Config::load_runtime_snapshot_without_keychain()
            .map_err(|error| anyhow!("invalid runtime settings snapshot: {error:?}"))?,
    );
    let session = tokio::spawn(transcription_session(
        rx,
        event_sink,
        SessionConfig {
            session_id: uuid::Uuid::new_v4().to_string(),
            capture_epoch: 0,
            runtime_settings,
            acoustic_ledger: Arc::new(StdMutex::new(AcousticLedger::new())),
            sample_rate,
            language,
            stream_log_path: None,
            utterance_silence_sec: None,
            // Offline replay harness: Layer 1 arming is a live-recording
            // decision owned elsewhere.
            layer1: Layer1Decision::Disarmed,
            lifecycle_events: None,
        },
    ));

    for chunk in samples.chunks(chunk_size) {
        if tx.send(chunk.to_vec()).await.is_err() {
            return Err(anyhow!("Transcription session dropped channel"));
        }
    }
    drop(tx);

    session
        .await
        .map_err(|e| anyhow!("Transcription session join error: {}", e))?;

    Ok(collector.transcript())
}

/// Public helper: run the event session pipeline and return the emitted engine events.
///
/// This is the closest non-interactive test hook to the real live flow:
/// canonical audio samples enter the same `transcription_session` runtime used by
/// recording, and callers can replay the resulting `EngineEvent`s through
/// `PresentationEmitter`/overlay code without touching the microphone.
pub async fn collect_buffered_engine_events(
    samples: &[f32],
    sample_rate: u32,
    language: Option<String>,
) -> Result<Vec<EngineEvent>> {
    let runtime_settings = Arc::new(
        Config::load_runtime_snapshot_without_keychain()
            .map_err(|error| anyhow!("invalid runtime settings snapshot: {error:?}"))?,
    );
    collect_buffered_engine_events_with_config(
        samples,
        SessionConfig {
            session_id: uuid::Uuid::new_v4().to_string(),
            capture_epoch: 0,
            runtime_settings,
            acoustic_ledger: Arc::new(StdMutex::new(AcousticLedger::new())),
            sample_rate,
            language,
            stream_log_path: None,
            utterance_silence_sec: None,
            // Offline replay harness: Layer 1 arming is a live-recording
            // decision owned elsewhere.
            layer1: Layer1Decision::Disarmed,
            lifecycle_events: None,
        },
    )
    .await
}

/// Run buffered PCM through an explicitly supplied production session config.
///
/// Unlike [`collect_buffered_engine_events`], this seam never invents or
/// hard-codes a Layer 1 decision. The recording owner must supply the complete
/// [`SessionConfig`], which makes this suitable for production-owned replay
/// witnesses while preserving the exact `transcription_session` implementation
/// used by live capture.
pub async fn collect_buffered_engine_events_with_config(
    samples: &[f32],
    config: SessionConfig,
) -> Result<Vec<EngineEvent>> {
    if samples.is_empty() {
        return Ok(Vec::new());
    }

    let chunk_size = ((config.sample_rate as f32) * 0.1).round().max(1.0) as usize;
    let (tx, rx) = mpsc::channel::<Vec<f32>>(8);
    let collector = Arc::new(SessionEventCollector::new());
    let event_sink: Arc<dyn EventSink> = collector.clone();
    let session = tokio::spawn(transcription_session(rx, event_sink, config));

    for chunk in samples.chunks(chunk_size) {
        if tx.send(chunk.to_vec()).await.is_err() {
            return Err(anyhow!("Transcription session dropped channel"));
        }
        // `transcription_session` consumes a live capture stream. Preserve
        // that temporal contract for replay: flooding an entire recording in
        // one scheduler tick advances `audio_secs` ahead of Apple's result
        // timestamps and turns otherwise valid phrase windows into unresolved
        // seals. A 100 ms packet therefore occupies 100 ms of wall time, just
        // like the production callback cadence this seam replaces at its only
        // unavoidable boundary.
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    drop(tx);

    session
        .await
        .map_err(|e| anyhow!("Transcription session join error: {}", e))?;

    Ok(collector.events())
}

#[cfg(any())]
/// Unit tests for semantic-gate counters, tail-patch emit, and correction windows.
mod session_tests {
    use super::*;

    #[test]
    fn tail_patch_session_receipt_round_trips_through_production_event_shape() {
        let receipt =
            TailPatchSessionReceipt::new(true, 4, 2, 1, 1, 0, TailPatchDrainDisposition::TimedOut);
        assert_eq!(
            TailPatchSessionReceipt::from_events(&[receipt.as_event()]),
            Some(receipt)
        );
        assert!(!receipt.armed_without_submissions());

        let unexercised =
            TailPatchSessionReceipt::new(true, 0, 0, 0, 0, 0, TailPatchDrainDisposition::Completed);
        assert!(unexercised.armed_without_submissions());
    }

    #[test]
    fn stop_receipt_classifies_completed_and_timeout_residue() {
        let completed = TailPatchSessionReceipt::from_stop(true, 3, 2, 1, 0);
        assert_eq!(completed.drain, TailPatchDrainDisposition::Completed);
        assert_eq!(completed.timed_out, 0);
        assert_eq!(completed.abandoned, 0);

        let timed_out = TailPatchSessionReceipt::from_stop(true, 3, 1, 0, 2);
        assert_eq!(timed_out.drain, TailPatchDrainDisposition::TimedOut);
        assert_eq!(timed_out.timed_out, 2);
        assert_eq!(timed_out.abandoned, 0);
        assert_eq!(
            TailPatchSessionReceipt::from_events(&[timed_out.as_event()]),
            Some(timed_out)
        );
    }

    #[tokio::test]
    async fn vad_route_refuses_unbound_local_tail_patch_mutation() {
        let (tx, rx) = mpsc::channel::<Vec<f32>>(1);
        drop(tx);
        let collector = Arc::new(SessionEventCollector::new());
        let sink: Arc<dyn EventSink> = collector.clone();
        vad_transcription_session(
            rx,
            sink,
            SessionConfig {
                sample_rate: 16_000,
                language: Some("pl".to_string()),
                stream_log_path: None,
                utterance_silence_sec: None,
                layer1: Layer1Decision::LocalTailPatch(
                    crate::asr_session::LocalTailPatchDisposition::ArmedPhase(1),
                ),
                lifecycle_events: None,
            },
        )
        .await;

        let events = collector.events();
        assert!(events.iter().any(|event| matches!(
            event,
            EngineEvent::Warning { code, .. }
                if code == TAIL_PATCH_ROUTE_UNBOUND_WARNING_CODE
        )));
        assert!(events.iter().all(|event| !matches!(
            event,
            EngineEvent::ReplaceRange {
                source: LayerSource::TailPatch,
                ..
            }
        )));
        let receipt = TailPatchSessionReceipt::from_events(&events)
            .expect("VAD route must emit typed tail-patch evidence");
        assert!(!receipt.armed);
        assert_eq!(receipt.submitted, 0);
        assert_eq!(receipt.drain, TailPatchDrainDisposition::NotArmed);
        let receipt_pos = events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    EngineEvent::Warning { code, .. }
                        if code == TAIL_PATCH_SESSION_RECEIPT_WARNING_CODE
                )
            })
            .expect("receipt event");
        let final_pos = events
            .iter()
            .position(|event| matches!(event, EngineEvent::SessionFinalised { .. }))
            .expect("session finalised event");
        assert!(
            receipt_pos < final_pos,
            "receipt must precede session finality"
        );
    }

    #[tokio::test]
    async fn w13_provenance_survives_tail_patch_job() {
        let range = TailSampleRange {
            session: "w13-replay-191351".to_string(),
            capture_epoch: 4,
            sample_start: 48_000,
            sample_end: 48_320,
        };
        let identity = TailRequestIdentity {
            request_id: 73,
            range: range.clone(),
        };
        let evidence = crate::stt::tail_provider::TailProviderEvidence {
            source: TailEvidenceSource::Whisper,
            revision: Some("fixture-r1".to_string()),
            stability: TailEvidenceStability::Final,
            timing_quality: TailTimingQuality::ExactSampleRange,
            avg_logprob: Some(-0.21),
        };
        let payload = TailProviderPayload {
            identity: identity.clone(),
            text: "ala ma kota".to_string(),
            segments: vec![TimedTailSegment {
                text: "kota".to_string(),
                range: TailSampleRange {
                    sample_start: 48_160,
                    sample_end: 48_300,
                    ..range.clone()
                },
            }],
            avg_logprob: Some(-0.21),
            compression_ratio: Some(1.03),
            provider_id: TailProviderId::Fake,
            elapsed_ms: 7,
            evidence: evidence.clone(),
        };
        let request = TailProviderRequest {
            identity,
            sample_rate: 16_000,
            language: Some("pl-PL".to_string()),
        };

        let job = compute_tail_patch_job_with(
            73,
            "ala ma kota".to_string(),
            String::new(),
            vec![0.0; 320],
            request,
            TailPatchConfig::default(),
            move |request, pcm| {
                request.validate_pcm(pcm)?;
                Ok(payload)
            },
        )
        .await
        .expect("typed fake tail job");

        assert!(matches!(job.outcome, TailPatchOutcome::NoChange));
        assert_eq!(job.payload.identity.range, range);
        assert_eq!(job.payload.segments[0].range.sample_start, 48_160);
        assert_eq!(job.payload.segments[0].range.sample_end, 48_300);
        assert_eq!(job.payload.evidence, evidence);
        assert_eq!(job.payload.provider_id, TailProviderId::Fake);
    }

    #[test]
    /// Successful tail-patch outcomes surface as `ReplaceRange` engine events.
    fn tail_patch_result_emits_replace_range_events() {
        let collector = SessionEventCollector::new();
        let emitted = emit_tail_patch_result(
            &collector,
            Ok((
                42,
                TailPatchOutcome::Patches(vec![EngineEvent::ReplaceRange {
                    utterance_id: 42,
                    start: 4,
                    end: 7,
                    text: "kot".to_string(),
                    source: LayerSource::TailPatch,
                }]),
            )),
        );

        assert_eq!(emitted, 1);
        assert!(matches!(
            collector.events().as_slice(),
            [EngineEvent::ReplaceRange {
                utterance_id: 42,
                start: 4,
                end: 7,
                text,
                source: LayerSource::TailPatch,
            }] if text == "kot"
        ));
    }

    /// Build an under-commit outcome with `appends` gap-appends and the given
    /// escalation, without needing Whisper or a diff.
    fn under_commit_fixture(appends: usize, residual_required: bool) -> UnderCommit {
        UnderCommit {
            appends: (0..appends)
                .map(|idx| EngineEvent::ReplaceRange {
                    utterance_id: 7,
                    start: 11 + idx,
                    end: 11 + idx,
                    text: " odzyskane".to_string(),
                    source: LayerSource::TailPatch,
                })
                .collect(),
            residual_required,
            committed_tokens: 3,
            retranscribed_tokens: 12,
            committed_chars: 21,
            retranscribed_chars: 84,
            commit_ratio: 0.25,
        }
    }

    #[test]
    /// W-C: recovered gap-appends reach the sink and are counted as Layer 1
    /// work — the outcome the bounded cap used to discard in silence.
    fn under_commit_gap_appends_reach_the_sink_and_count() {
        let collector = SessionEventCollector::new();
        let emitted = emit_tail_patch_result(
            &collector,
            Ok((
                7,
                TailPatchOutcome::UnderCommit(under_commit_fixture(1, false)),
            )),
        );

        assert_eq!(emitted, 1, "an appended gap is Layer 1 work, not a skip");
        let events = collector.events();
        assert!(matches!(
            events.as_slice(),
            [EngineEvent::ReplaceRange {
                start: 11,
                end: 11,
                source: LayerSource::TailPatch,
                ..
            }]
        ));
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, EngineEvent::Warning { .. })),
            "nothing is owed to the stop path when everything landed live"
        );
    }

    #[test]
    /// W-C: an under-commit that could place nothing escalates outward instead
    /// of leaving the stop path to call the starved canvas complete.
    fn under_commit_without_safe_anchor_emits_residual_escalation() {
        let collector = SessionEventCollector::new();
        let emitted = emit_tail_patch_result(
            &collector,
            Ok((
                7,
                TailPatchOutcome::UnderCommit(under_commit_fixture(0, true)),
            )),
        );

        assert_eq!(emitted, 0);
        let events = collector.events();
        let warning = events
            .iter()
            .find_map(|e| match e {
                EngineEvent::Warning { code, message } if code == UNDER_COMMIT_WARNING_CODE => {
                    Some(message.clone())
                }
                _ => None,
            })
            .expect("residual escalation must be emitted");
        // Counts travel; transcript text never does.
        assert!(warning.contains("committed_chars=21"));
        assert!(warning.contains("retranscribed_chars=84"));
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, EngineEvent::ReplaceRange { .. })),
            "no anchor was safe, so no canvas may be touched"
        );
    }

    #[test]
    /// Trimmed final_text is the sole offset baseline for tail-patch apply.
    fn final_text_trim_contract_keeps_tail_patch_offsets_aligned() {
        // Simulate the emit site: accumulated_text carries whitespace, the single
        // trim owner produces final_text, and that SAME string is both
        // UtteranceFinal.text and the tail-patch committed_text.
        let accumulated = "  ala ma kota  ";
        let final_text = accumulated.trim().to_string();

        // Retranscribed side mimics real Whisper output shape: leading/trailing
        // whitespace and a newline. It must never skew offsets or get skipped.
        let outcome = crate::stt::tail_patcher::compute_tail_patch(
            &final_text,
            " ala ma psa \n",
            1,
            &TailPatchConfig::default(),
        );
        let TailPatchOutcome::Patches(events) = &outcome else {
            panic!("expected Patches (not Skipped/NoChange), got {outcome:?}");
        };

        // Offsets must apply cleanly against the exact string the consumer holds.
        let mut buf = final_text.clone();
        for event in events {
            let applied = event
                .apply_to_committed_text(&mut buf)
                .expect("patch offsets must be in range for the trimmed final_text");
            assert!(applied, "ReplaceRange must mutate the committed buffer");
        }
        assert_eq!(buf, "ala ma psa");
    }

    #[test]
    /// Session end emits `SessionFinalised` carrying the layer replacement summary.
    fn session_finalised_emits_layer_summary() {
        let collector = SessionEventCollector::new();
        emit_session_finalised(&collector, "session-test".to_string(), 3);

        assert!(matches!(
            collector.events().as_slice(),
            [EngineEvent::SessionFinalised {
                session_id,
                layer_summary,
            }] if session_id == "session-test"
                && layer_summary.tail_patch_replacements == 3
                && layer_summary.lexicon_replacements == 0
                && layer_summary.inline_llm_replacements == 0
                && layer_summary.final_bam_replacements == 0
                && layer_summary.annotations_inserted == 0
        ));
    }

    #[test]
    /// The starvation verdict: zero applied with the skip floor reached is the
    /// lane not doing its job. One landed patch — even against 116 skips —
    /// proves the lane alive; a skip or two with nothing applied is honest
    /// divergence, not starvation.
    fn tail_patch_starvation_fires_only_on_all_rejected_sessions() {
        assert!(tail_patch_lane_starved(0, TAIL_PATCH_STARVED_MIN_SKIPS));
        assert!(tail_patch_lane_starved(0, 116));
        assert!(!tail_patch_lane_starved(1, 116), "one landed patch = alive");
        assert!(
            !tail_patch_lane_starved(0, TAIL_PATCH_STARVED_MIN_SKIPS - 1),
            "a couple of honest divergences is not starvation"
        );
        assert!(
            !tail_patch_lane_starved(0, 0),
            "an idle lane is not starved"
        );
    }

    #[test]
    /// Correction audio buffer drains oldest samples so length never exceeds the window.
    fn correction_buffer_window_cap() {
        let sr = 16_000u32;
        let window = CORRECTION_WINDOW_SEC;
        let cap = (window * sr as f32) as usize;

        // Under cap: untouched, nothing drained.
        let mut buf: Vec<f32> = vec![0.0; cap / 2];
        let len_before = buf.len();
        assert_eq!(cap_correction_buffer(&mut buf, sr, window), 0);
        assert_eq!(buf.len(), len_before);

        // Grow well past the cap across several 1s extends (25s > 18s window):
        // buffer must never exceed cap.
        let chunks = 25u32;
        let mut buf: Vec<f32> = Vec::new();
        for chunk in 0..chunks {
            let chunk_samples: Vec<f32> = (0..sr).map(|i| (chunk * sr + i) as f32).collect();
            buf.extend_from_slice(&chunk_samples);
            cap_correction_buffer(&mut buf, sr, window);
            assert!(buf.len() <= cap, "buffer {} exceeds cap {}", buf.len(), cap);
        }
        // After overflow it is pinned to exactly the cap...
        assert_eq!(buf.len(), cap);
        // ...and holds the freshest tail (last sample is the most recent one).
        let last = *buf.last().unwrap();
        assert_eq!(last, (chunks * sr - 1) as f32);

        // Zero window disables capping (no panic, no drain).
        let mut buf: Vec<f32> = vec![1.0; 100];
        assert_eq!(cap_correction_buffer(&mut buf, sr, 0.0), 0);
        assert_eq!(buf.len(), 100);
    }

    #[test]
    /// Correction text window evicts the oldest head and keeps the freshest tail.
    fn correction_window_text_cap_keeps_recent_tail() {
        let max_chars = 64usize;
        let mut window_text = String::new();

        for idx in 0..40 {
            append_to_correction_window_text(
                &mut window_text,
                &format!("utterance-{idx:02}"),
                max_chars,
            );
            assert!(
                window_text.chars().count() <= max_chars,
                "window text {} chars exceeds cap {}",
                window_text.chars().count(),
                max_chars
            );
        }

        assert!(
            !window_text.contains("utterance-00"),
            "old text should be evicted from the correction window"
        );
        assert!(
            window_text.ends_with("utterance-39"),
            "fresh correction tail must stay available"
        );
    }
}
