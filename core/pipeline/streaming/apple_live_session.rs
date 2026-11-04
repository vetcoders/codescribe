//! Shared live session — one capture clock with independently optional Apple
//! recognition and ledger admission.
//!
//! Enabled Apple streams provide raw previews and phrase finals. Ledger mode
//! admits their observations normally; direct mode publishes the selected
//! recognizer's exact bytes without word certificates or acoustic seals.
//!
//! With ledger admission enabled, Apple and Lexicon/Light+ are separate
//! observations of the same
//! PCM-identified occurrence. `admit_ledger_label` offers them to
//! `AcousticLedger::admit`; a closed occurrence passes through
//! `AcousticLedger::seal`; `EngineEvent::LedgerMutation` and
//! `EngineEvent::LedgerSeal` then carry the receipts to
//! `PresentationEmitter` / `TranscriptReducer`. Neither raw Apple text nor the
//! seal-time shaping pass owns the document.
//!
//! Local Power arms Whisper through the explicit Layer 1 decision, never an
//! implicit model load. Ledger mode retains capture-clock 9-second windows
//! every 3 seconds, shared with physically qualified owners. Without Apple,
//! Whisper can supply those owners without an Apple label prerequisite.
//! Direct mode uses disjoint 9-second publication ranges and a final short
//! range at Stop; it forwards raw payloads rather than stitching overlapping
//! text or slicing translations against source-language word counts.
//! One deliberate exception: the terminal DictationTranscriber witness. When
//! both controls permit Apple/ledger work and the opt-in lane is available,
//! Stop may run a bounded number of final
//! DT decodes over the recorder-owned terminal archive, aimed only at ranges
//! still unresolved after every retained replay (pending text recovery and
//! material speech gaps). Its segments enter through the same occurrence
//! admission as any producer; the whole-take DT text never replaces the
//! document and never reopens a seal.
//! Apple-only never implicitly loads Whisper. If Apple is disabled and no
//! consent-owned Layer 1 source is armed, the take closes with a no-source
//! outcome. Explicit off/invalid Local Power overrides retain their degraded
//! disposition.
//! `CODESCRIBE_APPLE_STT_LIVE_MODE=wav` selects the older Apple `transcribe_live`
//! temp-WAV request transport for A/B comparison with live AudioBuffer delivery.
//! It does not restore the deleted VAD/scheduler pipeline or create another
//! transcript authority.
//!
//! When enabled, the Apple bridge global lock and child live on a dedicated OS thread
//! (MutexGuard is `!Send`); the async session only shuttles PCM in and
//! `EngineEvent`s out.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use futures_util::stream::FuturesOrdered;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::pipeline::occurrence_slot_receipt::SlotReceiptSink;

use crate::agent::consultation::{
    ConsultationGroupAnswer, ConsultationInputQueue, ConsultationReadiness,
    PendingConsultationGroup, PreparedConsultationGroup, SealedConsultationInput,
};
use crate::asr_session::recorder::{
    LAYER1_DEGRADED_WARNING_CODE, LAYER1_FINALS_NOT_ADMITTED_WARNING_CODE, Layer1DegradeReason,
    RecorderLayer1Lane, apply_recorder_lifecycle_event,
};
use crate::asr_session::{SessionId as Layer1SessionId, SessionInput as Layer1SessionInput};
use crate::audio::capture_receipt::{
    AcousticAvailability, AcousticSpeechEvidence, CaptureEnergyOwner, CaptureLevelAccumulator,
    CapturePathMeta, DIGITAL_ZERO_ABS, SpeechAnchor, emit_capture_level_receipt,
};
use crate::audio::speech_input::{
    SpeechInputArchive, SpeechInputFrame, SpeechInputPlan, SpeechInputStream,
};
use crate::audio::streaming_recorder::CaptureTurnIntent;
use crate::config::{FormattingPolicy, RuntimeSettingsSnapshot};
use crate::llm::ai_formatting::{
    AiFormatResult, AiFormatStatus, TakeFormatterChain, format_text_with_status_for_policy,
};
use crate::llm::inline_format::{LabelProposalDisposition, OccurrenceLabelProposal};
#[cfg(test)]
use crate::pipeline::acoustic_ledger::SealCoverageStatus;
use crate::pipeline::acoustic_ledger::{
    AcousticEvidence, AcousticLedger, EnergyCalibration, MutationReceipt, NoAuthorityReason,
    ObservationIdentity as LedgerObservationIdentity,
    ObservationProducer as LedgerObservationProducer, OccurrenceIdentity, OverlapPinClass,
    RefuseReason, SealCoverageReceipt, SealRefusal, SilenceUnderSpan,
};
use crate::pipeline::contracts::{
    ApplePhraseOutcome, ClosedApplePhrase, EngineEvent, EventSink, PreviewPin,
    SessionConservationReceipt, SpeechIntegrity, SpeechIntegrityPhase, TranscriptSegment,
    UnadmittedAppleWord, UnadmittedAppleWordSource,
};
use crate::pipeline::engine_tuning::EngineTuning;
use crate::pipeline::speech_islands::{
    SpeechIslandEvidence, SpeechIslandInput, join_speech_islands, measure_speech_islands,
};
use crate::pipeline::whisper_word_speech::{
    SileroSpeechDistance, WhisperWordSpeech, WhisperWordSpeechDecision, WhisperWordSpeechVerdict,
    judge_whisper_words_tuned, repeat_key, separation_window, window_has_energy_valley,
    window_has_silero_dip,
};
use crate::stt::apple_stt::{LiveStreamEvent, LiveStreamSession};
use crate::stt::tail_patcher::{SkipReasonCode, TailPatchConfig, TailPatchOutcome};
use crate::stt::tail_provider::{
    TailEvidenceSource, TailProviderPayload, TailProviderRequest, TailRequestIdentity,
    TailSampleRange, TailSegmentGrain, TimedTailSegment,
};

use super::layer1_window::CaptureWindowPlan;
use super::live_audio_buffer::{DEFAULT_RETENTION_SECS, LiveAudioBuffer, ResolvedAudioWindow};
use super::session::{
    LocalExecutionOwner, SessionConfig, TailPatchDrainDisposition, TailPatchJobInput,
    TailPatchJobResult, TailPatchSessionReceipt, compute_tail_patch_job, emit_session_finalised,
    log_tail_patch_session_receipt,
};
use super::silero_fusion::{FusionWord, SileroIngress, slice_apple_words};
use super::speech_progress::SpeechProgress;
use super::stream_log::append_to_stream_log;

/// Bounded transport for capture-plan windows. A full queue retains the exact
/// pending request without blocking the worker that forwards PCM to SFSpeech.
const TAIL_PATCH_QUEUE_CAP: usize = 8;
/// Bounded transport ownership for occurrence formatter jobs. Saturation skips
/// Formatter scheduling for that occurrence; it never backpressures PCM.
const FORMATTER_QUEUE_CAP: usize = 8;

const CONSULTATION_QUEUE_CAP: usize = 16;

enum LiveConsultationRequest {
    Assess(SealedConsultationInput),
    Prepare(SealedConsultationInput),
    Finish(PendingConsultationGroup),
}

#[cfg(test)]
mod retroactive_split_delivery_tests {
    use super::*;

    fn sample(seconds: f32) -> u64 {
        (seconds * 16_000.0) as u64
    }

    fn word(text: &str, start: f32, end: f32) -> TranscriptSegment {
        TranscriptSegment {
            confidence: None,
            text: text.into(),
            start_ts: start,
            end_ts: end,
        }
    }

    #[test]
    fn retroactive_split_delivers_apple_words_on_both_sides_without_overlap_refusal() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = AppleSealState::new_for_session(16_000, "retro-apple".into(), 0);
        state.energy_calibration = Some(EnergyCalibration::new("synthetic", 0.0, 0));
        state.audio.push(&vec![0.25; sample(30.0) as usize]);
        let mut fusion = SileroIngress::new(16_000, state.session_id.clone(), 0);
        fusion.observe(Some((0, sample(20.0))), false, sample(20.0));
        state.fusion = Some(fusion);
        state.fusion_seal_armed = true;
        assert!(seal_sliced_by_silero(
            &mut state,
            &tx,
            &[word("before", 5.0, 5.5), word("after", 14.0, 14.5)]
        ));
        state.fusion.as_mut().unwrap().observe_with_closed_end(
            Some((sample(11.0), sample(20.0))),
            true,
            Some(sample(11.0)),
            sample(20.0),
        );
        assert!(seal_sliced_by_silero(&mut state, &tx, &[]));
        state.fusion.as_mut().unwrap().observe(
            Some((sample(11.0), sample(22.0))),
            false,
            sample(22.0),
        );
        state
            .fusion
            .as_mut()
            .unwrap()
            .observe(None, true, sample(22.0));
        assert!(seal_sliced_by_silero(&mut state, &tx, &[]));
        let text = state.acoustic_ledger.lock().unwrap().rendered_text();
        assert!(
            text.contains("before"),
            "first Apple word must have a decision: {text}"
        );
        assert!(
            text.contains("after"),
            "second Apple word must have a decision: {text}"
        );
        let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
        assert!(!events.iter().any(
            |event| matches!(event, EngineEvent::Warning { code, .. } if code.contains("overlap"))
        ));
    }
}

enum LiveConsultationResult {
    Assessed(Result<ConsultationReadiness>),
    Prepared(Result<PreparedConsultationGroup>),
    Answer(Result<ConsultationGroupAnswer>),
}

enum LiveConsultationReturn {
    Assessed(Result<ConsultationReadiness>),
    Prepared(Result<PreparedConsultationGroup>),
    Published(Result<()>),
}

/// Capture-thread grouping and admission. Provider futures live on the async
/// session; only this owner reads speech edges and advances the accepted prefix.
struct LiveConsultationCapture {
    queue: ConsultationInputQueue,
    requests: mpsc::Sender<LiveConsultationRequest>,
    returns: std_mpsc::Receiver<LiveConsultationReturn>,
    last_assessed: Option<SealedConsultationInput>,
    assessment_pending: bool,
    answers_pending: usize,
    speech_open: bool,
    refused: bool,
}

impl LiveConsultationCapture {
    fn observe(
        &mut self,
        ingest: &super::silero_fusion::SileroIngest,
        samples_seen: u64,
    ) -> Result<()> {
        self.speech_open = ingest.open.is_some();
        if ingest.speech_live {
            self.queue.resume_speech();
        }
        // A closing block is speech_live too. Only the actual close with no
        // successor open can nominate a candidate; silence alone cannot.
        if !ingest.closed.is_empty() && !self.speech_open {
            self.queue.append_boundary(samples_seen)?;
        }
        Ok(())
    }

    fn report_refusal(&mut self, events: &mpsc::UnboundedSender<EngineEvent>) {
        if !self.refused {
            let _ = events.send(EngineEvent::Warning {
                code: "max_consultation_refused".into(),
                message: "Max consultation could not settle; source audio and transcript remain authoritative. Accepted tools must not be replayed.".into(),
            });
        }
        self.refused = true;
    }

    fn tick(&mut self, state: &AppleSealState, events: &mpsc::UnboundedSender<EngineEvent>) {
        loop {
            let result = match self.returns.try_recv() {
                Ok(result) => result,
                Err(std_mpsc::TryRecvError::Empty) => break,
                Err(std_mpsc::TryRecvError::Disconnected) => {
                    self.report_refusal(events);
                    // The session task vanished. This cannot certify tool
                    // cancellation; the retained executor still owns recovery.
                    self.assessment_pending = false;
                    self.answers_pending = 0;
                    break;
                }
            };
            match result {
                LiveConsultationReturn::Published(result) => {
                    self.answers_pending = self.answers_pending.saturating_sub(1);
                    if result.is_err() {
                        self.report_refusal(events);
                    }
                }
                LiveConsultationReturn::Assessed(result) => {
                    self.assessment_pending = false;
                    let Ok(assessment) = result else {
                        let _ = events.send(EngineEvent::Warning {
                            code: "max_assessment_unavailable".into(),
                            message: "Max could not assess this candidate. It remains pending; new speech can form a new candidate.".into(),
                        });
                        continue;
                    };
                    if self.refused || self.speech_open {
                        continue;
                    }
                    let ConsultationReadiness::Complete(input) = assessment else {
                        continue;
                    };
                    match self
                        .requests
                        .try_send(LiveConsultationRequest::Prepare(input))
                    {
                        Ok(()) => self.assessment_pending = true,
                        Err(_) => self.report_refusal(events),
                    }
                }
                LiveConsultationReturn::Prepared(result) => {
                    self.assessment_pending = false;
                    let prepared = match result {
                        Ok(prepared) => prepared,
                        Err(_) => {
                            self.report_refusal(events);
                            continue;
                        }
                    };
                    // A dropped preparation cannot execute. Revalidate AFTER
                    // disk I/O, while capture owns current speech and ledger.
                    if self.refused || self.speech_open {
                        continue;
                    }
                    let Some(fusion) = state.fusion.as_ref() else {
                        continue;
                    };
                    let speech = fusion.acoustic_speech_evidence();
                    // Reserve result transport BEFORE accepting effects. A
                    // successfully accepted handle cannot disappear on pressure.
                    let sender = self.requests.clone();
                    let Ok(permit) = sender.try_reserve() else {
                        self.report_refusal(events);
                        continue;
                    };
                    let ledger = state
                        .acoustic_ledger
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    match self.queue.ready(&ledger, &speech) {
                        Ok(Some(current)) if &current == prepared.input() => {}
                        _ => continue,
                    }
                    // A stale assessment is expected after continuation. It
                    // never invokes the executor and never consumes the front.
                    let accepted = self.queue.authorize_prepared(prepared, &ledger, &speech);
                    match accepted {
                        Ok(pending) => {
                            let acknowledged = self.queue.acknowledge(&pending);
                            self.answers_pending += 1;
                            permit.send(LiveConsultationRequest::Finish(pending));
                            if acknowledged.is_err() {
                                self.report_refusal(events);
                            }
                        }
                        Err(_) => self.report_refusal(events),
                    }
                }
            }
        }
        if self.refused
            || self.speech_open
            || self.assessment_pending
            || self.answers_pending >= CONSULTATION_QUEUE_CAP
        {
            return;
        }
        let Some(fusion) = state.fusion.as_ref() else {
            return;
        };
        let speech = fusion.acoustic_speech_evidence();
        let ledger = state
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while self.queue.skip_measured_silence(&ledger, &speech) {}
        let input = match self.queue.ready(&ledger, &speech) {
            Ok(Some(input)) => input,
            Ok(None) => return,
            Err(_) => {
                self.report_refusal(events);
                return;
            }
        };
        if self.last_assessed.as_ref() == Some(&input) {
            return;
        }
        match self
            .requests
            .try_send(LiveConsultationRequest::Assess(input.clone()))
        {
            Ok(()) => {
                self.last_assessed = Some(input);
                self.assessment_pending = true;
            }
            Err(mpsc::error::TrySendError::Full(_)) => {}
            Err(mpsc::error::TrySendError::Closed(_)) => self.report_refusal(events),
        }
    }

    fn settle(
        &mut self,
        state: &AppleSealState,
        events: &mpsc::UnboundedSender<EngineEvent>,
        end: u64,
    ) {
        self.speech_open = false;
        if self.queue.append_boundary(end).is_err() {
            self.report_refusal(events);
        }
        while self.queue.pending_groups() > 1 {
            if self.queue.join_front().is_err() {
                self.report_refusal(events);
                break;
            }
        }
        loop {
            self.tick(state, events);
            if !self.assessment_pending && self.answers_pending == 0 {
                break;
            }
            // The async session keeps draining providers, approvals and answer
            // publication. Microphone capture has already ended at this point.
            thread::sleep(LIVE_WORKER_QUANTUM);
        }
        if self.queue.pending_groups() > 0 {
            self.report_refusal(events);
        }
    }
}

fn deliver_consultation_result(
    result: LiveConsultationResult,
    returns: &std_mpsc::Sender<LiveConsultationReturn>,
    sink: &dyn EventSink,
) {
    let result = match result {
        LiveConsultationResult::Assessed(result) => LiveConsultationReturn::Assessed(result),
        LiveConsultationResult::Prepared(result) => LiveConsultationReturn::Prepared(result),
        LiveConsultationResult::Answer(result) => LiveConsultationReturn::Published(
            result.and_then(|answer| sink.on_consultation_completed(&answer)),
        ),
    };
    if returns.send(result).is_err() {
        warn!("Max capture owner closed before result acknowledgement; no execution replay");
    }
}

/// Total budget for the end-of-session closure loop across all Layer 1
/// job to report back.
///
/// This sits directly on the stop path, in front of an operator watching the
/// overlay, so it is a product budget rather than an engineering safety net.
/// Every job it waits for was queued during capture against a model that is
/// already warm, and the observed windows close well under a second; the cap is
/// here for a genuinely wedged job, not for normal completion. It was 30s until
/// 2026-08-12, when a stop that owed nothing at all still paid the full 30s
/// because the loop was waiting on the wrong condition.
const TAIL_PATCH_CLOSURE_TIMEOUT: Duration = Duration::from_secs(5);

/// Closure budget when this session's local engine has not yet completed a
/// single window. The warm assumption behind [`TAIL_PATCH_CLOSURE_TIMEOUT`]
/// does not hold on first use: the one accepted job may still be paying the
/// model cold-load, and expiring it discards the only text the take has
/// (measured 2026-10-16, take 30ac82b4: 3.99s load + 8.755s encoder, while
/// the 5s budget expired and the receipt closed submitted=3/applied=0).
/// A stop that owes nothing still pays nothing: the budget is consulted only
/// while accepted work is outstanding.
const TAIL_PATCH_COLD_START_CLOSURE_TIMEOUT: Duration = Duration::from_secs(20);

/// A 200 ms VAD/energy edge is ordinary quantisation; an uncovered span over
/// 250 ms is not allowed to become terminal transcript truth.
pub const SEAL_COVERAGE_INCOMPLETE_MS: u64 = 250;

/// Stable outward receipt when accepted Layer 1 work cannot land before the
/// Apple seal worker closes. The Apple canvas remains authoritative.
pub const TAIL_PATCH_DRAIN_TIMEOUT_WARNING_CODE: &str = "tail_patch_drain_timeout";

/// Local power was selected but its required live patcher could not arm.
pub const LOCAL_TAIL_PATCH_DEGRADED_WARNING_CODE: &str = "local_tail_patch_degraded";

/// The provider result did not prove that it describes the PCM range owned by
/// the pending span. No transcript text is included in this receipt.
pub const TAIL_PATCH_IDENTITY_MISMATCH_WARNING_CODE: &str = "tail_patch_identity_mismatch";

/// Content-free marker emitted when an Apple final callback contained segment
/// time already committed by an earlier callback. The overlapping portion is
/// removed before a new utterance id can be allocated.
pub const APPLE_FINAL_OVERLAP_WARNING_CODE: &str = "apple_final_window_overlap_normalized";

/// Terminal ledger finality was refused for a structurally meaningful reason.
/// The refusal token is diagnostics-only and never force-seals the document.
pub const LEDGER_TERMINAL_SEAL_REFUSED_WARNING_CODE: &str = "acoustic_ledger_terminal_seal_refused";

/// What the retired char-diff made of one Layer 1 job, and what actually
/// happened to that job's payload.
///
/// The two are separate facts and were being read as one. `verdict = "skipped"`
/// reads like a rejection, and three such lines on one archived take were read
/// as three lost Whisper labels; correlating PCM spans against Bus revisions
/// showed all three had been admitted and had persisted to a later published
/// revision. The verdict never decided that. The one-throne path admits
/// Whisper through `AcousticLedger` on occurrence identity, and `payload` is
/// forwarded on every successful job regardless of what the char-diff said.
///
/// So the receipt states both, and states which one has authority. Text-free by
/// construction: no transcript ever reaches a log line through here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LegacyCharDiffReceipt {
    /// The retired decision, as a token.
    verdict: &'static str,
    /// Whether the provider payload continued to occurrence admission. `true`
    /// for every `Ok` job — including `skipped` and `under_commit`.
    payload_forwarded: bool,
    /// Who actually decides admission. Never this verdict.
    admission_authority: &'static str,
}

/// Ledger admission is bound to occurrence identity, never to a text diff.
const LEDGER_ADMISSION_AUTHORITY: &str = "acoustic_ledger_occurrence_identity";

fn legacy_char_diff_receipt(
    outcome: &TailPatchOutcome,
    payload_forwarded: bool,
) -> LegacyCharDiffReceipt {
    LegacyCharDiffReceipt {
        verdict: match outcome {
            TailPatchOutcome::Patches(_) => "patches",
            TailPatchOutcome::NoChange => "no_change",
            TailPatchOutcome::UnderCommit(_) => "under_commit",
            TailPatchOutcome::Skipped { .. } => "skipped",
        },
        payload_forwarded,
        admission_authority: LEDGER_ADMISSION_AUTHORITY,
    }
}

/// One sealed utterance handed from the worker thread to the async Layer 1 lane.
struct TailPatchRequest {
    /// Worker-owned submission identity; zero is reserved for unsent work.
    submission_sequence: u64,
    utterance_id: u64,
    /// Byte-identical to the emitted `UtteranceFinal.text` — the string every
    /// `ReplaceRange` char offset is computed against.
    committed_text: String,
    /// Canvas already sealed BEFORE this utterance.
    ///
    /// Layer 1 sees one utterance at a time, so a phrase the previous
    /// utterance already carries reads as a gap here and is appended a second
    /// time — measured 2026-08-14 the moment recoveries first reached the
    /// canvas ("…hard pruna I road która pozwoli nam na zrobienie hard Pru."),
    /// which cost more WER than the recovery gained. The neighbour context is
    /// read-only: it is never patched, only consulted so a duplicate is
    /// escalated instead of placed.
    neighbour_context: String,
    /// Exact original capture PCM named by the plan offer.
    audio: Vec<f32>,
    /// Exact capture range behind `audio`; this is the window-start authority.
    provider_request: TailProviderRequest,
    /// Full issued half-open range; every returned pin still needs ledger admission.
    admit_sample_start: u64,
    admit_sample_end: u64,
    /// Physical owners known at submission; more can qualify before return.
    member_occurrences: Vec<(u64, OccurrenceIdentity)>,
}

const LIVE_WORKER_QUANTUM: Duration = Duration::from_millis(40);

#[derive(Debug, Clone, Copy)]
enum RefinementFailure {
    LaneGone,
    PcmUnavailable,
    InvalidIdentity,
    NoLabel,
    StopDeadline,
    NotScheduled,
    QualificationRefused,
}

impl RefinementFailure {
    fn code(self) -> &'static str {
        match self {
            Self::LaneGone => "live_refinement_lane_gone",
            Self::PcmUnavailable => "live_refinement_pcm_unavailable",
            Self::InvalidIdentity => "live_refinement_invalid_identity",
            Self::NoLabel => "live_refinement_no_label",
            Self::StopDeadline => "live_refinement_stop_deadline",
            Self::NotScheduled => "live_refinement_not_scheduled",
            Self::QualificationRefused => "live_refinement_qualification_refused",
        }
    }
}

/// Whisper closure returned to the worker that owns Apple + seal state.
struct TailPatchCompletion {
    /// Worker-owned submission identity; zero is reserved for unsent work.
    submission_sequence: u64,
    utterance_id: u64,
    request_identity: Option<TailRequestIdentity>,
    payload: Option<TailProviderPayload>,
    member_occurrences: Vec<(u64, OccurrenceIdentity)>,
}

/// In-flight Layer 1 job identity, including the coalesce map.
struct TailPatchInFlight {
    /// Worker-owned submission identity; zero is reserved for unsent work.
    submission_sequence: u64,
    utterance_id: u64,
    request_identity: TailRequestIdentity,
    admit_sample_start: u64,
    admit_sample_end: u64,
    member_occurrences: Vec<(u64, OccurrenceIdentity)>,
}

/// One concrete formatter job, keyed only by an existing PCM occurrence.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FormatterRequest {
    occurrence: OccurrenceIdentity,
    existing_label: String,
    source_observation: Option<LedgerObservationIdentity>,
    policy: FormattingPolicy,
}

/// Provider outcome bound to one request over an already sealed label. It
/// travels only to PresentationEmitter as a source-bound text result; the
/// seal worker never waits for it, and it never certifies or delays acoustic
/// finality.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FormatterCompletion {
    occurrence: OccurrenceIdentity,
    proposal: OccurrenceLabelProposal,
}

impl FormatterCompletion {
    fn from_result(request: FormatterRequest, result: AiFormatResult) -> Self {
        let (proposed_label, disposition) = match result.status {
            AiFormatStatus::Applied if !result.text.trim().is_empty() => {
                (result.text, LabelProposalDisposition::Propose)
            }
            AiFormatStatus::Applied | AiFormatStatus::Failed => {
                (String::new(), LabelProposalDisposition::Refuse)
            }
            AiFormatStatus::Skipped | AiFormatStatus::AiNoop => {
                (String::new(), LabelProposalDisposition::PreserveExisting)
            }
        };
        let occurrence = request.occurrence;
        let mut proposal = OccurrenceLabelProposal::for_existing_occurrence(
            occurrence.session.clone(),
            occurrence.capture_epoch,
            occurrence.sample_start,
            occurrence.sample_end,
            proposed_label,
            disposition,
        );
        if let Some(source) = request.source_observation {
            proposal = proposal.with_source(source, request.existing_label, request.policy);
        }
        Self {
            occurrence,
            proposal,
        }
    }

    fn carries_same_occurrence(&self) -> bool {
        self.proposal.session == self.occurrence.session
            && self.proposal.capture_epoch == self.occurrence.capture_epoch
            && self.proposal.sample_start == self.occurrence.sample_start
            && self.proposal.sample_end == self.occurrence.sample_end
            && self.proposal.binds_real_samples()
    }
}

async fn bound_formatter_reply(
    raw: &str,
    deadline: Duration,
    reply: impl std::future::Future<Output = AiFormatResult>,
) -> AiFormatResult {
    tokio::time::timeout(deadline, reply)
        .await
        .unwrap_or_else(|_| AiFormatResult {
            text: raw.to_string(),
            reasoning_text: None,
            status: AiFormatStatus::Failed,
        })
}

/// Build one bounded text job over an already sealed label. The policy and
/// every provider knob come from the take's immutable settings generation; the
/// reply is bounded by that generation's attempt timeout and a timeout keeps
/// the raw label.
fn formatter_job(
    mut request: FormatterRequest,
    runtime_settings: Arc<RuntimeSettingsSnapshot>,
    language: Option<String>,
    chain: Arc<TakeFormatterChain>,
) -> BoxFuture<'static, FormatterCompletion> {
    request.policy = runtime_settings.formatting_policy();
    Box::pin(async move {
        let deadline = runtime_settings
            .ai_execution()
            .request_timing()
            .attempt_timeout();
        // The deadline starts when this job's turn does, not while it waits
        // behind the take's earlier fragment.
        let result = chain
            .next_turn(bound_formatter_reply(
                &request.existing_label,
                deadline,
                format_text_with_status_for_policy(
                    &request.existing_label,
                    language.as_deref(),
                    runtime_settings.as_ref(),
                    None,
                ),
            ))
            .await;
        FormatterCompletion::from_result(request, result)
    })
}

/// Hand one completed text job to the take's PresentationEmitter. The emitter
/// admits the derived text only against its exact sealed source (CAS); the
/// seal worker is not involved because the occurrence was sealed before the
/// job was scheduled. A completion whose proposal lost its PCM identity fails
/// closed here.
fn deliver_formatter_completion(
    completion: FormatterCompletion,
    event_sink: &dyn EventSink,
    stream_log_path: Option<&std::path::Path>,
) -> bool {
    if !completion.carries_same_occurrence() {
        let occurrence = &completion.occurrence;
        warn!(
            session = occurrence.session,
            capture_epoch = occurrence.capture_epoch,
            sample_start = occurrence.sample_start,
            sample_end = occurrence.sample_end,
            "Formatter completion refused — proposal changed exact PCM identity"
        );
        return false;
    }
    deliver_event(
        &EngineEvent::OccurrenceLabelProposal {
            proposal: completion.proposal,
        },
        event_sink,
        stream_log_path,
    );
    true
}

/// Waves of text work a stopped take may still owe: at most one full queue
/// already in flight plus one full queue accepted behind it.
const RETAINED_FORMATTER_WAVES: u32 = 2;

/// How long a stopped take's retained formatter owner may live: its
/// [`RETAINED_FORMATTER_WAVES`] of the generation's attempt timeout. Past it
/// the owner drops its remaining jobs and, with them, the take's event sink.
pub fn retained_formatter_deadline(runtime_settings: &RuntimeSettingsSnapshot) -> Duration {
    runtime_settings
        .ai_execution()
        .request_timing()
        .attempt_timeout()
        .saturating_mul(RETAINED_FORMATTER_WAVES)
}

/// Text jobs a take accepted while it was live, carried past the end of its
/// acoustic session.
///
/// Formatting is text processing over committed labels, so its lifetime is not
/// the microphone's: Stop, PCM coverage and the terminal events never wait for
/// it, and it never holds a seal or the frontier. It is still owned work, not
/// a detached spawn: one task per stopped take, holding exactly the bounded
/// queue (`FORMATTER_QUEUE_CAP` requests) and the jobs already in flight
/// (`FORMATTER_QUEUE_CAP`), every reply bounded by the generation's attempt
/// timeout and the whole owner by [`RETAINED_FORMATTER_WAVES`] of it. Results
/// go only to this take's own emitter, whose reducer is the document
/// authority and whose source CAS refuses a result after a user edit. A
/// successor take has its own emitter, so a late result can never reach it or
/// route a paste.
struct RetainedFormatterJobs {
    requests: mpsc::Receiver<FormatterRequest>,
    jobs: FuturesOrdered<BoxFuture<'static, FormatterCompletion>>,
    runtime_settings: Arc<RuntimeSettingsSnapshot>,
    language: Option<String>,
    chain: Arc<TakeFormatterChain>,
    event_sink: Arc<dyn EventSink>,
    stream_log_path: Option<PathBuf>,
}

impl RetainedFormatterJobs {
    /// Start the owner when accepted work remains. Returns `None` when nothing
    /// is queued or in flight, so a take without text work spawns nothing.
    fn retain(mut self) -> Option<tokio::task::JoinHandle<()>> {
        // The seal worker has finished scheduling. Closing refuses any later
        // send while keeping every already accepted request drainable, so the
        // owner never waits on a sender the worker thread has not dropped yet.
        self.requests.close();
        if self.jobs.is_empty() && self.requests.is_empty() {
            self.event_sink.on_formatter_pending(false);
            return None;
        }
        self.event_sink.on_formatter_pending(true);
        let deadline = retained_formatter_deadline(&self.runtime_settings);
        info!(
            in_flight = self.jobs.len(),
            queued = self.requests.len(),
            deadline_ms = deadline.as_millis() as u64,
            "Formatter text jobs retained beyond the acoustic session"
        );
        Some(tokio::spawn(self.run(deadline)))
    }

    async fn run(mut self, deadline: Duration) {
        let mut delivered = 0u64;
        let mut refused = 0u64;
        let drained = tokio::time::timeout(deadline, async {
            loop {
                tokio::select! {
                    Some(request) = self.requests.recv(), if self.jobs.len() < FORMATTER_QUEUE_CAP => {
                        self.jobs.push_back(formatter_job(
                            request,
                            Arc::clone(&self.runtime_settings),
                            self.language.clone(),
                            Arc::clone(&self.chain),
                        ));
                    }
                    Some(completion) = self.jobs.next() => {
                        if deliver_formatter_completion(
                            completion,
                            self.event_sink.as_ref(),
                            self.stream_log_path.as_deref(),
                        ) {
                            delivered = delivered.saturating_add(1);
                        } else {
                            refused = refused.saturating_add(1);
                        }
                        self.event_sink.on_formatter_pending(
                            !self.jobs.is_empty() || !self.requests.is_empty(),
                        );
                    }
                    else => break,
                }
            }
        })
        .await
        .is_ok();
        self.event_sink.on_formatter_pending(false);
        // On the owner deadline the remaining provider futures are dropped,
        // which cancels their requests; raw labels were never touched.
        let abandoned = (self.jobs.len() + self.requests.len()) as u64;
        if drained {
            info!(
                delivered,
                refused, "Retained formatter text jobs settled after the acoustic session"
            );
        } else {
            warn!(
                delivered,
                refused,
                abandoned,
                "Retained formatter text jobs hit the owner deadline; raw labels kept"
            );
        }
    }
}

impl Drop for RetainedFormatterJobs {
    fn drop(&mut self) {
        self.requests.close();
        self.jobs.clear();
        self.event_sink.on_formatter_pending(false);
    }
}

#[cfg(test)]
mod formatter_deadline_tests {
    use super::*;

    #[tokio::test]
    async fn formatter_deadline_cancels_a_pending_reply_and_preserves_raw() {
        let result = bound_formatter_reply(
            "Iwo Iwo Iwo",
            Duration::from_millis(2),
            std::future::pending(),
        )
        .await;
        assert_eq!(result.status, AiFormatStatus::Failed);
        assert_eq!(result.text, "Iwo Iwo Iwo");
    }
}

#[cfg(test)]
mod retained_formatter_lifecycle_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[derive(Default)]
    struct PendingSink {
        pending: AtomicBool,
        events: AtomicUsize,
    }

    impl EventSink for PendingSink {
        fn on_event(&self, _event: &EngineEvent) {
            self.events.fetch_add(1, Ordering::SeqCst);
        }

        fn on_formatter_pending(&self, pending: bool) {
            self.pending.store(pending, Ordering::SeqCst);
        }
    }

    fn retain_job(
        job: BoxFuture<'static, FormatterCompletion>,
    ) -> (
        Arc<PendingSink>,
        tokio::task::JoinHandle<()>,
        tempfile::TempDir,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let inputs = crate::config::CapturedRuntimeInputs::defaults_at(directory.path().into(), 1);
        let settings = Arc::new(crate::config::Config::runtime_snapshot_from_captured(
            inputs,
        ));
        let (sender, requests) = mpsc::channel(FORMATTER_QUEUE_CAP);
        drop(sender);
        let mut jobs = FuturesOrdered::new();
        jobs.push_back(job);
        let sink = Arc::new(PendingSink::default());
        let owner = RetainedFormatterJobs {
            requests,
            jobs,
            runtime_settings: settings,
            language: None,
            chain: TakeFormatterChain::new(),
            event_sink: sink.clone(),
            stream_log_path: None,
        }
        .retain()
        .unwrap();
        assert!(sink.pending.load(Ordering::SeqCst));
        (sink, owner, directory)
    }

    #[tokio::test]
    async fn retained_formatter_panic_releases_pending_selection_signal() {
        let (sink, owner, _directory) = retain_job(Box::pin(async {
            panic!("injected formatter panic");
        }));
        let error = owner
            .await
            .expect_err("the injected provider future must panic");
        assert!(error.is_panic());
        assert!(
            !sink.pending.load(Ordering::SeqCst),
            "a crashed formatter must not leave Stop waiting on dead work"
        );
        assert_eq!(
            sink.events.load(Ordering::SeqCst),
            0,
            "failure cannot manufacture transcript or acoustic events"
        );
    }

    #[tokio::test]
    async fn retained_formatter_cancellation_releases_pending_selection_signal() {
        let (sink, owner, _directory) = retain_job(Box::pin(std::future::pending()));
        owner.abort();
        let error = owner
            .await
            .expect_err("the retained owner was explicitly cancelled");
        assert!(error.is_cancelled());
        assert!(
            !sink.pending.load(Ordering::SeqCst),
            "cancellation must not leave Stop waiting on dead work"
        );
        assert_eq!(sink.events.load(Ordering::SeqCst), 0);
    }
}

/// Async Layer 1 lane for the Apple progressive path.
///
/// Owns the in-flight Whisper gap-fill job and the replacement count that
/// `SessionFinalised.layer_summary` reports. Jobs are boxed so the lane can be
/// driven by a stub future in tests without a model on disk.
struct AppleTailPatchLane {
    execution: Arc<LocalExecutionOwner>,
    jobs: FuturesOrdered<BoxFuture<'static, Result<TailPatchJobResult>>>,
    language: Option<String>,
    config: TailPatchConfig,
    provider: crate::stt::tail_provider::TailProviderId,
}

impl AppleTailPatchLane {
    /// Open an empty lane. `TailPatchConfig::from_env` is read once here so the
    /// whole session judges every patch against the same thresholds, even if the
    /// env flips mid-hold.
    fn new(
        _sample_rate: u32,
        language: Option<String>,
        provider: crate::stt::tail_provider::TailProviderId,
    ) -> Self {
        Self {
            execution: Arc::new(LocalExecutionOwner::default()),
            jobs: FuturesOrdered::new(),
            language,
            // F2: thresholds stay exactly where the shared primitive puts them.
            config: TailPatchConfig::from_env(),
            provider,
        }
    }

    /// Turn a sealed utterance into a Whisper gap-fill job and queue it. The job
    /// is only constructed — inference runs on a retained native worker, so
    /// this call never sits on the event-drain path.
    fn push_request(&mut self, mut req: TailPatchRequest) {
        req.provider_request.language = self.language.clone();
        let job = compute_tail_patch_job(
            &self.execution,
            TailPatchJobInput {
                utterance_id: req.utterance_id,
                committed_text: req.committed_text,
                neighbour_context: req.neighbour_context,
                audio: req.audio,
                request: req.provider_request,
                config: self.config,
            },
            self.provider,
        );
        self.push_job(job);
    }

    /// Queue an already-built job. Boxed and separate from `push_request` so
    /// tests can drive the lane with a stub future, with no model on disk.
    fn push_job(&mut self, job: BoxFuture<'static, Result<TailPatchJobResult>>) {
        self.jobs.push_back(job);
    }

    /// Await the next finished job. `FuturesOrdered` (not `Unordered`) is the
    /// point: patches must reach the sink in seal order, or a later utterance's
    /// `ReplaceRange` could land before an earlier one's.
    async fn next(&mut self) -> Option<Result<TailPatchJobResult>> {
        self.jobs.next().await
    }

    /// Convert a finished job into the closure message consumed by the
    /// progressive seal owner. The request identity rides separately from the
    /// provider payload so failures can still close the exact pending window.
    fn finish_for_worker(
        &mut self,
        inflight: Option<TailPatchInFlight>,
        result: Result<TailPatchJobResult>,
    ) -> TailPatchCompletion {
        let (submission_sequence, fallback_id, request_identity, member_occurrences) =
            match inflight {
                Some(job) => (
                    job.submission_sequence,
                    job.utterance_id,
                    Some(job.request_identity),
                    job.member_occurrences,
                ),
                None => (0, 0, None, Vec::new()),
            };
        match result {
            Ok(job) => {
                // Counts only. A `Patches` outcome carries transcript text, so
                // the verdict is reduced to a token before it reaches the log.
                // The payload is forwarded whatever the verdict says — the
                // receipt names both facts so neither can be read as the other.
                let receipt = legacy_char_diff_receipt(&job.outcome, true);
                debug!(
                    utterance_id = job.utterance_id,
                    legacy_verdict = receipt.verdict,
                    payload_forwarded = receipt.payload_forwarded,
                    admission_authority = receipt.admission_authority,
                    "Layer 1 char-diff verdict is diagnostic only; the payload continues to \
                     occurrence admission"
                );
                TailPatchCompletion {
                    submission_sequence,
                    utterance_id: job.utterance_id,
                    request_identity,
                    payload: Some(job.payload),
                    member_occurrences,
                }
            }
            Err(error) => {
                warn!(
                    utterance_id = fallback_id,
                    payload_forwarded = false,
                    admission_authority = LEDGER_ADMISSION_AUTHORITY,
                    "Layer 1 provider job failed; Apple text is preserved: {error}"
                );
                TailPatchCompletion {
                    submission_sequence,
                    utterance_id: fallback_id,
                    request_identity,
                    payload: None,
                    member_occurrences,
                }
            }
        }
    }

    /// Hand a completion to the live seal owner and only then account it in
    /// the session receipt. A closed receiver means the worker has already
    /// sealed raw and no patch can reach the canvas.
    fn forward_completion_to_worker(
        &mut self,
        tx: &std_mpsc::Sender<TailPatchCompletion>,
        completion: TailPatchCompletion,
    ) -> bool {
        if tx.send(completion).is_err() {
            return false;
        }
        true
    }
}

/// Deliver one engine event to the sink, writing the same per-utterance
/// diagnostic line the VAD path writes.
///
/// Factored out because the Layer 1 branch must flush every queued event before
/// emitting a patch: a `ReplaceRange` that overtook its own `UtteranceFinal`
/// would address canvas that has not been committed yet.
fn deliver_event(
    event: &EngineEvent,
    event_sink: &dyn EventSink,
    stream_log_path: Option<&std::path::Path>,
) {
    if let (Some(path), EngineEvent::UtteranceFinal { text, .. }) = (stream_log_path, event) {
        let _ = append_to_stream_log(path, text.trim());
    }
    event_sink.on_event(event);
}

const fn formatter_lane_is_armed(policy: FormattingPolicy, lane_available: bool) -> bool {
    !matches!(policy, FormattingPolicy::Off) && lane_available
}

/// Whether *this take* may open a paid formatter slot while it is still live.
///
/// The configuration predicate above answers "is a formatter reachable at all";
/// this one adds the only other question the worker has: does the gesture that
/// opened the microphone want per-occurrence formatting as it happens?
///
/// A one-turn composer take answers no. It is not deduplicated downstream and
/// it is not counted and discarded — the sender simply never exists, so
/// `schedule_formatter_for_sealed_label` cannot reserve a permit and no
/// sealed label is handed to a formatter. The turn is formatted once at
/// terminal processing by the controller instead.
/// A capture take never carries Max: admission seals the automatic policy
/// (Off, Correction or Smart), and Max runs only for an explicit Format action
/// on the retained document. Max still refuses this occurrence lane, because it
/// is the Agent and its tools never run on isolated words.
fn live_formatter_lane_is_armed(
    capture_turn: CaptureTurnIntent,
    policy: FormattingPolicy,
    lane_available: impl FnOnce() -> bool,
) -> bool {
    capture_turn.schedules_live_formatting()
        && !matches!(policy, FormattingPolicy::Max)
        && formatter_lane_is_armed(policy, true)
        && lane_available()
}

/// Surface one Layer 1 lane degrade as a counts-only warning event.
///
/// The message is the typed reason token and nothing else — no transcript,
/// audio, provider payload, or endpoint detail can ride this event into a log.
fn emit_layer1_degrade_warning(event_sink: &dyn EventSink, reason: Layer1DegradeReason) {
    event_sink.on_event(&EngineEvent::Warning {
        code: LAYER1_DEGRADED_WARNING_CODE.to_string(),
        message: reason.as_token().to_string(),
    });
}

/// Emit the stop receipt once for collected finals, without transcript content.
fn emit_layer1_finals_not_admitted_warning(
    event_sink: &dyn EventSink,
    finals_accepted: u64,
    refiner: crate::asr_session::RefinerMode,
) {
    if finals_accepted == 0 {
        return;
    }
    let refiner = refiner.as_token();
    warn!(
        code = LAYER1_FINALS_NOT_ADMITTED_WARNING_CODE,
        finals_accepted, refiner, "Layer 1 finals arrived after the worker exited"
    );
    event_sink.on_event(&EngineEvent::Warning {
        code: LAYER1_FINALS_NOT_ADMITTED_WARNING_CODE.to_string(),
        message: format!(
            "finals_accepted={finals_accepted} refiner={refiner}: Layer 1 finals arrived after the worker exited"
        ),
    });
}

fn emit_local_tail_patch_degraded_warning(event_sink: &dyn EventSink, disposition: &str) {
    event_sink.on_event(&EngineEvent::Warning {
        code: LOCAL_TAIL_PATCH_DEGRADED_WARNING_CODE.to_string(),
        message: disposition.to_string(),
    });
}

/// Report abandoned local tail-patch work exactly once, before session finality.
fn report_tail_patch_drain_degrade(event_sink: &dyn EventSink, abandoned: u64) {
    if abandoned == 0 {
        return;
    }
    event_sink.on_event(&EngineEvent::Warning {
        code: TAIL_PATCH_DRAIN_TIMEOUT_WARNING_CODE.to_string(),
        message: format!(
            "{abandoned} accepted Layer 1 tail-patch job(s) missed the bounded stop drain; Apple live text was preserved"
        ),
    });
}

/// Preserve a terminal ledger refusal on the ordered diagnostic surface.
/// `NotQualified` is the one quiet outcome: it means no physical speech was
/// admitted, not that a known occurrence failed to close.
fn report_terminal_seal_refusal(ev_tx: &mpsc::UnboundedSender<EngineEvent>, refusal: SealRefusal) {
    if refusal == SealRefusal::NotQualified {
        return;
    }
    let _ = ev_tx.send(EngineEvent::Warning {
        code: LEDGER_TERMINAL_SEAL_REFUSED_WARNING_CODE.to_string(),
        message: refusal.as_str().to_string(),
    });
}

/// Reconcile job-level terminal buckets after the worker's bounded closure
/// loop. No-change, provider skip, and rewrite-fence refusal all land in
/// `skipped`; `applied` means a completed job whose bounded mutation survived.
#[derive(Clone, Copy)]
struct TailPatchWorkerAccounting {
    applied_jobs: u64,
    skipped_jobs: u64,
    timeout_residue: u64,
}

fn tail_patch_receipt_after_stop(
    armed: bool,
    submitted: u64,
    worker_accounting: Option<TailPatchWorkerAccounting>,
    conservation: SessionConservationReceipt,
) -> TailPatchSessionReceipt {
    // The worker increments its awaiting-completion counter before the async
    // owner accepts a request. On bounded closure expiry, that counter already
    // owns every async in-flight/queued request as `timed_out`; the async side
    // must not classify the same requests again as `abandoned`. Abandonment is
    // reserved for the distinct route where the worker returns no accounting.
    let (applied_jobs, skipped_jobs, timeout_residue, abandoned_jobs) = match worker_accounting {
        Some(accounting) => (
            accounting.applied_jobs,
            accounting.skipped_jobs,
            accounting.timeout_residue,
            0,
        ),
        None => (0, 0, 0, submitted),
    };
    // `new` names any remaining gap as abandoned. A mismatch must not abort the take.
    TailPatchSessionReceipt::new(
        armed,
        submitted,
        applied_jobs,
        skipped_jobs,
        timeout_residue,
        abandoned_jobs,
        if !armed {
            TailPatchDrainDisposition::NotArmed
        } else if timeout_residue > 0 {
            TailPatchDrainDisposition::TimedOut
        } else if abandoned_jobs > 0 {
            TailPatchDrainDisposition::Abandoned
        } else {
            TailPatchDrainDisposition::Completed
        },
    )
    .with_conservation(conservation)
}

/// Drive one progressive Apple stream session until the audio channel closes.
pub(crate) async fn apple_stream_transcription_session(
    chunk_receiver: mpsc::Receiver<Vec<f32>>,
    event_sink: Arc<dyn EventSink>,
    config: SessionConfig,
) {
    let SessionConfig {
        session_id,
        capture_epoch,
        runtime_settings,
        live_formatting_agent,
        acoustic_ledger,
        sample_rate,
        capture_device_name,
        language,
        stream_log_path,
        utterance_silence_sec,
        capture_turn,
        layer1,
        mut lifecycle_events,
        terminal_audio,
        mut last_window_closed,
    } = config;
    // Arm the hold witness for this take's shared ledger: if any holder wedges
    // mid-take, the receipt names the lock instead of the take ending silently.
    crate::pipeline::acoustic_ledger::witness::watch(&acoustic_ledger, &session_id);
    let seal_ledger_enabled = runtime_settings.seal_ledger_enabled();
    // One owner for this capture epoch's acoustic evidence. This async arm is
    // the writer and the blocking Apple worker below is the reader; both hold
    // the same handle, so no process-global slot and no reset entrypoint sit
    // between them.
    let capture_energy = CaptureEnergyOwner::bind(session_id.clone(), capture_epoch);
    let mut capture_level = CaptureLevelAccumulator::bound_to_at_rate(&capture_energy, sample_rate);
    let input_plan = SpeechInputPlan::for_capture(
        &runtime_settings,
        capture_device_name.as_deref(),
        sample_rate,
    );
    let input_enhanced = input_plan.is_enhanced();
    let mut chunk_receiver = SpeechInputStream::open(chunk_receiver, input_plan, &session_id);
    let input_archive = chunk_receiver.archive();
    // Hands-free silence is the ENGINE LIFECYCLE on this lane, not a chunker
    // knob: SFSpeech still owns phrase boundaries inside an utterance, but the
    // threshold decides when the engine rests (mic + Silero keep watching) and
    // when a fresh epoch wakes on the next speech edge. Unset = one continuous
    // stream for the whole take, the pre-lifecycle behaviour.
    if let Some(sec) = utterance_silence_sec {
        info!(
            utterance_silence_sec = sec,
            "Apple progressive live mode: engine lifecycle armed on the hands-free silence \
             threshold (speech epochs)"
        );
    }

    info!(
        sample_rate,
        "Apple progressive live session started (stream multi-seal)"
    );
    // The controller owns this identity and the one immutable settings read.
    // Apple may consume both but may not mint a lane-local session or reload.
    let settings_digest = runtime_settings.digest().as_str().to_string();

    // C1: split the one recording-start decision into its explicit local
    // exact-span disposition and (when Cloud is selected) the injected generic
    // provider. Construction and consent live with the settings owner.
    let lane_input = Layer1SessionInput {
        session_id: Layer1SessionId::new(session_id.clone())
            .expect("uuid session ids are never blank"),
        locale: language.clone(),
        sample_rate,
    };
    let local_tail_patch = layer1.local_tail_patch_disposition();
    let mut layer1_lane = RecorderLayer1Lane::open(layer1, &lane_input);
    // Preserve the opened lane label before stop or degradation drops its provider.
    let layer1_refiner = layer1_lane.refiner_mode();
    if let Some(reason) = layer1_lane.take_degrade_notice() {
        emit_layer1_degrade_warning(event_sink.as_ref(), reason);
    }

    // PCM → worker (None = EOF). Unbounded so the async select loop never
    // blocks on a full sync_channel while live Preview events wait to drain
    // (bounded sync_channel + blocking send would re-stall presentation).
    let (pcm_tx, pcm_rx) = std_mpsc::channel::<Result<Option<SpeechInputFrame>, String>>();
    // Worker → async events.
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel::<EngineEvent>();
    let (worker_close_tx, mut worker_close_rx) = tokio::sync::oneshot::channel();
    let (apple_finished_tx, mut apple_finished_rx) = tokio::sync::oneshot::channel();
    let (live_finals_tx, mut live_finals_rx) = tokio::sync::oneshot::channel();

    // The local Whisper decision is resolved once from product mode + the
    // compatibility phase token before capture starts. Never re-read env here:
    // Settings, replay, logging, and runtime must all observe one decision.
    let tail_patch_on = local_tail_patch.is_some_and(|decision| decision.is_armed());
    if tail_patch_on {
        info!(
            disposition = local_tail_patch
                .map(|decision| decision.as_token())
                .unwrap_or("not_applicable"),
            "Local Whisper tail-patch armed on Apple progressive path"
        );
    } else if let Some(disposition) = local_tail_patch {
        warn!(
            disposition = disposition.as_token(),
            "Local power degraded: required Whisper tail-patch is not armed"
        );
        emit_local_tail_patch_degraded_warning(event_sink.as_ref(), disposition.as_token());
    }
    let mut tail_patch_lane = AppleTailPatchLane::new(
        sample_rate,
        language.clone(),
        runtime_settings
            .tail_provider()
            .unwrap_or(crate::stt::tail_provider::TailProviderId::InProcess),
    );
    // Direct publication needs the decoder payload, never a computed char patch.
    // The existing in-flight gate and execution owner bound both job kinds.
    let mut direct_decode_job: Option<BoxFuture<'static, Result<TailProviderPayload>>> = None;
    // At-most-one-in-flight gate (F1), tracked outside the lane so the admit
    // branch's guard does not borrow what the collect branch holds mutably.
    let mut tail_patch_in_flight = false;
    let mut tail_patch_lane_in_flight: Option<TailPatchInFlight> = None;
    let mut tail_patch_submitted = 0u64;
    // Bounded: the worker `try_send`s from the PCM-forwarding thread.
    let (tp_tx, mut tp_rx) = mpsc::channel::<TailPatchRequest>(TAIL_PATCH_QUEUE_CAP);
    let (tp_done_tx, tp_done_rx) = std_mpsc::channel::<TailPatchCompletion>();
    // Layered off → the worker gets no sender at all, so the lane stays empty
    // and its branch never yields: zero jobs, zero behaviour change.
    let worker_tp_tx = tail_patch_on.then_some(tp_tx);
    if capture_turn == CaptureTurnIntent::AgentChannel && !tail_patch_on {
        event_sink.on_event(&EngineEvent::Warning {
            code: "agent_raw_whisper_disabled".into(),
            message: "Whisper lane disabled for this take; Raw is not Whisper-refined.".into(),
        });
    }

    // Formatting consumes only facts frozen into this exact per-take snapshot.
    // Arming the transport schedules nothing; a sealed occurrence must acquire
    // a bounded queue permit first. Apple on-device counts as an engine on its
    // own: it needs no cloud key, endpoint or account.
    let formatter_on = seal_ledger_enabled
        && live_formatter_lane_is_armed(capture_turn, runtime_settings.formatting_policy(), || {
            crate::llm::ai_formatting::text_formatting_unavailable_reason(&runtime_settings)
                .is_none()
        });
    if !capture_turn.schedules_live_formatting() {
        info!(
            "Capture intent: the live formatter lane stays unarmed for this take, so no \
             silence-delimited fragment can open a paid provider slot"
        );
    }
    // This take's own formatter conversation: its fragments go in order, each
    // with the ones before it, and none of it reaches another take.
    let formatter_chain = TakeFormatterChain::new();
    let mut formatter_jobs = FuturesOrdered::<BoxFuture<'static, FormatterCompletion>>::new();
    let formatter_runtime_settings = Arc::clone(&runtime_settings);
    let formatter_language = language.clone();
    let (formatter_tx, mut formatter_rx) = mpsc::channel::<FormatterRequest>(FORMATTER_QUEUE_CAP);
    let worker_formatter_tx = formatter_on.then_some(formatter_tx);

    let cloud_on = layer1_lane.is_live()
        && layer1_lane.refiner_mode() == crate::asr_session::RefinerMode::CloudSession;
    let (cloud_commit_tx, cloud_commit_rx) = mpsc::channel::<u64>(32);
    let mut cloud_commit_rx = cloud_on.then_some(cloud_commit_rx);
    let (cloud_notice_tx, cloud_notice_rx) = std_mpsc::channel::<CloudWorkerNotice>();
    let cloud_notice_tx = cloud_on.then_some(cloud_notice_tx);
    let worker_cloud = cloud_on.then_some(CloudWorkerChannels {
        commit: cloud_commit_tx,
        notice: cloud_notice_rx,
    });

    let (consultation_tx, mut consultation_rx) = mpsc::channel(CONSULTATION_QUEUE_CAP);
    let (consultation_return_tx, consultation_return_rx) = std_mpsc::channel();
    let mut consultation_assessments =
        FuturesOrdered::<BoxFuture<'static, Result<ConsultationReadiness>>>::new();
    let mut consultation_preparations =
        FuturesOrdered::<BoxFuture<'static, Result<PreparedConsultationGroup>>>::new();
    let mut consultation_answers =
        FuturesOrdered::<BoxFuture<'static, Result<ConsultationGroupAnswer>>>::new();
    if live_formatting_agent.is_some() && event_sink.consultation_destinations() != 1 {
        event_sink.on_event(&EngineEvent::Warning {
            code: "max_consultation_destination_unavailable".into(),
            message: "Live Max requires exactly one configured presentation destination.".into(),
        });
    }
    // Grouped consultation arms only for a Max take snapshot. Capture admission
    // seals the automatic policy, so a product capture never satisfies this;
    // the host supplies no executor to a capture either.
    let live_formatting_agent = live_formatting_agent.filter(|_| {
        seal_ledger_enabled
            && capture_turn.schedules_live_formatting()
            && runtime_settings.formatting_policy() == FormattingPolicy::Max
            && event_sink.consultation_destinations() == 1
    });
    let worker_consultation = live_formatting_agent
        .as_ref()
        .map(|_| LiveConsultationCapture {
            queue: ConsultationInputQueue::new(session_id.clone(), capture_epoch)
                .expect("recorder owns a valid capture identity"),
            requests: consultation_tx,
            returns: consultation_return_rx,
            last_assessed: None,
            assessment_pending: false,
            answers_pending: 0,
            speech_open: false,
            refused: false,
        });

    let worker_session_id = session_id.clone();
    let worker_capture_energy = capture_energy.clone();
    let worker_execution = Arc::clone(&tail_patch_lane.execution);
    let worker = thread::spawn(move || {
        apple_stream_worker(
            pcm_rx,
            ev_tx,
            worker_tp_tx,
            tp_done_rx,
            worker_formatter_tx,
            AppleWorkerConfig {
                local_execution: worker_execution,
                input_enhanced,
                input_archive,
                sample_rate,
                capture_device_name,
                language: language.as_deref(),
                session_id: worker_session_id,
                capture_epoch,
                capture_energy: worker_capture_energy,
                runtime_settings,
                acoustic_ledger,
                settings_digest,
                capture_turn,
                utterance_silence_sec,
                terminal_audio,
                last_window_closed: worker_close_tx,
                apple_finished: apple_finished_tx,
                live_finals_admitted: live_finals_tx,
                consultation: worker_consultation,
                cloud: worker_cloud,
            },
        )
    });

    // CRITICAL (operator 2026-07-27 — live preview "blocked" on overlay):
    // PCM forward and EngineEvent drain MUST interleave. The previous shape
    // drained `ev_rx` only *after* `chunk_receiver` closed (key-up / stop), so
    // every `Preview` / mid-stream `UtteranceFinal` sat in the unbounded queue
    // until EOF. The engine had letter-level partials; the overlay saw nothing
    // until the session ended. Product truth: presentation was missing, not STT.
    let mut audio_eof = false;
    let mut input_eof = false;
    let mut worker_finished = false;
    let mut close_ack_pending = true;
    let mut apple_finish_pending = true;
    let mut live_finals_pending = true;
    let mut cloud_stop_task: Option<
        tokio::task::JoinHandle<(
            RecorderLayer1Lane,
            crate::asr_session::recorder::Layer1SessionOutcome,
        )>,
    > = None;
    let mut stopped_layer1_outcome = None;
    let mut stop_live_tick = tokio::time::interval(LIVE_WORKER_QUANTUM);
    loop {
        tokio::select! {
            // Once capture is closed, a quiet Apple finish must not prevent
            // the cloud owner from observing a final or a lost live lane.
            _ = stop_live_tick.tick(), if audio_eof && cloud_on && cloud_stop_task.is_none() => {}
            close = &mut worker_close_rx, if close_ack_pending => {
                close_ack_pending = false;
                if close.is_ok() {
                    // Preserve seal acknowledgement ordering, but only the
                    // live-final receipt below can release the stop snapshot.
                    while let Ok(event) = ev_rx.try_recv() {
                        deliver_event(&event, event_sink.as_ref(), stream_log_path.as_deref());
                    }
                }
            }
            settled = &mut live_finals_rx, if live_finals_pending => {
                live_finals_pending = false;
                event_sink.on_formatter_pending(
                    !formatter_jobs.is_empty() || !formatter_rx.is_empty(),
                );
                forward_live_finals_admitted(
                    settled.is_ok(),
                    &mut ev_rx,
                    event_sink.as_ref(),
                    stream_log_path.as_deref(),
                    &mut last_window_closed,
                );
            }
            finished = &mut apple_finished_rx, if apple_finish_pending => {
                apple_finish_pending = false;
                if finished.is_ok() && cloud_on {
                    // Apple has emitted its last capture-owned close requests.
                    // Process those before `end`, then move the provider's
                    // bounded blocking close off the event-drain executor.
                    if let Some(receiver) = cloud_commit_rx.as_mut() {
                        while let Ok(sample) = receiver.try_recv() {
                            let _late = layer1_lane.commit_through(sample);
                        }
                    }
                    cloud_commit_rx = None;
                    let mut stopping_lane = std::mem::replace(
                        &mut layer1_lane,
                        RecorderLayer1Lane::open(
                            crate::asr_session::recorder::Layer1Decision::Disarmed,
                            &lane_input,
                        ),
                    );
                    cloud_stop_task = Some(tokio::task::spawn_blocking(move || {
                        let outcome = stopping_lane.stop();
                        (stopping_lane, outcome)
                    }));
                }
            }
            stopped = async {
                match cloud_stop_task.as_mut() {
                    Some(task) => task.await,
                    None => std::future::pending().await,
                }
            } => {
                cloud_stop_task = None;
                match stopped {
                    Ok((lane, outcome)) => {
                        layer1_lane = lane;
                        let succeeded = outcome.degrade_reason().is_none()
                            && cloud_stop_finals_cover_capture(
                                outcome.finals(),
                                layer1_lane.pushed_samples(),
                            );
                        if let Some(sender) = cloud_notice_tx.as_ref() {
                            for event in layer1_lane.take_unforwarded_finals() {
                                if sender.send(CloudWorkerNotice::Final(Box::new(event))).is_err() {
                                    layer1_lane.note_final_after_worker_exit();
                                }
                            }
                            let notice = if succeeded {
                                CloudWorkerNotice::EndSettled
                            } else {
                                CloudWorkerNotice::LaneLost
                            };
                            let _ = sender.send(notice);
                        }
                        stopped_layer1_outcome = Some(outcome);
                    }
                    Err(error) => {
                        warn!(%error, "Cloud live stop worker failed");
                        if let Some(sender) = cloud_notice_tx.as_ref() {
                            let _ = sender.send(CloudWorkerNotice::LaneLost);
                        }
                    }
                }
            }
            event = ev_rx.recv(), if !worker_finished => {
                match event {
                    // Same diagnostic artifact the VAD path writes: one line
                    // per committed utterance (CODESCRIBE_STREAM_LOG).
                    Some(event) => deliver_event(
                        &event,
                        event_sink.as_ref(),
                        stream_log_path.as_deref(),
                    ),
                    // Worker dropped the sender — stream finished.
                    None => worker_finished = true,
                }
            }
            Some(request) = consultation_rx.recv() => {
                match request {
                    LiveConsultationRequest::Assess(input) => {
                        if let Some(agent) = live_formatting_agent.as_ref() {
                            let agent = Arc::clone(agent);
                            let settings = Arc::clone(&formatter_runtime_settings);
                            consultation_assessments.push_back(Box::pin(async move {
                                agent.assess_group(input, &settings).await
                            }));
                        }
                    }
                    LiveConsultationRequest::Prepare(input) => {
                        if let Some(agent) = live_formatting_agent.as_ref() {
                            let agent = Arc::clone(agent);
                            let settings = Arc::clone(&formatter_runtime_settings);
                            consultation_preparations.push_back(Box::pin(async move {
                                tokio::task::spawn_blocking(move || agent.prepare_group(input, &settings))
                                    .await.map_err(|error| anyhow::anyhow!("Max preparation worker failed: {error}"))?
                            }));
                        }
                    }
                    LiveConsultationRequest::Finish(pending) => {
                        consultation_answers.push_back(Box::pin(pending.finish()));
                    }
                }
            }
            Some(result) = consultation_assessments.next() => {
                deliver_consultation_result(LiveConsultationResult::Assessed(result),
                    &consultation_return_tx, event_sink.as_ref());
            }
            Some(result) = consultation_preparations.next() => {
                deliver_consultation_result(LiveConsultationResult::Prepared(result),
                    &consultation_return_tx, event_sink.as_ref());
            }
            Some(result) = consultation_answers.next() => {
                // A fast provider must not overtake the source ledger events
                // already emitted by the capture worker on the other channel.
                while let Ok(event) = ev_rx.try_recv() {
                    deliver_event(&event, event_sink.as_ref(), stream_log_path.as_deref());
                }
                deliver_consultation_result(LiveConsultationResult::Answer(result),
                    &consultation_return_tx, event_sink.as_ref());
            }
            chunk = chunk_receiver.recv(), if !audio_eof => {
                match chunk {
                    Some(Ok(frame)) => {
                        if let Some(receiver) = cloud_commit_rx.as_mut() {
                            while let Ok(sample) = receiver.try_recv() {
                                let _late = layer1_lane.commit_through(sample);
                            }
                        }
                        capture_level.push_samples(frame.raw_samples());
                        let chunk = &frame.samples;
                        // C1 fan-out: offer the frame to the Layer 1 lane
                        // before forwarding to the Apple worker. The offer
                        // returns immediately, always — a refiner that cannot
                        // keep up costs refinement frames, never capture, and
                        // sustained overflow degrades the lane instead of
                        // exerting backpressure here.
                        layer1_lane.offer_pcm(chunk);
                        if pcm_tx.send(Ok(Some(frame))).is_err() {
                            warn!("Apple live stream worker dropped PCM channel");
                            audio_eof = true;
                        }
                    }
                    Some(Err(error)) => {
                        // The worker closes with an explicit failure; preserve
                        // the existing capture-drain and SessionFinalised path.
                        let _ = pcm_tx.send(Err(error));
                        audio_eof = true;
                    }
                    None => {
                        input_eof = true;
                        // Capture stopped — push the held tail, then signal EOF.
                        // `end` is the lane's close after the worker has admitted
                        // the commits this flush makes possible.
                        layer1_lane.flush_holdback();
                        let _ = pcm_tx.send(Ok(None));
                        audio_eof = true;
                    }
                }
            }
            lifecycle = async {
                match lifecycle_events.as_mut() {
                    Some(events) => events.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                match lifecycle {
                    Some(event) => {
                        apply_recorder_lifecycle_event(&mut layer1_lane, event);
                        if let Some(reason) = layer1_lane.take_degrade_notice() {
                            emit_layer1_degrade_warning(event_sink.as_ref(), reason);
                            if let Some(sender) = cloud_notice_tx.as_ref() {
                                let _ = sender.send(CloudWorkerNotice::LaneLost);
                                drop(last_window_closed.take());
                            }
                        }
                    }
                    None => lifecycle_events = None,
                }
            }
            // Admit one sealed utterance into Layer 1 at a time. The Whisper
            // call itself runs on a retained native worker, so this loop
            // only ever schedules and collects — inference never sits on the
            // event-drain path (F1).
            Some(mut req) = tp_rx.recv(), if !tail_patch_in_flight => {
                info!(
                    session = %req.provider_request.identity.range.session,
                    capture_epoch = req.provider_request.identity.range.capture_epoch,
                    sample_start = req.provider_request.identity.range.sample_start,
                    sample_end = req.provider_request.identity.range.sample_end,
                    request_id = req.provider_request.identity.request_id,
                    disposition = "provider_started",
                    "live_refinement"
                );
                tail_patch_submitted = tail_patch_submitted.saturating_add(1);
                let inflight = TailPatchInFlight {
                    submission_sequence: req.submission_sequence,
                    utterance_id: req.utterance_id,
                    request_identity: req.provider_request.identity.clone(),
                    admit_sample_start: req.admit_sample_start,
                    admit_sample_end: req.admit_sample_end,
                    member_occurrences: req.member_occurrences.clone(),
                };
                if seal_ledger_enabled {
                    tail_patch_lane.push_request(req);
                } else {
                    req.provider_request.language = tail_patch_lane.language.clone();
                    let provider = tail_patch_lane.provider;
                    let audio = req.audio;
                    let request = req.provider_request;
                    let receiver = tail_patch_lane.execution.spawn(move |control| {
                        crate::stt::tail_provider::transcribe_selected_controlled(
                            provider, &request, &audio, control,
                        )
                    });
                    direct_decode_job = Some(Box::pin(async move {
                        receiver?
                            .await
                            .map_err(|error| anyhow::anyhow!("direct decoder worker: {error}"))?
                    }));
                }
                tail_patch_in_flight = true;
                // One job is in flight; the coalesce map rides alongside so
                // the completion can close every member seal.
                tail_patch_lane_in_flight = Some(inflight);
            }
            sample = async {
                let Some(receiver) = cloud_commit_rx.as_mut() else {
                    std::future::pending().await
                };
                receiver.recv().await
            } => {
                match sample {
                    Some(sample) => {
                        let _late = layer1_lane.commit_through(sample);
                    }
                    None => cloud_commit_rx = None,
                }
            }
            Some(result) = tail_patch_lane.next() => {
                tail_patch_in_flight = false;
                let inflight = tail_patch_lane_in_flight.take();
                let completion = tail_patch_lane.finish_for_worker(inflight, result);
                let rejected_id = completion.utterance_id;
                if !tail_patch_lane.forward_completion_to_worker(&tp_done_tx, completion) {
                    warn!(
                        utterance_id = rejected_id,
                        "Layer 1 completion rejected — Apple seal worker already closed"
                    );
                }
            }
            result = async {
                match direct_decode_job.as_mut() {
                    Some(job) => job.await,
                    None => std::future::pending().await,
                }
            }, if !seal_ledger_enabled && tail_patch_in_flight => {
                direct_decode_job = None;
                tail_patch_in_flight = false;
                let Some(inflight) = tail_patch_lane_in_flight.take() else {
                    warn!("Direct decoder completion has no accepted request");
                    continue;
                };
                let payload = match result {
                    Ok(payload) => Some(payload),
                    Err(error) => {
                        event_sink.on_event(&EngineEvent::Warning {
                            code: "direct_decode_failed".into(),
                            message: error.to_string(),
                        });
                        None
                    }
                };
                let completion = TailPatchCompletion {
                    submission_sequence: inflight.submission_sequence,
                    utterance_id: inflight.utterance_id,
                    request_identity: Some(inflight.request_identity),
                    payload,
                    member_occurrences: inflight.member_occurrences,
                };
                if !tail_patch_lane.forward_completion_to_worker(&tp_done_tx, completion) {
                    warn!("Direct decoder returned after its capture worker closed");
                }
            }
            Some(request) = formatter_rx.recv(), if formatter_jobs.len() < FORMATTER_QUEUE_CAP => {
                // The request channel is independent from `ev_rx`. Drain every
                // already-enqueued ledger observation before provider work can
                // complete, so a fast formatter cannot overtake the reducer
                // revision that established its current label.
                while let Ok(event) = ev_rx.try_recv() {
                    deliver_event(
                        &event,
                        event_sink.as_ref(),
                        stream_log_path.as_deref(),
                    );
                }
                formatter_jobs.push_back(formatter_job(
                    request,
                    Arc::clone(&formatter_runtime_settings),
                    formatter_language.clone(),
                    Arc::clone(&formatter_chain),
                ));
                event_sink.on_formatter_pending(true);
            }
            Some(completion) = formatter_jobs.next() => {
                let _ = deliver_formatter_completion(
                    completion,
                    event_sink.as_ref(),
                    stream_log_path.as_deref(),
                );
                event_sink.on_formatter_pending(
                    !formatter_jobs.is_empty() || !formatter_rx.is_empty(),
                );
            }
        }
        // C1: drain whatever the Layer 1 provider has ready. Partials stay
        // volatile draft inside the lane (never canvas); finals pass the
        // ingest doctrine. Non-blocking, so live Preview drainage above is
        // never delayed by the refiner.
        layer1_lane.poll();
        if let Some(sender) = cloud_notice_tx.as_ref() {
            for event in layer1_lane.take_unforwarded_finals() {
                if sender
                    .send(CloudWorkerNotice::Final(Box::new(event)))
                    .is_err()
                {
                    layer1_lane.note_final_after_worker_exit();
                }
            }
        }
        if let Some(reason) = layer1_lane.take_degrade_notice() {
            emit_layer1_degrade_warning(event_sink.as_ref(), reason);
            if let Some(sender) = cloud_notice_tx.as_ref() {
                let _ = sender.send(CloudWorkerNotice::LaneLost);
                drop(last_window_closed.take());
            }
        }
        if worker_finished
            && !live_finals_pending
            && cloud_stop_task.is_none()
            && consultation_rx.is_closed()
            && consultation_rx.is_empty()
            && consultation_assessments.is_empty()
            && consultation_preparations.is_empty()
            && consultation_answers.is_empty()
        {
            break;
        }
    }

    // The acoustic session is over; accepted text jobs are not. Every ledger
    // event was delivered before `ev_rx` closed, so no result can overtake the
    // revision that established its label. The owner runs beside the teardown
    // below and is never awaited: microphone release, PCM coverage and
    // SessionFinalised do not wait for a provider.
    let _retained_formatter = RetainedFormatterJobs {
        requests: formatter_rx,
        jobs: formatter_jobs,
        runtime_settings: Arc::clone(&formatter_runtime_settings),
        language: formatter_language.clone(),
        chain: formatter_chain,
        event_sink: Arc::clone(&event_sink),
        stream_log_path: stream_log_path.clone(),
    }
    .retain();

    // Worker exited (event channel closed). If audio is still open, keep
    // consuming to EOF so upstream capture senders never hit a dropped
    // channel — an early engine death (e.g. bridge spawn failure) must not
    // turn live audio callbacks into send errors. Mirrors the pre-interleave
    // contract where the session always outlived the audio stream.
    //
    // This comes before the Layer 1 backlog on purpose: capture-sender safety
    // is the older, harder contract, and Whisper must never run while live
    // audio is still being drained.
    if !input_eof {
        while chunk_receiver.recv().await.is_some() {}
    }

    // `ev_rx` closes only after the worker's bounded closure loop has assigned
    // every accepted request still awaiting completion to its timeout bucket.
    // Their results cannot change the closed ledger. Cancel admission and join
    // actual executions before SessionFinalised; accounting is not execution.
    let mut outstanding_tail_patch_jobs = u64::from(tail_patch_in_flight);
    while tp_rx.try_recv().is_ok() {
        tail_patch_submitted = tail_patch_submitted.saturating_add(1);
        outstanding_tail_patch_jobs = outstanding_tail_patch_jobs.saturating_add(1);
    }
    if outstanding_tail_patch_jobs > 0 {
        warn!(
            outstanding_tail_patch_jobs,
            "Layer 1 results discarded after terminal accounting; joining retained execution"
        );
    }
    tail_patch_lane.execution.close_and_join().await;
    // CLOUD has already ended while its worker could admit the final. Other
    // modes retain their existing terminal lane accounting.
    let layer1_outcome = stopped_layer1_outcome.unwrap_or_else(|| layer1_lane.stop());
    if let Some(reason) = layer1_lane.take_degrade_notice() {
        emit_layer1_degrade_warning(event_sink.as_ref(), reason);
    }
    let layer1_counts = layer1_outcome.telemetry();
    let finals_after_worker = layer1_lane.unforwarded_finals();
    emit_layer1_finals_not_admitted_warning(
        event_sink.as_ref(),
        finals_after_worker,
        layer1_refiner,
    );
    if layer1_counts.frames_offered > 0 || layer1_counts.finals_accepted > 0 {
        info!(
            frames_forwarded = layer1_counts.frames_forwarded,
            overflow_frame_drops = layer1_counts.overflow_frame_drops,
            partials_applied = layer1_counts.partials_applied,
            finals_accepted = layer1_counts.finals_accepted,
            events_rejected = layer1_counts.events_rejected,
            provider_errors = layer1_counts.provider_errors,
            degrade_reason = layer1_outcome
                .degrade_reason()
                .map(|reason| reason.as_token())
                .unwrap_or("none"),
            "Layer 1 live lane closed"
        );
    }

    let mut accepted_tail_patch_replacements = 0u64;
    let mut lexicon_rewrites = 0u64;
    let mut lexicon_entries_custom = 0usize;
    let mut tail_patch_worker_accounting = None;
    let mut conservation = SessionConservationReceipt::default();
    match worker.join() {
        Ok(Ok(outcome)) => {
            info!(
                sealed = outcome.sealed,
                filtered_empty_drops = outcome.filtered_empty_drops,
                unresolved_windows = outcome.unresolved_windows,
                under_commit_escalations = outcome.under_commit_escalations,
                tail_patch_replacements = outcome.tail_patch_replacements,
                tail_patch_refusals = outcome.tail_patch_refusals,
                "Apple progressive live session finished"
            );
            accepted_tail_patch_replacements = outcome.tail_patch_replacements;
            lexicon_rewrites = outcome.lexicon_rewrites;
            lexicon_entries_custom = outcome.lexicon_entries_custom;
            conservation = outcome.conservation.clone();
            tail_patch_worker_accounting = Some(TailPatchWorkerAccounting {
                applied_jobs: outcome.tail_patch_jobs_applied,
                skipped_jobs: outcome.tail_patch_jobs_skipped,
                timeout_residue: outcome.tail_patch_timeout_residue,
            });
        }
        Ok(Err(e)) => {
            warn!("Apple live stream worker failed: {e:#}");
            event_sink.on_event(&EngineEvent::NoSpeech {
                reason: format!("apple_live_stream_worker: {e:#}"),
            });
        }
        Err(_) => {
            warn!("Apple live stream worker panicked");
            event_sink.on_event(&EngineEvent::NoSpeech {
                reason: "apple_live_stream_worker_panic".into(),
            });
        }
    }

    let receipt = tail_patch_receipt_after_stop(
        tail_patch_on,
        tail_patch_submitted,
        tail_patch_worker_accounting,
        conservation.clone(),
    );
    log_tail_patch_session_receipt(&receipt);
    report_tail_patch_drain_degrade(
        event_sink.as_ref(),
        receipt.timed_out.saturating_add(receipt.abandoned),
    );
    event_sink.on_event(&receipt.as_event());
    emit_capture_level_receipt(
        event_sink.as_ref(),
        &capture_level.finalize(CapturePathMeta::resolve(sample_rate, 1, None)),
    );
    info!(
        lexicon_rewrites,
        lexicon_entries_bundled = super::live_lexicon::bundled_count(),
        lexicon_entries_custom,
        "Live lexicon take finalised"
    );
    emit_session_finalised(
        event_sink.as_ref(),
        session_id,
        accepted_tail_patch_replacements,
        conservation,
    );
}

/// Mutable seal state for one Apple stream: revision counters plus the shared
/// postprocessor that corrects every final at seal time.
///
/// Grouped into one struct so `emit_stream_events` keeps a readable signature
/// while the worker and the event mapper stay on the same postprocessor
/// instance (one lexicon reload cadence, one drop counter).
/// Cached speech anchors for one `(capture_epoch, observed_samples, floor_bits)`
/// snapshot on `AppleSealState`.
struct VoicedOnsetAnchorCache {
    capture_epoch: u64,
    observed_samples: u64,
    floor_bits: u32,
    anchors: Arc<[SpeechAnchor]>,
}

struct PendingAppleSeal {
    /// Exact physical occurrence selected before any observer was launched.
    occurrence: OccurrenceIdentity,
    raw_text: String,
    /// Byte-identical baseline handed to the tail patcher. Patch char offsets
    /// are valid only against this string, never against raw Apple text.
    layer1_baseline: String,
    start_ts: f32,
    end_ts: f32,
    segments: Vec<TranscriptSegment>,
}

struct AppleSealState {
    capture_turn: CaptureTurnIntent,
    session_id: String,
    capture_epoch: u64,
    sample_rate: u32,
    /// Immutable, independent per-capture policy; neither changes PCM ownership.
    seal_ledger_enabled: bool,
    apple_live_enabled: bool,
    /// Taking this owner closes delivery exactly once and fences late results.
    direct_end: Option<DirectTranscriptEndGuard>,
    /// Selected once from the actually armed, consent-owned recognizer lanes.
    direct_producer: LedgerObservationProducer,
    direct_sample_end: u64,
    direct_apple_epoch_has_final: bool,
    /// Direct publications suppressed by owned Silero non-speech evidence
    /// over a valid capture range. A diagnostic count, never a receipt.
    direct_suppressed_no_measured_speech: u64,
    /// Direct-lane Whisper words dropped because Silero heard no speech within
    /// the near radius of their span. A diagnostic count; each drop is logged.
    direct_words_over_silence: u64,
    /// Direct-lane words dropped as unpinned copies of the word before them
    /// (a decoder loop). A diagnostic count; each window's drop is logged.
    direct_unpinned_repeats: u64,
    /// Valid direct finals withheld on purpose: Silero measured the whole
    /// range quiet, or heard no speech near any word. Completed work, so it
    /// never withholds the direct completion that Stop waits for.
    direct_jobs_suppressed: u64,
    preview_rev: u64,
    speech_progress: SpeechProgress,
    last_integrity: Option<SpeechIntegrity>,
    utterance_id: u64,
    open_partial: String,
    open_partial_segments: Vec<TranscriptSegment>,
    open_partial_pin: u64,
    /// Next open phrase; closing it advances this independently of PCM clocks.
    phrase_id: u64,
    /// A forced close can still receive its recognizer final during finish.
    finishing_phrase_id: Option<u64>,
    last_closed_phrase_id: Option<u64>,
    closed_phrases: BTreeMap<u64, ClosedApplePhrase>,
    published_closed_phrases: BTreeMap<u64, ClosedApplePhrase>,
    refused_untimed_words: BTreeMap<u64, Vec<UnadmittedAppleWord>>,
    unadmitted_revision: u64,
    published_unadmitted_words: Vec<UnadmittedAppleWord>,
    sealed_count: u64,
    filtered_empty_drops: u64,
    lexicon_custom_path: PathBuf,
    lexicon_rewrites: u64,
    lexicon_entries_custom: usize,
    /// Raw capture PCM used to materialize exact planned ranges.
    audio: LiveAudioBuffer,
    terminal_pcm: Option<super::live_audio_buffer::OwnedTerminalPcm>,
    /// Derived ASR values at the same raw capture indices; never energy proof.
    recognizer_audio: Option<LiveAudioBuffer>,
    recognizer_archive: SpeechInputArchive,
    /// End of the last Apple segment admitted to committed canvas. Unlike the
    /// PCM retention cursor, this advances even when Layer 1 audio lookup is
    /// unavailable: Apple segment time is the authority for text disjointness.
    last_apple_segment_end: f32,
    /// Seals whose audio window could not be resolved (F3 falsification).
    unresolved_windows: u64,
    /// Whisper windows accepted onto the provider queue.
    windows_admitted: u64,
    /// Accepted windows built from more than one occurrence.
    windows_coalesced: u64,
    /// Windows refused before a provider ever saw them, by reason code.
    windows_refused_before_inference: BTreeMap<String, u64>,
    /// Seals where Layer 1 recovered speech it could not place on the canvas
    /// (W-C). A non-zero count means the stop path is owed a residual gap fill.
    under_commit_escalations: u64,
    /// Layer 1 hand-off, present only when layered transcription is armed.
    tail_patch: Option<mpsc::Sender<TailPatchRequest>>,
    /// The only authority that offers new inference ranges on capture PCM.
    window_plan: CaptureWindowPlan,
    capture_stopping: bool,
    terminal_archive_pending: bool,
    refinement_pending: VecDeque<TailPatchRequest>,
    /// Original returns retained until closed physical owners can consume them.
    /// No waveform is retained here and replay cannot create a fresh witness.
    retained_word_decodes: VecDeque<TailProviderPayload>,
    /// Returned Whisper windows with their own VAD verdict: `true` when that
    /// VAD selected speech. Only measured verdicts are kept.
    tail_vad_windows: Vec<(u64, u64, bool)>,
    replayed_window_owners: BTreeSet<(u64, OccurrenceIdentity)>,
    refinement_submitted: BTreeMap<(u64, u64, u64, u64), TailPatchInFlight>,
    /// Edge words retain the authenticated decode request until admission or
    /// an accounted same-PCM replacement. Terminal closure supplies no scope.
    pending_whisper_stubs: Vec<(TailRequestIdentity, OccurrenceIdentity, RoutedPin)>,
    /// Incremented only after queue acceptance; never reused within this session.
    last_submission_sequence: u64,
    /// Latest completed window start or closed capture extent. Earlier jobs
    /// must drain before an occurrence behind this horizon can seal.
    admission_horizon: u64,
    /// Utterance-grain straddles still refuse whole-span replacement.
    whisper_span_refused: BTreeSet<OccurrenceIdentity>,
    refinement_clock: Instant,
    terminal_window_tail_replayed: bool,
    refinement_lane_lost: bool,
    /// A matched provider completion returned at least once this session, so
    /// the local engine is demonstrably past cold start. Gates the choice
    /// between the warm and cold-start closure budgets at stop.
    first_whisper_window_completed: bool,
    refinement_started: Instant,
    /// Occurrence formatter hand-off. Presence means the frozen snapshot
    /// permits jobs; it is not itself a scheduled ledger return.
    formatter: Option<mpsc::Sender<FormatterRequest>>,
    /// Sealed occurrences already handed to the formatter, so one committed
    /// label is formatted at most once. Never consulted by sealing or Stop.
    formatter_requested: BTreeSet<OccurrenceIdentity>,
    /// Concatenation of already progressive-sealed text — left context for
    /// Light+ casing on the next seal (w2-b).
    sealed_prefix: String,
    /// Event payload retained until the occurrence's scheduled observers have
    /// returned. AcousticLedger remains the only seal authority.
    pending_events: BTreeMap<u64, PendingAppleSeal>,
    /// Bounded patch events that actually rewrote a pending span this session.
    tail_patch_replacements: u64,
    /// Completed provider jobs whose mutation crossed the rewrite fence.
    tail_patch_jobs_applied: u64,
    /// Completed provider jobs that produced no accepted mutation (no-change,
    /// provider skip, identity/range refusal, or sealed-fence refusal).
    tail_patch_jobs_skipped: u64,
    /// Identity, replay, sealed-fence, or invalid-range refusals.
    tail_patch_refusals: u64,
    /// The session's single Silero: Supervisor VAD + utterance ledger. `None`
    /// only when neither consumer wants it, or when the model failed to load.
    fusion: Option<SileroIngress>,
    /// Whether Silero identity may reach the seal (`CODESCRIBE_SILERO_FUSION`,
    /// default ON). Independent of [`Self::fusion`] existing: the engine
    /// lifecycle needs the VAD even when an operator has pinned the seal path
    /// back to Apple's own segment boundaries.
    fusion_seal_armed: bool,
    /// Word-admission knobs frozen from the take's settings snapshot (the Lab
    /// ledger slider). [`EngineTuning::DEFAULT`] until the worker binds it.
    engine_tuning: EngineTuning,
    /// Summary admission window opened only by the armed EOF early seal.
    /// Hands-free epoch close never opens it; finish and residue close consume it.
    stop_trailing_finish: bool,
    pending_silero_words: BTreeMap<u64, Vec<FusionWord>>,
    unmatched_silero_words: Vec<FusionWord>,
    /// Session PCM identity, never text equality: repeated labels may be speech.
    warned_unmatched_words: BTreeSet<(u64, u64)>,
    no_time_overlap_warnings: u64,
    /// Silero only appends utterances and extends/closes its last range.
    /// Keep the last slice input so unchanged ticks do not clone/scan words.
    silero_slice_revision: Option<(usize, Option<super::silero_fusion::SileroUtterance>)>,
    /// Exact callback input at the slice fence, independent of rewritten labels.
    silero_slice_words: Vec<TranscriptSegment>,
    reconciled_silero: BTreeSet<u64>,
    /// Shared one-throne ledger.
    acoustic_ledger: Arc<Mutex<AcousticLedger>>,
    /// Measured threshold frozen into the same settings snapshot. Absence is a
    /// fail-closed W2 state: no occurrence qualifies and no text mutates.
    energy_calibration: Option<EnergyCalibration>,
    /// Linear RMS of the calibrated existence floor for this take.
    /// `None` means this take has no measured floor. Word admission does not
    /// invent one from `min_energy_integral` or from the −80 dBFS hop floor.
    existence_floor_rms: Option<f32>,
    /// This take's capture energy ladder, bound to session and capture epoch.
    /// The live writer is the async capture arm; this is its reader handle.
    capture_energy: CaptureEnergyOwner,
    /// Cached speech anchors keyed by `(capture_epoch, observed_samples, floor_bits)`
    /// so voiced-onset clamping does not rescan the hop clock per Silero range
    /// or per Whisper word.
    voiced_onset_anchor_cache: Mutex<Option<VoicedOnsetAnchorCache>>,
    /// Present only when this take opened a live cloud session. Local power
    /// leaves it empty, so no commit is sent and no cloud observer is scheduled.
    cloud_commit_tx: Option<mpsc::Sender<u64>>,
    /// Every scheduled CloudLive occurrence not yet assigned to a commit.
    cloud_uncommitted: BTreeSet<OccurrenceIdentity>,
    cloud_last_commit_sample: u64,
    cloud_uncommitted_span_ms: u64,
    /// Commit samples the async lane has not accepted yet.
    cloud_commit_retry: VecDeque<u64>,
    /// Commits whose finals have not returned. The sample is the Silero close.
    cloud_inflight: VecDeque<PendingCloudCommit>,
    cloud_clock_unreliable_logged: bool,
    cloud_live_admitted: u64,
    cloud_live_replaced_apple: u64,
    cloud_live_refused_sealed: u64,
    cloud_live_unowned_routed: u64,
    cloud_live_lane_lost: u64,
    cloud_live_timed_out: u64,
    cloud_live_unmatched_final: u64,
    /// Witness arming. Construction reads the developer env gate; the worker
    /// then applies the immutable settings snapshot verdict (Voice Lab key
    /// with the same env as its power-user override). Nothing re-reads either
    /// source mid-take.
    dictation_witness_armed: bool,
    /// Sender half of the per-utterance witness return lane; the worker owns
    /// the receiver. Witness threads never touch the ledger directly.
    dictation_witness_return: Option<std_mpsc::Sender<DictationWitnessReturn>>,
    /// One witness request in flight per session; the next seal re-checks
    /// whatever debt is still open then.
    dictation_witness_inflight: bool,
    /// Exact owners held by the concrete in-flight witness job.
    dictation_witness_owners: Vec<OccurrenceIdentity>,
    /// Whisper closures awaiting the next bounded independent question.
    dictation_witness_waiting: Vec<OccurrenceIdentity>,
    /// Latest measured witness segments; untimed callbacks may borrow only
    /// this capture-clock evidence, never guessed word durations.
    dictation_witness_payload: Option<TailProviderPayload>,
    /// Untimed finals await measured timing on their own phrase/PCM scope.
    pending_untimed_finals: Vec<(u64, String, u64, u64)>,
    /// Owner ranges already asked live and their attempt counts. A live owner
    /// is asked once per session; persistent debt stays honestly unresolved
    /// instead of charging the decoder at every seal.
    dictation_witness_attempts: Vec<((u64, u64), u8)>,
    /// Owners a live witness answer already testified about. The terminal
    /// pass does not decode the same PCM for them a second time at Stop.
    dictation_witness_heard: Vec<OccurrenceIdentity>,
    /// Set once the terminal witness pass starts. Live questions stay open
    /// through Stop, so the Stop-pass Whisper return cannot seal an owner
    /// before DT answered; after this point a question has no one to wait
    /// for its answer.
    dictation_witness_live_closed: bool,
    /// Live witness requests mint ids in their own band above the terminal
    /// ordinals, so a return can never pose as a terminal-phase answer.
    dictation_witness_requests: u64,
    /// Locale handed to the witness decode, captured at session start.
    dictation_witness_language: Option<String>,
}

/// Capture-clock allowance for a final; the scheduled frontier is the hold.
const CLOUD_FINAL_GRACE: Duration = Duration::from_secs(3);

/// One websocket commit waiting for the final the server stamps back.
struct PendingCloudCommit {
    /// Exclusive Silero close the worker asked the lane to commit.
    sample_end: u64,
    /// Qualified, unsealed occurrences this commit covers.
    occurrences: Vec<OccurrenceIdentity>,
}

/// Worker-bound cloud notices. The end receipt follows all stop finals.
enum CloudWorkerNotice {
    /// Boxed: a stamped final is far larger than `LaneLost` (clippy::large_enum_variant).
    Final(Box<crate::asr_session::events::TranscriptEvent>),
    LaneLost,
    EndSettled,
}

/// The worker's two ends of the CLOUD live lane. They exist only together.
struct CloudWorkerChannels {
    commit: mpsc::Sender<u64>,
    notice: std_mpsc::Receiver<CloudWorkerNotice>,
}

fn inflight_key(submission_sequence: u64, identity: &TailRequestIdentity) -> (u64, u64, u64, u64) {
    (
        submission_sequence,
        identity.request_id,
        identity.range.sample_start,
        identity.range.sample_end,
    )
}

/// What a pin that does not join an exclusive tail becomes.
enum SidePin {
    Replay,
    Unanchored(NoAuthorityReason),
    /// Measured range, no voiced hop. Refusal, not paint.
    NoVoicedHop,
    /// No qualified occurrence holds this PCM, and the calibrated existence
    /// predicate that qualification applies measured it below the shortest
    /// region. Refusal, not paint.
    BelowExistence,
}

/// One original timed pin routed to a single member. Only ledger slot
/// projection clips it to that owner; routing must conserve its physical PCM.
#[derive(Clone)]
struct RoutedPin {
    index: usize,
    pin: OccurrenceIdentity,
    text: String,
    /// Per-word acoustic confidence carried from the routed segment (A6).
    confidence: Option<crate::pipeline::word_confidence::WordConfidence>,
}

/// Exclusive pins for one member, plus whether any other pin blocks the span.
#[derive(Clone, Default)]
struct MemberPinRoute {
    exclusive: Vec<RoutedPin>,
    blocked: bool,
}

/// One member's routed words, with the original geometry of the pins the same
/// batch gave its neighbours: those may cover a group's speech, never its text.
#[derive(Clone, Copy)]
struct RoutedWords<'a> {
    pins: &'a [RoutedPin],
    neighbours: &'a [(OccurrenceIdentity, OccurrenceIdentity)],
}

/// Decode identity and optional owned publication evidence for one batch.
struct RoutedDecode<'a> {
    request_window: (u64, Option<(u64, u64)>),
    publication: Option<(&'a TailProviderPayload, (u64, u64), bool)>,
}

/// `(owner, unclipped pin)` for every pin this batch routed to another member.
fn neighbour_pin_assignments(
    owners: &[(u64, OccurrenceIdentity)],
    routes: &[MemberPinRoute],
    segments: &[TimedTailSegment],
    current: usize,
) -> Vec<(OccurrenceIdentity, OccurrenceIdentity)> {
    owners
        .iter()
        .zip(routes)
        .enumerate()
        .filter(|(index, _)| *index != current)
        .flat_map(|(_, ((_, owner), route))| {
            route.exclusive.iter().filter_map(move |pin| {
                segments
                    .get(pin.index)
                    .map(|segment| (owner.clone(), OccurrenceIdentity::from(&segment.range)))
            })
        })
        .collect()
}

fn exclusive_label(pins: &[RoutedPin]) -> String {
    let mut ordered = pins.to_vec();
    ordered.sort_by_key(|pin| pin.pin.sample_start);
    ordered
        .iter()
        .map(|pin| pin.text.as_str())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn pin_intersects(pin: &OccurrenceIdentity, member: &OccurrenceIdentity) -> bool {
    pin.sample_end > member.sample_start && pin.sample_start < member.sample_end
}

/// Admission window one overlap-pin routing pass runs against: the request
/// that produced the segments and the PCM bounds the admit filter used.
/// Grouping these was the separate cut promised when CL-W2 added the
/// producer argument.
struct AdmitWindow {
    request_id: u64,
    sample_start: u64,
    sample_end: u64,
}

/// How one completed Whisper job of the ledger-free direct lane ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectJobOutcome {
    /// The final reached the presentation.
    Published,
    /// A valid final the take's own Silero evidence withheld on purpose.
    Suppressed,
    /// No valid final, or a publication refused for identity, range or a
    /// closed sink.
    Failed,
}

impl AppleSealState {
    /// Only the session's Silero classifies speech. Both capture observers
    /// must authenticate a complete, valid measurement before silence can
    /// suppress a label. Energy above the ladder floor can be room noise;
    /// it cannot overrule a measured absence of speech. Missing, partial,
    /// foreign or invalid evidence always preserves the possible words.
    fn direct_range_measured_quiet(&self, range: &OccurrenceIdentity) -> bool {
        let fully_observed = |evidence: &AcousticSpeechEvidence| {
            evidence
                .identity()
                .matches(&range.session, range.capture_epoch)
                && matches!(evidence.availability(),
                    AcousticAvailability::Observed { observed_samples }
                        if observed_samples >= range.sample_end)
        };
        let Some(fusion) = &self.fusion else {
            return false;
        };
        let speech = fusion.acoustic_speech_evidence();
        let capture = self.capture_energy.session_active_speech_ranges(
            &self.session_id,
            self.capture_epoch,
            self.sample_rate,
        );
        fully_observed(&speech)
            && fully_observed(&capture)
            && !speech.ranges().iter().any(|speech| {
                speech.sample_end > range.sample_start && speech.sample_start < range.sample_end
            })
    }

    /// Publish raw recognizer bytes without claiming word or seal authority.
    fn publish_direct_transcript(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        observation: LedgerObservationIdentity,
        text: String,
        final_chunk: bool,
    ) -> bool {
        let range = &observation.occurrence;
        if self.seal_ledger_enabled
            || self.direct_end.is_none()
            || observation.producer != self.direct_producer
            || range.session != self.session_id
            || range.capture_epoch != self.capture_epoch
            || range.sample_start < self.direct_sample_end
            || range.sample_end <= range.sample_start
            || range.sample_end > self.audio.session_sample_end()
        {
            return false;
        }
        // The direct lane has no ledger admission to stop a decoder's
        // near-silence hallucination; the producer-side protection is the
        // take's own measured evidence. A fully measured quiet range is the
        // only suppressible input — anything unmeasured keeps its words.
        if self.direct_range_measured_quiet(range) {
            self.direct_suppressed_no_measured_speech += 1;
            debug!(
                session = %self.session_id,
                sample_start = range.sample_start,
                sample_end = range.sample_end,
                suppressed = self.direct_suppressed_no_measured_speech,
                "direct transcript suppressed: owned Silero measured no speech in the valid capture range"
            );
            return false;
        }
        if final_chunk {
            self.direct_sample_end = range.sample_end;
        }
        ev_tx
            .send(EngineEvent::DirectTranscript {
                observation,
                text,
                final_chunk,
            })
            .is_ok()
    }

    /// Count one completed direct Whisper job. Only a failed job withholds
    /// the direct completion; a deliberate suppression owes no words.
    fn account_direct_job(&mut self, outcome: DirectJobOutcome) {
        match outcome {
            DirectJobOutcome::Published => {
                self.tail_patch_jobs_applied = self.tail_patch_jobs_applied.saturating_add(1);
            }
            DirectJobOutcome::Suppressed => {
                self.direct_jobs_suppressed = self.direct_jobs_suppressed.saturating_add(1);
            }
            DirectJobOutcome::Failed => {
                self.tail_patch_jobs_skipped = self.tail_patch_jobs_skipped.saturating_add(1);
            }
        }
    }

    /// Replace the presentation mirror from the worker's actual held words.
    /// Call only after the ledger events for the same transition were sent.
    fn publish_unadmitted_words(&mut self, ev_tx: &mpsc::UnboundedSender<EngineEvent>) {
        if !self.seal_ledger_enabled || !self.apple_live_enabled {
            return;
        }
        let committed = {
            let ledger = self
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ledger
                .occurrences()
                .filter(|occurrence| {
                    occurrence.session == self.session_id
                        && occurrence.capture_epoch == self.capture_epoch
                })
                .map(|occurrence| (occurrence.sample_start, occurrence.sample_end))
                .collect::<Vec<_>>()
        };
        let mut words = Vec::new();
        // The pin is diagnostic only, including when Apple supplied segments.
        // Repeated lexical words in one phrase must retain their multiplicity.
        // Untimed words remain explicitly uncertain. Neither arrival order
        // nor their index in a revised partial proves PCM coverage.
        words.extend(
            self.open_partial
                .split_whitespace()
                .map(|text| UnadmittedAppleWord {
                    text: text.to_string(),
                    sample_start: self.open_partial_pin,
                    sample_end: self.open_partial_pin,
                    source: UnadmittedAppleWordSource::OpenPartial {
                        rev: self.preview_rev,
                        phrase_id: self.phrase_id,
                    },
                }),
        );
        let mut current_ranged: Vec<(FusionWord, UnadmittedAppleWordSource)> = Vec::new();
        let ranged = self
            .unmatched_silero_words
            .iter()
            .cloned()
            .map(|word| (word, UnadmittedAppleWordSource::Unmatched))
            .chain(
                self.pending_silero_words
                    .iter()
                    .flat_map(|(&utterance_id, pending)| {
                        pending.iter().cloned().map(move |word| {
                            (word, UnadmittedAppleWordSource::Pending { utterance_id })
                        })
                    }),
            );
        for candidate in ranged {
            if let Some(index) = current_ranged.iter().position(|(held, _)| {
                held.sample_start == candidate.0.sample_start
                    && held.sample_end == candidate.0.sample_end
            }) {
                current_ranged[index] = candidate;
            } else {
                current_ranged.push(candidate);
            }
        }
        words.extend(
            current_ranged
                .into_iter()
                .map(|(word, source)| UnadmittedAppleWord {
                    text: word.text,
                    sample_start: word.sample_start,
                    sample_end: word.sample_end,
                    source,
                }),
        );
        words.extend(self.refused_untimed_words.values().flatten().cloned());
        words.retain(|word| {
            if matches!(
                word.source,
                UnadmittedAppleWordSource::OpenPartial { .. }
                    | UnadmittedAppleWordSource::RefusedUntimed { .. }
            ) {
                return true;
            }
            let midpoint =
                word.sample_start + word.sample_end.saturating_sub(word.sample_start) / 2;
            !committed
                .iter()
                .any(|&(start, end)| start <= midpoint && midpoint < end)
        });
        words.sort_by_key(|word| match word.source {
            UnadmittedAppleWordSource::OpenPartial { .. } => (2, 0, 0),
            UnadmittedAppleWordSource::RefusedUntimed { phrase_id } => (1, phrase_id, 0),
            _ => (0, word.sample_start, word.sample_end),
        });
        if words == self.published_unadmitted_words
            && self.closed_phrases == self.published_closed_phrases
        {
            return;
        }
        self.unadmitted_revision = self.unadmitted_revision.saturating_add(1);
        self.published_unadmitted_words = words.clone();
        self.published_closed_phrases
            .clone_from(&self.closed_phrases);
        let _ = ev_tx.send(EngineEvent::UnadmittedAppleWords {
            revision: self.unadmitted_revision,
            words,
            closed_phrases: self.closed_phrases.clone(),
        });
    }

    /// Current partial or accepted capture label; a refused raw callback alone
    /// must not release an empty stop before finish can supply usable text.
    fn has_stop_canvas_text(&self) -> bool {
        if !self.open_partial.trim().is_empty() {
            return true;
        }
        let ledger = self
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ledger.qualified_occurrences().any(|occurrence| {
            occurrence.session == self.session_id
                && occurrence.capture_epoch == self.capture_epoch
                && ledger
                    .text_of(occurrence)
                    .is_some_and(|text| !text.trim().is_empty())
        })
    }

    fn emit_speech_integrity(&mut self, ev_tx: &mpsc::UnboundedSender<EngineEvent>) {
        // Without admission there is no ledger text frontier to compare with.
        // Capture energy and Silero sideband measurements remain independent.
        if !self.seal_ledger_enabled {
            return;
        }
        let ledger = self
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let debts = ledger.pending_text_recoveries(&self.session_id, self.capture_epoch);
        let recovering = debts.iter().any(|occurrence| {
            ledger.frontier_of(occurrence).is_some_and(|frontier| {
                frontier
                    .open_producers()
                    .contains(&LedgerObservationProducer::Whisper)
            })
        });
        drop(ledger);
        let phase = if recovering {
            SpeechIntegrityPhase::Recovering
        } else if !debts.is_empty() {
            SpeechIntegrityPhase::Unresolved
        } else {
            self.speech_progress.phase(self.sample_rate)
        };
        let debt_ms = self.speech_progress.debt_ms(self.sample_rate);
        let speech_live = self.speech_progress.speech_live();
        if self.last_integrity.as_ref().is_some_and(|previous| {
            previous.phase == phase
                && previous.pending_occurrences == debts.len() as u64
                && previous.acoustic_speech_ms_since_text_advance / 100 == debt_ms / 100
                && previous.speech_live == speech_live
        }) {
            return;
        }
        let evidence = SpeechIntegrity {
            session_id: self.session_id.clone(),
            capture_epoch: self.capture_epoch,
            sequence: self
                .last_integrity
                .as_ref()
                .map_or(1, |previous| previous.sequence + 1),
            acoustic_speech_ms_since_text_advance: debt_ms,
            pending_occurrences: debts.len() as u64,
            phase,
            speech_live,
        };
        self.last_integrity = Some(evidence.clone());
        let _ = ev_tx.send(EngineEvent::SpeechIntegrity { evidence });
    }

    fn observe_apple_progress(&mut self, text: &str, segments: &[TranscriptSegment]) {
        let word_end = segments
            .iter()
            .map(|segment| (segment.end_ts.max(0.0) * self.sample_rate as f32) as u64)
            .max()
            .unwrap_or(0);
        self.speech_progress.observe_apple(text, word_end);
    }

    fn window_by_samples(&self, start: u64, end: u64) -> Option<ResolvedAudioWindow> {
        if let Some(archive) = &self.terminal_pcm {
            archive.window(start, end)
        } else {
            self.audio
                .window_by_samples(start, end)
                .filter(|window| window.sample_start == start && window.sample_end == end)
        }
    }

    /// Decoder-only view. Acoustic qualification and fence measurements keep
    /// using window_by_samples, whose raw authority is unchanged.
    fn recognizer_window_by_samples(&self, start: u64, end: u64) -> Option<ResolvedAudioWindow> {
        let Some(audio) = self.recognizer_audio.as_ref() else {
            return self.window_by_samples(start, end);
        };
        if let Some(window) = self.recognizer_archive.window(start, end) {
            return match window {
                Ok(window) => Some(window),
                Err(error) => {
                    warn!(%error, start, end, "recognizer projection range unavailable");
                    None
                }
            };
        }
        audio
            .window_by_samples(start, end)
            .filter(|window| window.sample_start == start && window.sample_end == end)
    }

    /// Resolve one full alignment-uncertainty interval inside the issued edge.
    /// Keep no waveform in the ledger or on individual Words.
    fn record_decode_fence_measurement(
        &self,
        ledger: &mut AcousticLedger,
        decode_window: Option<(u64, u64)>,
    ) {
        let resolution = u64::from(self.sample_rate).div_ceil(50).saturating_add(1);
        let read_edge = |start, end| {
            self.window_by_samples(start, end)
                .filter(|window| window.sample_start == start && window.sample_end == end)
        };
        let edges = decode_window
            .map(|(start, end)| {
                [
                    start
                        .checked_add(resolution)
                        .filter(|edge_end| *edge_end <= end)
                        .and_then(|edge_end| read_edge(start, edge_end)),
                    end.checked_sub(resolution)
                        .filter(|edge_start| *edge_start >= start)
                        .and_then(|edge_start| read_edge(edge_start, end)),
                ]
            })
            .unwrap_or([None, None]);
        let ranges = edges.each_ref().map(|edge| {
            edge.as_ref().map(|window| {
                OccurrenceIdentity::new(
                    &self.session_id,
                    self.capture_epoch,
                    window.sample_start,
                    window.sample_end,
                )
            })
        });
        ledger.record_decode_fence_energy(
            &self.capture_energy,
            self.audio.session_sample_end(),
            std::array::from_fn(|index| {
                ranges[index]
                    .as_ref()
                    .zip(edges[index].as_ref())
                    .map(|(range, window)| (range, window.samples.as_slice()))
            }),
        );
        if self.capture_stopping {
            ledger.close_speech_anchor_capture(
                &self.session_id,
                self.capture_epoch,
                self.audio.session_sample_end(),
            );
        }
    }

    /// Bind the original Apple pins to this capture's measured PCM before
    /// admission. Missing or partial capture evidence creates no anchors.
    fn bind_apple_word_measurement(&self, ledger: &mut AcousticLedger, owner: &OccurrenceIdentity) {
        if owner.session != self.session_id
            || owner.capture_epoch != self.capture_epoch
            || owner.sample_end > self.audio.session_sample_end()
            || !self
                .capture_energy
                .identity()
                .matches(&owner.session, owner.capture_epoch)
        {
            return;
        }
        let capture = self.capture_energy.session_active_speech_ranges(
            &self.session_id,
            self.capture_epoch,
            self.sample_rate,
        );
        if capture
            .availability()
            .observed_samples()
            .is_none_or(|end| end < owner.sample_end)
        {
            return;
        }
        let speech = coverage_speech_evidence(self);
        if speech
            .availability()
            .observed_samples()
            .is_some_and(|end| end >= owner.sample_end)
        {
            ledger.record_speech_evidence(&speech);
        } else {
            ledger.record_speech_evidence(&capture);
        }
        self.record_decode_fence_measurement(ledger, None);
    }

    fn new_for_session(sample_rate: u32, session_id: String, capture_epoch: u64) -> Self {
        let session_id_for_energy = session_id.clone();
        let speech_progress = SpeechProgress::new(session_id.clone(), capture_epoch, sample_rate);
        let state = Self {
            capture_turn: CaptureTurnIntent::HandsFree,
            session_id,
            capture_epoch,
            sample_rate,
            seal_ledger_enabled: true,
            apple_live_enabled: true,
            direct_end: None,
            direct_producer: LedgerObservationProducer::Apple,
            direct_sample_end: 0,
            direct_apple_epoch_has_final: false,
            direct_suppressed_no_measured_speech: 0,
            direct_words_over_silence: 0,
            direct_unpinned_repeats: 0,
            direct_jobs_suppressed: 0,
            preview_rev: 0,
            speech_progress,
            last_integrity: None,
            utterance_id: 0,
            open_partial: String::new(),
            open_partial_segments: Vec::new(),
            open_partial_pin: 0,
            phrase_id: 1,
            finishing_phrase_id: None,
            last_closed_phrase_id: None,
            closed_phrases: BTreeMap::new(),
            published_closed_phrases: BTreeMap::new(),
            refused_untimed_words: BTreeMap::new(),
            unadmitted_revision: 0,
            published_unadmitted_words: Vec::new(),
            sealed_count: 0,
            filtered_empty_drops: 0,
            lexicon_custom_path: crate::config::Config::config_dir().join("lexicon.custom.jsonl"),
            lexicon_rewrites: 0,
            lexicon_entries_custom: 0,
            audio: LiveAudioBuffer::new(sample_rate, DEFAULT_RETENTION_SECS),
            terminal_pcm: None,
            recognizer_audio: None,
            recognizer_archive: SpeechInputArchive::default(),
            last_apple_segment_end: 0.0,
            unresolved_windows: 0,
            windows_admitted: 0,
            windows_coalesced: 0,
            windows_refused_before_inference: BTreeMap::new(),
            under_commit_escalations: 0,
            tail_patch: None,
            window_plan: CaptureWindowPlan::new(
                session_id_for_energy.clone(),
                capture_epoch,
                sample_rate,
            ),
            capture_stopping: false,
            terminal_archive_pending: false,
            refinement_pending: VecDeque::new(),
            retained_word_decodes: VecDeque::new(),
            tail_vad_windows: Vec::new(),
            replayed_window_owners: BTreeSet::new(),
            refinement_submitted: BTreeMap::new(),
            pending_whisper_stubs: Vec::new(),
            last_submission_sequence: 0,
            admission_horizon: 0,
            whisper_span_refused: BTreeSet::new(),
            refinement_clock: Instant::now(),
            terminal_window_tail_replayed: false,
            refinement_lane_lost: false,
            first_whisper_window_completed: false,
            refinement_started: Instant::now(),
            formatter: None,
            formatter_requested: BTreeSet::new(),
            sealed_prefix: String::new(),
            pending_events: BTreeMap::new(),
            tail_patch_replacements: 0,
            tail_patch_jobs_applied: 0,
            tail_patch_jobs_skipped: 0,
            tail_patch_refusals: 0,
            fusion: None,
            fusion_seal_armed: false,
            engine_tuning: EngineTuning::DEFAULT,
            stop_trailing_finish: false,
            // One source of truth for the default cut. Without a Silero
            // ingress nothing reads this field; when one arms, `from_env`
            // resolves the same default unless an operator overrode it.
            pending_silero_words: BTreeMap::new(),
            unmatched_silero_words: Vec::new(),
            warned_unmatched_words: BTreeSet::new(),
            no_time_overlap_warnings: 0,
            silero_slice_revision: None,
            silero_slice_words: Vec::new(),
            reconciled_silero: BTreeSet::new(),
            acoustic_ledger: Arc::new(Mutex::new(AcousticLedger::new())),
            energy_calibration: None,
            existence_floor_rms: None,
            capture_energy: CaptureEnergyOwner::bind(session_id_for_energy, capture_epoch),
            voiced_onset_anchor_cache: Mutex::new(None),
            cloud_commit_tx: None,
            cloud_uncommitted: BTreeSet::new(),
            cloud_last_commit_sample: 0,
            cloud_uncommitted_span_ms: 0,
            cloud_commit_retry: VecDeque::new(),
            cloud_inflight: VecDeque::new(),
            cloud_clock_unreliable_logged: false,
            cloud_live_admitted: 0,
            cloud_live_replaced_apple: 0,
            cloud_live_refused_sealed: 0,
            cloud_live_unowned_routed: 0,
            cloud_live_lane_lost: 0,
            cloud_live_timed_out: 0,
            cloud_live_unmatched_final: 0,
            dictation_witness_armed: crate::stt::apple_stt::dictation_transcriber_enabled(),
            dictation_witness_return: None,
            dictation_witness_inflight: false,
            dictation_witness_owners: Vec::new(),
            dictation_witness_waiting: Vec::new(),
            dictation_witness_payload: None,
            pending_untimed_finals: Vec::new(),
            dictation_witness_attempts: Vec::new(),
            dictation_witness_heard: Vec::new(),
            dictation_witness_live_closed: false,
            dictation_witness_requests: 0,
            dictation_witness_language: None,
        };
        state
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .bind_capture_rate(sample_rate);
        state
    }

    /// Adopt the capture arm's energy-ladder owner.
    ///
    /// The worker is the reader; the writer lives on the async capture arm.
    /// Installing its handle here is what makes the two threads share one
    /// measurement instead of two ladders that agree by coincidence.
    fn bind_capture_energy(&mut self, owner: CaptureEnergyOwner) {
        debug_assert!(
            owner
                .identity()
                .matches(&self.session_id, self.capture_epoch),
            "the capture owner must name this take"
        );
        *self
            .voiced_onset_anchor_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        self.capture_energy = owner;
    }

    fn voiced_onset_anchors(&self) -> Option<Arc<[SpeechAnchor]>> {
        let floor = self
            .existence_floor_rms
            .filter(|floor| floor.is_finite() && *floor > 0.0)?;
        let observed_samples = self.capture_energy.observed_samples();
        let floor_bits = floor.to_bits();
        if let Some(cached) = self
            .voiced_onset_anchor_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            && cached.capture_epoch == self.capture_epoch
            && cached.observed_samples == observed_samples
            && cached.floor_bits == floor_bits
        {
            return Some(Arc::clone(&cached.anchors));
        }
        let evidence = self.capture_energy.session_speech_anchors(
            &self.session_id,
            self.capture_epoch,
            self.sample_rate,
            floor,
        );
        let AcousticAvailability::Observed { observed_samples } = evidence.availability else {
            return None;
        };
        let anchors: Arc<[SpeechAnchor]> = Arc::from(evidence.anchors);
        *self
            .voiced_onset_anchor_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(VoicedOnsetAnchorCache {
            capture_epoch: self.capture_epoch,
            observed_samples,
            floor_bits,
            anchors: Arc::clone(&anchors),
        });
        Some(anchors)
    }

    fn new_for_session_with_ledger(
        sample_rate: u32,
        session_id: String,
        capture_epoch: u64,
        acoustic_ledger: Arc<Mutex<AcousticLedger>>,
        energy_calibration: Option<EnergyCalibration>,
    ) -> Self {
        acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .bind_capture_rate(sample_rate);
        Self {
            acoustic_ledger,
            energy_calibration,
            ..Self::new_for_session(sample_rate, session_id, capture_epoch)
        }
    }

    /// Schedule at creation, before synchronous observers can close the frontier.
    /// Exact PCM identities survive utterance re-partition and re-mint.
    fn track_cloud_occurrence(&mut self, occurrence: &OccurrenceIdentity) {
        if self.cloud_commit_tx.is_none() {
            return;
        }
        let mut ledger = self
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !ledger.is_qualified(occurrence) || ledger.is_sealed(occurrence) {
            return;
        }
        if ledger.frontier_of(occurrence).is_none() {
            ledger.schedule_frontier(occurrence.clone(), [LedgerObservationProducer::CloudLive]);
        } else {
            ledger.schedule_observer(occurrence.clone(), LedgerObservationProducer::CloudLive);
        }
        if ledger.frontier_of(occurrence).is_some_and(|frontier| {
            frontier
                .open_producers()
                .contains(&LedgerObservationProducer::CloudLive)
        }) && !self
            .cloud_inflight
            .iter()
            .any(|commit| commit.occurrences.contains(occurrence))
        {
            self.cloud_uncommitted.insert(occurrence.clone());
        }
    }

    /// Only the VAD's typed silence decision can cut the live stream.
    /// EndOfCapture is handled by the lane's existing `end` at stop.
    fn commit_cloud_close(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        receipt: super::silero_fusion::UtteranceCloseReceipt,
    ) {
        if self.cloud_commit_tx.is_none()
            || receipt.cause != super::silero_fusion::UtteranceCloseCause::SilenceFence
            || receipt.decision_sample <= self.cloud_last_commit_sample
            || self
                .cloud_commit_retry
                .back()
                .is_some_and(|sample| receipt.decision_sample <= *sample)
        {
            return;
        }
        let sample_end = receipt.decision_sample;
        let ledger = self
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let occurrences = self
            .cloud_uncommitted
            .iter()
            .filter(|owner| {
                // The callback can close A after S; B's pre-pad can start
                // before S. Only already-closed owners belong to this final.
                owner.sample_start < sample_end
                    && ledger
                        .serial_of(owner)
                        .is_some_and(|serial| serial.vad_closed())
            })
            .cloned()
            .collect::<Vec<_>>();
        drop(ledger);
        for occurrence in &occurrences {
            self.cloud_uncommitted.remove(occurrence);
        }
        self.cloud_inflight.push_back(PendingCloudCommit {
            sample_end,
            occurrences,
        });
        self.cloud_commit_retry.push_back(sample_end);
        self.flush_cloud_commits(ev_tx);
    }

    /// Measure the longest span since a commit, including split-only monologues.
    fn observe_cloud_capture_head(&mut self, capture_head: u64) {
        if self.cloud_commit_tx.is_some() {
            let span_ms = capture_head
                .saturating_sub(self.cloud_last_commit_sample)
                .saturating_mul(1_000)
                / u64::from(self.sample_rate.max(1));
            self.cloud_uncommitted_span_ms = self.cloud_uncommitted_span_ms.max(span_ms);
        }
    }

    fn flush_cloud_commits(&mut self, ev_tx: &mpsc::UnboundedSender<EngineEvent>) {
        while let Some(sample_end) = self.cloud_commit_retry.pop_front() {
            let Some(sender) = self.cloud_commit_tx.as_ref() else {
                self.cloud_commit_retry.clear();
                self.return_cloud_live_lost(ev_tx);
                return;
            };
            match sender.try_send(sample_end) {
                Ok(()) => {
                    self.observe_cloud_capture_head(sample_end);
                    self.cloud_last_commit_sample = self.cloud_last_commit_sample.max(sample_end);
                }
                Err(mpsc::error::TrySendError::Full(sample_end)) => {
                    self.cloud_commit_retry.push_front(sample_end);
                    return;
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    self.cloud_commit_tx = None;
                    self.cloud_commit_retry.clear();
                    self.return_cloud_live_lost(ev_tx);
                    return;
                }
            }
        }
    }

    fn handle_cloud_notice(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        notice: CloudWorkerNotice,
    ) {
        match notice {
            CloudWorkerNotice::LaneLost => self.return_cloud_live_lost(ev_tx),
            CloudWorkerNotice::Final(event) => {
                self.admit_cloud_final(ev_tx, *event);
                // A cloud final commits speech the open partial may restate.
                self.publish_unadmitted_words(ev_tx);
            }
            CloudWorkerNotice::EndSettled => {}
        }
    }

    fn return_cloud_live_lost(&mut self, ev_tx: &mpsc::UnboundedSender<EngineEvent>) {
        if !self.seal_ledger_enabled
            && self.direct_producer == LedgerObservationProducer::CloudLive
            && self.direct_end.is_some()
        {
            let _ = ev_tx.send(EngineEvent::NoSpeech {
                reason: "The selected Cloud recognizer closed before capture completed.".into(),
            });
            drop(self.direct_end.take());
        }
        self.cloud_commit_tx = None;
        self.cloud_commit_retry.clear();
        for occurrence in self.take_outstanding_cloud() {
            self.cloud_live_lane_lost = self.cloud_live_lane_lost.saturating_add(1);
            self.cloud_receipt(&occurrence, "cloud_live_lane_lost");
            self.return_cloud_observer(ev_tx, &occurrence);
        }
    }

    /// A later utterance or Whisper horizon cannot expire CloudLive. Only
    /// S + grace on the raw capture clock returns a stalled commit's observers.
    fn release_cloud_live_behind(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        capture_head: u64,
    ) {
        let grace_samples = CLOUD_FINAL_GRACE
            .as_secs()
            .saturating_mul(u64::from(self.sample_rate.max(1)));
        let mut due = Vec::new();
        let mut kept = VecDeque::new();
        while let Some(commit) = self.cloud_inflight.pop_front() {
            if capture_head >= commit.sample_end.saturating_add(grace_samples) {
                due.extend(commit.occurrences);
            } else {
                kept.push_back(commit);
            }
        }
        self.cloud_inflight = kept;
        for occurrence in due {
            self.cloud_live_timed_out = self.cloud_live_timed_out.saturating_add(1);
            self.cloud_receipt(&occurrence, "cloud_live_timed_out");
            self.return_cloud_observer(ev_tx, &occurrence);
        }
    }

    fn take_outstanding_cloud(&mut self) -> BTreeSet<OccurrenceIdentity> {
        let mut occurrences = std::mem::take(&mut self.cloud_uncommitted);
        occurrences.extend(
            self.cloud_inflight
                .drain(..)
                .flat_map(|commit| commit.occurrences),
        );
        occurrences
    }

    fn return_outstanding_cloud(&mut self, ev_tx: &mpsc::UnboundedSender<EngineEvent>) {
        self.cloud_commit_tx = None;
        self.cloud_commit_retry.clear();
        for occurrence in self.take_outstanding_cloud() {
            self.cloud_receipt(&occurrence, "cloud_live_session_end");
            self.return_cloud_observer(ev_tx, &occurrence);
        }
    }

    fn cloud_receipt(&self, occurrence: &OccurrenceIdentity, disposition: &'static str) {
        info!(
            session = %occurrence.session,
            capture_epoch = occurrence.capture_epoch,
            sample_start = occurrence.sample_start,
            sample_end = occurrence.sample_end,
            disposition,
            "cloud_live"
        );
    }

    fn admit_cloud_final(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        event: crate::asr_session::events::TranscriptEvent,
    ) {
        let Some(commit) = event.commit.clone() else {
            return;
        };
        self.log_cloud_final(&commit);
        if !self.seal_ledger_enabled {
            if let Some(index) = self
                .cloud_inflight
                .iter()
                .position(|pending| pending.sample_end <= commit.sample_end)
            {
                self.cloud_inflight.remove(index);
            }
            let observation = LedgerObservationIdentity::new(
                LedgerObservationProducer::CloudLive,
                commit.sample_start,
                0,
                OccurrenceIdentity::new(
                    &self.session_id,
                    self.capture_epoch,
                    commit.sample_start,
                    commit.sample_end,
                ),
            );
            if self.publish_direct_transcript(ev_tx, observation, event.text, true) {
                self.cloud_live_admitted = self.cloud_live_admitted.saturating_add(1);
            }
            return;
        }
        let matched = self
            .cloud_inflight
            .iter()
            .position(|pending| pending.sample_end == commit.sample_end)
            .or_else(|| {
                self.cloud_inflight
                    .iter()
                    .position(|pending| pending.sample_end <= commit.sample_end)
            });
        let pending = matched.and_then(|index| self.cloud_inflight.remove(index));
        if pending.is_none() {
            self.cloud_live_unmatched_final = self.cloud_live_unmatched_final.saturating_add(1);
            info!(
                sample_end = commit.sample_end,
                cloud_live_unmatched_final = 1,
                "cloud_live_admission"
            );
        }
        let scheduled = pending.map(|commit| commit.occurrences).unwrap_or_default();
        let word_grain = commit.grain == crate::asr_session::events::FinalGrain::Word
            && !commit.words.is_empty()
            && !commit.phrase_fallback;
        if word_grain {
            self.admit_cloud_words(ev_tx, &commit);
        } else if !event.text.trim().is_empty() {
            self.admit_cloud_phrase(ev_tx, &commit, event.text.trim());
        }
        for occurrence in scheduled {
            self.return_cloud_observer(ev_tx, &occurrence);
        }
    }

    fn log_cloud_final(&mut self, commit: &crate::asr_session::events::FinalCommit) {
        let mismatch = commit.range_mismatch.as_ref().map(|mismatch| {
            format!(
                "commit={}..{} server={}..{}",
                mismatch.commit_sample_start,
                mismatch.commit_sample_end,
                mismatch.server_sample_start,
                mismatch.server_sample_end
            )
        });
        info!(
            match_path = %commit.match_path.as_label(),
            word_time_clamped = commit.word_time_clamped,
            phrase_fallback = commit.phrase_fallback,
            commit_range_mismatch = mismatch.as_deref().unwrap_or(""),
            "cloud_live_admission"
        );
        if commit.clock_unreliable && !self.cloud_clock_unreliable_logged {
            self.cloud_clock_unreliable_logged = true;
            info!(stream_clock_unreliable = true, "cloud_live_admission");
        }
    }

    fn admit_cloud_words(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        commit: &crate::asr_session::events::FinalCommit,
    ) {
        let segments = commit
            .words
            .iter()
            .map(|word| TimedTailSegment {
                text: word.word.clone(),
                range: TailSampleRange {
                    session: self.session_id.clone(),
                    capture_epoch: self.capture_epoch,
                    sample_start: word.sample_start,
                    sample_end: word.sample_end,
                },
                grain: crate::stt::tail_provider::TailSegmentGrain::Word,
                // Vendor `words[].probability` survives into the pin (A6);
                // absence stays `None`.
                confidence: word.probability.map(|probability| {
                    crate::pipeline::word_confidence::WordConfidence::new(
                        crate::pipeline::word_confidence::WordConfidenceSource::VendorWordProbability,
                        probability,
                        1,
                    )
                }),
            })
            .collect::<Vec<_>>();
        self.admit_cloud_segments(ev_tx, commit, &segments, true);
    }

    fn admit_cloud_phrase(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        commit: &crate::asr_session::events::FinalCommit,
        text: &str,
    ) {
        let segments = vec![TimedTailSegment {
            text: text.to_string(),
            range: TailSampleRange {
                session: self.session_id.clone(),
                capture_epoch: self.capture_epoch,
                sample_start: commit.sample_start,
                sample_end: commit.sample_end,
            },
            grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
            // A whole-phrase cloud commit has no per-word metric (A6).
            confidence: None,
        }];
        self.admit_cloud_segments(ev_tx, commit, &segments, false);
    }

    fn admit_cloud_segments(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        commit: &crate::asr_session::events::FinalCommit,
        segments: &[TimedTailSegment],
        word_grain: bool,
    ) {
        let owners = self.word_owners_in_window(commit.sample_start, commit.sample_end, segments);
        let routes = self.route_overlap_pins(
            ev_tx,
            AdmitWindow {
                request_id: commit.sample_start,
                sample_start: commit.sample_start,
                sample_end: commit.sample_end,
            },
            &owners,
            segments,
            LedgerObservationProducer::CloudLive,
        );
        for (index, ((member_id, occurrence), route)) in owners.iter().zip(&routes).enumerate() {
            if word_grain {
                if route.exclusive.is_empty() {
                    continue;
                }
                let before = self
                    .acoustic_ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .layer_trail()
                    .len();
                let admitted = self.admit_routed_words_with_decode_window(
                    ev_tx,
                    *member_id,
                    occurrence,
                    (
                        commit.sample_start,
                        Some((commit.sample_start, commit.sample_end)),
                    ),
                    RoutedWords {
                        pins: &route.exclusive,
                        neighbours: &neighbour_pin_assignments(&owners, &routes, segments, index),
                    },
                    LedgerObservationProducer::CloudLive,
                );
                if admitted {
                    self.cloud_live_admitted = self.cloud_live_admitted.saturating_add(1);
                }
                let (replaced_apple, refused_sealed) = {
                    let trail = self
                        .acoustic_ledger
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let mut replaced_apple = 0u64;
                    let mut refused_sealed = 0u64;
                    for entry in trail.layer_trail().iter().skip(before) {
                        match &entry.decision {
                            MutationReceipt::Refuse {
                                reason: RefuseReason::ReplacedByCloudLive,
                                ..
                            } if entry.observation.producer == LedgerObservationProducer::Apple => {
                                replaced_apple = replaced_apple.saturating_add(1);
                            }
                            MutationReceipt::KeepVisibleUnanchored {
                                reason: NoAuthorityReason::LateCloudLiveWordSealedOwner,
                                ..
                            } => {
                                refused_sealed = refused_sealed.saturating_add(1);
                            }
                            _ => {}
                        }
                    }
                    (replaced_apple, refused_sealed)
                };
                self.cloud_live_replaced_apple = self
                    .cloud_live_replaced_apple
                    .saturating_add(replaced_apple);
                self.cloud_live_refused_sealed = self
                    .cloud_live_refused_sealed
                    .saturating_add(refused_sealed);
                continue;
            }
            let text = exclusive_label(&route.exclusive);
            if text.is_empty() {
                continue;
            }
            let observation = self
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .next_word_observation(
                    LedgerObservationProducer::CloudLive,
                    commit.sample_start,
                    occurrence,
                );
            if admit_ledger_label(
                self,
                ev_tx,
                LabelAdmission {
                    observation,
                    label: &text,
                    energy: EnergyAdmission::RequireExistingQualification,
                },
            )
            .is_some_and(|receipt| receipt.grants_mutation())
            {
                self.cloud_live_admitted = self.cloud_live_admitted.saturating_add(1);
            }
            self.refresh_pending_label(*member_id, occurrence);
        }
    }

    fn return_cloud_observer(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        occurrence: &OccurrenceIdentity,
    ) {
        self.cloud_uncommitted.remove(occurrence);
        let mut ledger = self
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !ledger.frontier_of(occurrence).is_some_and(|frontier| {
            frontier
                .open_producers()
                .contains(&LedgerObservationProducer::CloudLive)
        }) {
            return;
        }
        let closed = ledger.note_frontier_return(occurrence, LedgerObservationProducer::CloudLive);
        if closed && let Ok(receipt) = ledger.seal(occurrence).cloned() {
            let _ = ev_tx.send(EngineEvent::LedgerSeal { receipt });
        }
        schedule_formatter_for_sealed_label(
            &ledger,
            self.formatter.as_ref(),
            &mut self.formatter_requested,
            occurrence,
        );
    }

    /// Occurrence qualification registers an observer, never an inference range.
    fn register_whisper_owner(
        &mut self,
        _ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        utterance_id: u64,
        occurrence: &OccurrenceIdentity,
    ) -> bool {
        if self.tail_patch.is_none() && !self.refinement_lane_lost {
            return false;
        }
        if self
            .pending_events
            .get(&utterance_id)
            .is_none_or(|pending| &pending.occurrence != occurrence)
        {
            return false;
        }
        let mut ledger = self
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !ledger.is_qualified(occurrence) || ledger.is_sealed(occurrence) {
            return false;
        }
        let scheduled = if ledger.frontier_of(occurrence).is_none() {
            ledger.schedule_frontier(occurrence.clone(), [LedgerObservationProducer::Whisper]);
            true
        } else if ledger.frontier_of(occurrence).is_some_and(|frontier| {
            frontier
                .open_producers()
                .contains(&LedgerObservationProducer::Whisper)
        }) {
            true
        } else {
            ledger.schedule_observer(occurrence.clone(), LedgerObservationProducer::Whisper)
        };
        drop(ledger);
        if scheduled {
            self.refinement_receipt(occurrence, "admitted");
        }
        scheduled
    }

    fn window_plan_receipt(&self, range: &TailSampleRange, disposition: &'static str) {
        info!(
            session_id = %range.session,
            capture_epoch = range.capture_epoch,
            sample_start = range.sample_start,
            sample_end = range.sample_end,
            grid_ordinal = range.sample_start / (u64::from(self.sample_rate).max(1) * 3),
            phase = if self.capture_stopping { "stop" } else { "live" },
            disposition,
            grid_pass_limit = 3,
            bounded_trials_per_nine_seconds = 1,
            "capture_window_plan"
        );
    }

    /// Only a complete Silero measurement can skip an encoder window. An
    /// unavailable or lagging observer is unknown, never measured silence.
    fn skip_speechless_decode_window(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        range: &TailSampleRange,
    ) -> bool {
        let Some(speech) = observed_silero(self) else {
            return false;
        };
        if range.session != self.session_id
            || range.capture_epoch != self.capture_epoch
            || span_inside_observed(&speech, range.sample_end).is_none()
            || ranges_overlap_span(&speech, range.sample_start, range.sample_end)
        {
            return false;
        }
        let decode = OccurrenceIdentity::from(range);
        crate::pipeline::trail::record_decode_skipped_no_speech(&decode, &speech);
        self.window_plan_receipt(range, "decode_skipped_no_speech");
        *self
            .windows_refused_before_inference
            .entry("decode_skipped_no_speech".to_string())
            .or_default() += 1;
        self.window_plan.account(range);
        let owners = self.unsealed_word_owners();
        for (id, owner) in owners {
            if range_overlaps_occurrence(range, &owner)
                && span_inside_observed(&speech, owner.sample_end).is_some()
                && !ranges_overlap_span(&speech, owner.sample_start, owner.sample_end)
            {
                self.refinement_receipt(&owner, "decode_skipped_no_speech");
                self.return_whisper_without_label(ev_tx, id, &owner);
                self.emit_pending_seal(ev_tx, id);
            }
        }
        true
    }

    fn planned_work_pending(&self) -> bool {
        self.capture_stopping
            && !self.window_plan.is_finished()
            && (self.tail_patch.is_some() || self.refinement_lane_lost)
    }

    /// Every caller polls this same sample clock. Only one unsent request owns
    /// PCM; a full provider queue leaves both that request and its offer intact.
    fn pump_capture_windows(&mut self, ev_tx: &mpsc::UnboundedSender<EngineEvent>) {
        if self.tail_patch.is_none() && !self.refinement_lane_lost {
            return;
        }
        self.retry_refinements(ev_tx);
        while self.refinement_pending.is_empty() {
            let Some(range) = self
                .window_plan
                .next_due(self.audio.session_sample_end(), self.capture_stopping)
            else {
                break;
            };
            if self.skip_speechless_decode_window(ev_tx, &range) {
                continue;
            }
            if self.refinement_lane_lost {
                self.window_plan_receipt(&range, "lane_unavailable");
                self.window_plan.account(&range);
                continue;
            }
            let window = self
                .recognizer_window_by_samples(range.sample_start, range.sample_end)
                .filter(|window| {
                    window.sample_start == range.sample_start
                        && window.sample_end == range.sample_end
                        && window.samples.len() as u64 == range.sample_end - range.sample_start
                });
            let Some(window) = window else {
                if self.capture_stopping && self.terminal_archive_pending {
                    break;
                }
                self.window_plan_receipt(&range, "pcm_unavailable");
                self.note_window_refused_before_inference(RefinementFailure::PcmUnavailable);
                self.unresolved_windows = self.unresolved_windows.saturating_add(1);
                self.window_plan.account(&range);
                let owners = self.unsealed_word_owners();
                let mut ledger = self
                    .acoustic_ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                for (_, owner) in owners {
                    if range_overlaps_occurrence(&range, &owner) {
                        ledger.require_text_recovery(&owner);
                    }
                }
                continue;
            };
            let member_occurrences = self
                .unsealed_word_owners()
                .into_iter()
                .filter(|(_, owner)| range_overlaps_occurrence(&range, owner))
                .collect::<Vec<_>>();
            let request_id = range.sample_start / (u64::from(self.sample_rate) * 3) + 1;
            let committed_text = member_occurrences
                .iter()
                .filter_map(|(id, _)| self.pending_events.get(id))
                .map(|pending| pending.layer1_baseline.as_str())
                .filter(|label| !label.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            self.refinement_pending.push_back(TailPatchRequest {
                submission_sequence: 0,
                utterance_id: request_id,
                committed_text,
                neighbour_context: self.sealed_prefix.clone(),
                audio: window.samples,
                admit_sample_start: range.sample_start,
                admit_sample_end: range.sample_end,
                member_occurrences,
                provider_request: TailProviderRequest {
                    identity: TailRequestIdentity { request_id, range },
                    sample_rate: self.sample_rate,
                    language: None,
                },
            });
            self.retry_refinements(ev_tx);
        }
        self.replay_completed_windows(ev_tx);
        if self.capture_stopping && !self.terminal_window_tail_replayed {
            self.terminal_window_tail_replayed = true;
            let head = self.audio.session_sample_end();
            if let Some(last) = self
                .retained_word_decodes
                .iter()
                .find(|payload| payload.identity.range.sample_end == head)
                .cloned()
            {
                let request_id = last.identity.request_id;
                self.replayed_window_owners
                    .retain(|(id, _)| *id != request_id);
                let end = self
                    .window_plan
                    .publication_band(&last.identity.range, head, true)
                    .1;
                let start = self
                    .window_plan
                    .publication_band(&last.identity.range, head, false)
                    .1;
                self.admit_window_publication(ev_tx, &last, false, (start, end), true);
            }
        }
        self.close_admission_horizon(ev_tx, self.window_plan.admission_horizon());
    }

    fn note_window_refused_before_inference(&mut self, reason: RefinementFailure) {
        *self
            .windows_refused_before_inference
            .entry(reason.code().to_string())
            .or_default() += 1;
    }

    fn session_conservation(&self) -> SessionConservationReceipt {
        let ledger = self
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        SessionConservationReceipt::from_ledger(
            &ledger,
            self.windows_admitted,
            self.windows_coalesced,
            self.unresolved_windows,
            self.windows_refused_before_inference.clone(),
        )
    }

    fn refinement_receipt(&self, occurrence: &OccurrenceIdentity, disposition: &'static str) {
        info!(
            session = %occurrence.session,
            capture_epoch = occurrence.capture_epoch,
            sample_start = occurrence.sample_start,
            sample_end = occurrence.sample_end,
            elapsed_ms = self.refinement_clock.saturating_duration_since(self.refinement_started).as_millis() as u64,
            disposition,
            "live_refinement"
        );
    }

    fn fail_refinement(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        id: u64,
        occurrence: &OccurrenceIdentity,
        reason: RefinementFailure,
    ) {
        self.refinement_receipt(occurrence, reason.code());
        let _ = ev_tx.send(EngineEvent::Warning {
            code: reason.code().into(),
            message: format!(
                "session={} epoch={} occurrence={} samples={}..{} refinement_not_completed=true",
                occurrence.session,
                occurrence.capture_epoch,
                id,
                occurrence.sample_start,
                occurrence.sample_end,
            ),
        });
        if matches!(reason, RefinementFailure::StopDeadline) {
            self.return_whisper_without_label(ev_tx, id, occurrence);
            self.emit_pending_seal(ev_tx, id);
            return;
        }
        if matches!(reason, RefinementFailure::NotScheduled) {
            // One code covers "never sent" and "matched, then refused".
            // A blank owner still owes a witness, so the frontier stays open.
            let blank = self
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .text_of(occurrence)
                .is_none_or(|label| label.trim().is_empty());
            if !blank {
                self.return_whisper_without_label(ev_tx, id, occurrence);
                self.emit_pending_seal(ev_tx, id);
            }
            return;
        }
        let overlaps_queue = self.refinement_pending.iter().any(|job| {
            occurrence.sample_start < job.admit_sample_end
                && job.admit_sample_start < occurrence.sample_end
        }) || self.refinement_submitted.values().any(|job| {
            occurrence.sample_start < job.admit_sample_end
                && job.admit_sample_start < occurrence.sample_end
        });
        // A dead lane cannot deliver a later frame. The capture horizon is
        // remaining work only while a sender can still accept it.
        let has_work = overlaps_queue
            || (!matches!(reason, RefinementFailure::LaneGone)
                && self.window_plan.admission_horizon() < occurrence.sample_end);
        // A failed return contains no acoustic label observation. Preserve
        // any committed label; only a blank owner needs the missing-label receipt.
        let preserve_committed_label = self
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .text_of(occurrence)
            .is_some_and(|label| !label.trim().is_empty());
        if !preserve_committed_label {
            let observation = self
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .next_word_observation(LedgerObservationProducer::Whisper, id, occurrence);
            let _ = admit_ledger_label(
                self,
                ev_tx,
                LabelAdmission {
                    observation,
                    label: "",
                    energy: EnergyAdmission::RequireExistingQualification,
                },
            );
        }
        if !has_work {
            self.return_whisper_without_label(ev_tx, id, occurrence);
            self.emit_pending_seal(ev_tx, id);
        }
    }

    /// Outstanding provider work has one owner: the registered submissions.
    /// Pending seals can instead be held by Apple and do not imply a decode.
    fn tail_patch_awaiting_completion(&self) -> u64 {
        self.refinement_submitted.len() as u64
    }

    /// Capture EOF is not a text barrier. A fast stop may hand off only after
    /// every scheduled observer returned and every measured speech range has
    /// a sealed label. Otherwise the terminal drain owns the first delivery.
    fn stop_document_settled(&self) -> bool {
        if !self.refinement_pending.is_empty()
            || !self.refinement_submitted.is_empty()
            || !self.pending_whisper_stubs.is_empty()
            || self.planned_work_pending()
            || !self.cloud_inflight.is_empty()
            || !self.cloud_uncommitted.is_empty()
            || !self.cloud_commit_retry.is_empty()
        {
            return false;
        }
        let speech = coverage_speech_evidence(self);
        let ledger = self
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !ledger
            .pending_text_recoveries(&self.session_id, self.capture_epoch)
            .is_empty()
            || ledger.qualified_occurrences().any(|owner| {
                owner.session == self.session_id
                    && owner.capture_epoch == self.capture_epoch
                    && !ledger.is_sealed(owner)
            })
        {
            return false;
        }
        // The fast path cannot hide even a short pending speech tail behind
        // the terminal coverage tolerance. Recovery decides that tail later.
        ledger
            .assess_seal_coverage(&self.session_id, self.capture_epoch, &speech, 0)
            .status
            .is_complete()
    }

    fn note_tail_vad(&mut self, payload: &TailProviderPayload) {
        let range = &payload.identity.range;
        if range.session != self.session_id || range.capture_epoch != self.capture_epoch {
            return;
        }
        if let Some(selected) = payload.evidence.vad_selected_samples {
            let heard = selected > 0 || !payload.segments.is_empty();
            self.tail_vad_windows
                .push((range.sample_start, range.sample_end, heard));
        }
    }

    fn retain_word_decode(&mut self, payload: &TailProviderPayload) {
        let range = &payload.identity.range;
        if payload.validate().is_err()
            || payload.evidence.source != crate::stt::tail_provider::TailEvidenceSource::Whisper
            || payload.evidence.timing_quality
                != crate::stt::tail_provider::TailTimingQuality::ExactSampleRange
            || payload.evidence.stability != crate::stt::tail_provider::TailEvidenceStability::Final
            || range.session != self.session_id
            || range.capture_epoch != self.capture_epoch
            || range.sample_end > self.audio.session_sample_end()
            || range.sample_end.saturating_sub(range.sample_start) > u64::from(self.sample_rate) * 9
            || self
                .retained_word_decodes
                .iter()
                .any(|held| held.identity.range == *range)
        {
            return;
        }
        self.retained_word_decodes.push_back(payload.clone());
    }

    /// A grid return can precede closure of its real Silero owner. Retain its
    /// original witness and replay it only into newly qualified owners.
    fn replay_completed_windows(&mut self, ev_tx: &mpsc::UnboundedSender<EngineEvent>) {
        let owners = self.unsealed_word_owners();
        if owners.is_empty() {
            return;
        }
        // Retained decodes and unsealed owners both grow over a long take, and
        // owners whose text recovery never lands stay unsealed from the first
        // minute on, so an envelope over all owners no longer prunes anything.
        // Owners sorted by start, with the running maximum of their ends, give
        // each decode exactly the owners it can overlap: those starting before
        // it ends, scanned back until no earlier owner reaches past its start.
        // The result is the same set the full product would return.
        let mut by_start = owners.iter().map(|(_, owner)| owner).collect::<Vec<_>>();
        by_start.sort_by_key(|owner| owner.sample_start);
        let reach = by_start
            .iter()
            .scan(0, |end, owner| {
                *end = owner.sample_end.max(*end);
                Some(*end)
            })
            .collect::<Vec<_>>();
        let returned = self
            .retained_word_decodes
            .iter()
            .filter(|payload| {
                let range = &payload.identity.range;
                let starts_before_end =
                    by_start.partition_point(|owner| owner.sample_start < range.sample_end);
                (0..starts_before_end)
                    .rev()
                    .take_while(|&index| reach[index] > range.sample_start)
                    .any(|index| {
                        let owner = by_start[index];
                        range_overlaps_occurrence(range, owner)
                            && !self
                                .replayed_window_owners
                                .contains(&(payload.identity.request_id, owner.clone()))
                    })
            })
            .cloned()
            .collect::<Vec<_>>();
        if returned.is_empty() {
            return;
        }
        for payload in returned {
            self.admit_completed_window(ev_tx, &payload, false);
        }
        self.publish_unadmitted_words(ev_tx);
    }

    fn publish_resolved_word_seals(&mut self, ev_tx: &mpsc::UnboundedSender<EngineEvent>) {
        if !self.seal_ledger_enabled {
            return;
        }
        let owners = self.unsealed_word_owners();
        for (id, owner) in owners {
            let seal = {
                let mut ledger = self
                    .acoustic_ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if ledger.is_sealed(&owner) {
                    continue;
                }
                // A lexical trial may have been the last outstanding owner
                // after ASR returned. The seal does not wait for formatting;
                // formatting reads the label only once it is committed.
                ledger.seal(&owner).cloned().ok()
            };
            if let Some(receipt) = seal {
                let _ = ev_tx.send(EngineEvent::LedgerSeal { receipt });
                let ledger = self
                    .acoustic_ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                schedule_formatter_for_sealed_label(
                    &ledger,
                    self.formatter.as_ref(),
                    &mut self.formatter_requested,
                    &owner,
                );
                drop(ledger);
                self.emit_pending_seal(ev_tx, id);
            }
        }
    }

    /// One bounded nonblocking attempt per pending request, stopping at pressure.
    fn retry_refinements(&mut self, ev_tx: &mpsc::UnboundedSender<EngineEvent>) {
        while let Some(mut request) = self.refinement_pending.pop_front() {
            let range = request.provider_request.identity.range.clone();
            if self
                .window_plan
                .next_due(self.audio.session_sample_end(), self.capture_stopping)
                .as_ref()
                != Some(&range)
            {
                self.window_plan_receipt(&range, "unoffered_request");
                self.note_window_refused_before_inference(RefinementFailure::InvalidIdentity);
                continue;
            }
            if self.skip_speechless_decode_window(ev_tx, &range) {
                continue;
            }
            let Some(sender) = self.tail_patch.as_ref() else {
                self.window_plan_receipt(&range, "lane_unavailable");
                self.window_plan.account(&range);
                for (id, occurrence) in &request.member_occurrences {
                    self.fail_refinement(ev_tx, *id, occurrence, RefinementFailure::LaneGone);
                }
                continue;
            };
            request.submission_sequence = self
                .last_submission_sequence
                .checked_add(1)
                .expect("Whisper submission sequence exhausted");
            let inflight = TailPatchInFlight {
                submission_sequence: request.submission_sequence,
                utterance_id: request.utterance_id,
                request_identity: request.provider_request.identity.clone(),
                admit_sample_start: request.admit_sample_start,
                admit_sample_end: request.admit_sample_end,
                member_occurrences: request.member_occurrences.clone(),
            };
            match sender.try_send(request) {
                Ok(()) => {
                    self.window_plan.account(&range);
                    self.window_plan_receipt(&range, "accepted");
                    self.windows_admitted = self.windows_admitted.saturating_add(1);
                    if inflight.member_occurrences.len() > 1 {
                        self.windows_coalesced = self.windows_coalesced.saturating_add(1);
                    }
                    for (_, occurrence) in &inflight.member_occurrences {
                        self.refinement_receipt(occurrence, "submitted");
                    }
                    self.last_submission_sequence = inflight.submission_sequence;
                    self.refinement_submitted.insert(
                        inflight_key(inflight.submission_sequence, &inflight.request_identity),
                        inflight,
                    );
                }
                Err(mpsc::error::TrySendError::Full(request)) => {
                    self.refinement_pending.push_front(request);
                    break;
                }
                Err(mpsc::error::TrySendError::Closed(request)) => {
                    self.window_plan.account(&range);
                    self.window_plan_receipt(&range, "lane_unavailable");
                    self.tail_patch = None;
                    self.refinement_lane_lost = true;
                    for (id, occurrence) in &request.member_occurrences {
                        self.fail_refinement(ev_tx, *id, occurrence, RefinementFailure::LaneGone);
                    }
                }
            }
        }
    }

    /// Return failed owner obligations without discarding shared returned work.
    fn settle_whisper_reservations(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        affected: BTreeSet<OccurrenceIdentity>,
        reason: RefinementFailure,
    ) {
        for occurrence in affected {
            let id = self
                .pending_events
                .iter()
                .find_map(|(id, pending)| (pending.occurrence == occurrence).then_some(*id))
                .unwrap_or(0);
            self.fail_refinement(ev_tx, id, &occurrence, reason);
        }
    }

    /// Called on every live worker turn, independent of Apple engine lifecycle.
    fn tick_refinements(&mut self, ev_tx: &mpsc::UnboundedSender<EngineEvent>, now: Instant) {
        self.flush_cloud_commits(ev_tx);
        self.refinement_clock = now;
        if self.refinement_lane_lost
            || self
                .tail_patch
                .as_ref()
                .is_some_and(mpsc::Sender::is_closed)
        {
            self.tail_patch = None;
            self.refinement_lane_lost = true;
            let affected = self
                .unsealed_word_owners()
                .into_iter()
                .map(|(_, owner)| owner)
                .collect();
            self.settle_whisper_reservations(ev_tx, affected, RefinementFailure::LaneGone);
        }
        self.pump_capture_windows(ev_tx);
        self.publish_resolved_word_seals(ev_tx);
    }

    /// Collect returned evidence while its request and producer reservations
    /// still exist. Stop must do this before expiry can remove those identities:
    /// a ready channel message is returned work, not a timed-out inference.
    /// Validation and mutation remain with the same live admission methods.
    fn collect_ready_refinement_returns(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        tail_patch_done: &std_mpsc::Receiver<TailPatchCompletion>,
        cloud_notice: Option<&std_mpsc::Receiver<CloudWorkerNotice>>,
        audio_secs: f32,
    ) {
        if let Some(notices) = cloud_notice {
            while let Ok(notice) = notices.try_recv() {
                self.handle_cloud_notice(ev_tx, notice);
            }
        }
        while let Ok(completion) = tail_patch_done.try_recv() {
            self.complete_whisper_window(ev_tx, completion, audio_secs);
        }
    }

    /// Return true while Stop still owns work. One absolute deadline applies
    /// to both pending transport and submitted jobs; completion races are
    /// resolved by the same registered request identity as the live path.
    fn stop_refinements_tick(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        now: Instant,
        deadline: Instant,
    ) -> bool {
        if now >= deadline {
            self.refinement_clock = now;
            self.return_outstanding_whisper_without_label(ev_tx);
            return false;
        }
        self.tick_refinements(ev_tx, now);
        self.planned_work_pending()
            || !self.refinement_submitted.is_empty()
            || !self.refinement_pending.is_empty()
    }

    fn new_with_tail_patch_for_session(
        sample_rate: u32,
        session_id: String,
        capture_epoch: u64,
        tail_patch: mpsc::Sender<TailPatchRequest>,
        acoustic_ledger: Arc<Mutex<AcousticLedger>>,
        energy_calibration: Option<EnergyCalibration>,
    ) -> Self {
        Self {
            tail_patch: Some(tail_patch),
            ..Self::new_for_session_with_ledger(
                sample_rate,
                session_id,
                capture_epoch,
                acoustic_ledger,
                energy_calibration,
            )
        }
    }

    /// Send every pin the admit filter used to drop. Exclusive-tail pins stay
    /// with their member; covered overlap is a named refusal; the rest stays
    /// visible at its own PCM range and does not enter the committed map.
    /// An utterance pin that intersects a member without fitting it blocks
    /// replacement of that whole member.
    fn route_overlap_pins(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        window: AdmitWindow,
        members: &[(u64, OccurrenceIdentity)],
        segments: &[TimedTailSegment],
        producer: LedgerObservationProducer,
    ) -> Vec<MemberPinRoute> {
        let AdmitWindow {
            request_id,
            sample_start: admit_sample_start,
            sample_end: admit_sample_end,
        } = window;
        let word_grain = !segments.is_empty()
            && segments
                .iter()
                .all(|segment| segment.grain == crate::stt::tail_provider::TailSegmentGrain::Word);
        let open_members = members
            .iter()
            .map(|(_, occurrence)| occurrence.clone())
            .collect::<Vec<_>>();
        let mut routes = vec![MemberPinRoute::default(); members.len()];
        let mut side = Vec::new();
        {
            if word_grain {
                let speech = self
                    .fusion
                    .as_ref()
                    .map(|fusion| fusion.acoustic_speech_evidence());
                let mut ledger = self
                    .acoustic_ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(speech) = speech {
                    ledger.record_speech_evidence(&speech);
                }
                self.record_decode_fence_measurement(&mut ledger, None);
            }
            let ledger = self
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for (index, segment) in segments.iter().enumerate() {
                let text = segment.text.trim();
                if text.is_empty() {
                    continue;
                }
                let pin = OccurrenceIdentity::from(&segment.range);
                let mut class = ledger.classify_overlap_pin(
                    &pin,
                    admit_sample_start,
                    admit_sample_end,
                    &open_members,
                    word_grain,
                );
                // Routing chooses an open utterance, never word identity.
                // Every word hypothesis reaches the ledger's anchor matcher;
                // approximate model timestamps cannot deduplicate it here.
                if word_grain
                    && let Some(member_index) = ledger.word_owner_index(&pin, &open_members)
                    && !ledger.is_sealed(&open_members[member_index])
                {
                    class = OverlapPinClass::ExclusiveTail { member_index };
                }
                let energy_silent = self
                    .capture_energy
                    .voiced_hops_in(
                        &pin.session,
                        pin.capture_epoch,
                        pin.sample_start,
                        pin.sample_end,
                    )
                    .is_some_and(|hops| hops.is_empty());
                let silero_silent = energy_silent
                    && self.fusion.as_ref().is_some_and(|fusion| {
                        let evidence = fusion.acoustic_speech_evidence();
                        evidence.identity().session == pin.session
                            && evidence.identity().capture_epoch == pin.capture_epoch
                            && evidence
                                .availability()
                                .observed_samples()
                                .is_some_and(|end| end >= pin.sample_end)
                            && evidence.ranges().iter().all(|range| {
                                range.sample_end <= pin.sample_start
                                    || range.sample_start >= pin.sample_end
                            })
                    });
                // Until a CTC witness exists, Silero and this energy ladder are
                // the only joint defence against Whisper words on silence.
                // Replay is already a refusal; missing Silero coverage is not silence.
                if energy_silent && silero_silent && !matches!(class, OverlapPinClass::Replay) {
                    side.push((index, pin, text.to_string(), SidePin::NoVoicedHop));
                    continue;
                }
                // An unanchored pin is painted as evidence. When no qualified
                // occurrence holds its PCM and that PCM measures below the
                // existence predicate qualification refused, the paint would
                // show a word over sound the ledger did not admit.
                if matches!(class, OverlapPinClass::Unanchored(_))
                    && !ledger
                        .qualified_occurrences_overlapping(&pin)
                        .any(|owner| pin_intersects(&pin, owner))
                    && pcm_measured_below_existence(self, &pin)
                {
                    side.push((index, pin, text.to_string(), SidePin::BelowExistence));
                    continue;
                }
                match class {
                    OverlapPinClass::ExclusiveTail { member_index } => {
                        routes[member_index].exclusive.push(RoutedPin {
                            index,
                            pin: pin.clone(),
                            text: text.to_string(),
                            confidence: segment.confidence,
                        });
                    }
                    OverlapPinClass::Replay => {
                        // Preserve the replay receipt while allowing an earlier
                        // PCM representative to replace a jittered copy that
                        // completed first. A broad Split also receives its held
                        // token here without changing any pin's replay verdict.
                        if word_grain
                            && let Some((owner_index, owner)) = ledger
                                .word_owner_index(&pin, &open_members)
                                .map(|index| (index, &open_members[index]))
                            && !ledger.is_sealed(owner)
                            && ledger.matching_word_slot(owner, &pin, text, true)
                        {
                            routes[owner_index].exclusive.push(RoutedPin {
                                index,
                                pin: pin.clone(),
                                text: text.to_string(),
                                confidence: segment.confidence,
                            });
                        }
                        side.push((index, pin, text.to_string(), SidePin::Replay));
                    }
                    OverlapPinClass::Unanchored(reason) => {
                        // Utterance grain addresses the whole span or nothing.
                        // A word with no midpoint owner stays read-only evidence.
                        if !word_grain {
                            for (member_index, member) in open_members.iter().enumerate() {
                                if pin_intersects(&pin, member) {
                                    routes[member_index].blocked = true;
                                }
                            }
                        }
                        side.push((index, pin, text.to_string(), SidePin::Unanchored(reason)));
                    }
                }
            }
        }
        for (index, pin, text, disposition) in side {
            let occurrence_ranges = open_members
                .iter()
                .filter(|member| pin_intersects(&pin, member))
                .map(|member| format!("{}..{}", member.sample_start, member.sample_end))
                .collect::<Vec<_>>()
                .join(",");
            let ledger_reason = match &disposition {
                SidePin::Replay => "replayed_range_identity",
                SidePin::Unanchored(reason) => reason.as_str(),
                SidePin::NoVoicedHop | SidePin::BelowExistence => "no_voiced_hop_in_pin",
            };
            let _ = ev_tx.send(EngineEvent::Warning {
                code: crate::pipeline::contracts::OVERLAP_PIN_REFUSED.into(),
                message: crate::pipeline::contracts::overlap_pin_refused_message(
                    (pin.sample_start, pin.sample_end),
                    &occurrence_ranges,
                    ledger_reason,
                ),
            });
            let observation = LedgerObservationIdentity::new(
                producer,
                request_id,
                1_000 + admit_sample_start + index as u64,
                pin,
            );
            if producer == LedgerObservationProducer::CloudLive
                && matches!(disposition, SidePin::Unanchored(_))
            {
                self.cloud_live_unowned_routed = self.cloud_live_unowned_routed.saturating_add(1);
            }
            let receipt = {
                let mut ledger = self
                    .acoustic_ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match disposition {
                    SidePin::Replay => ledger.refuse_replayed_range(&observation, &text),
                    SidePin::Unanchored(reason) => {
                        // An unrouted edge Word is not an accounted return.
                        // Preserve its original decode/pin before ordinary
                        // admission can retire the coarse source and seal it.
                        // This is recovery debt, never word ownership or a
                        // completeness receipt for the clipped hypothesis.
                        let intersecting = open_members
                            .iter()
                            .filter(|owner| pin_intersects(&observation.occurrence, owner))
                            .collect::<Vec<_>>();
                        if word_grain
                            && producer == LedgerObservationProducer::Whisper
                            && text.split_whitespace().count() == 1
                            && admit_sample_start < admit_sample_end
                            && admit_sample_end.saturating_sub(admit_sample_start)
                                <= u64::from(self.sample_rate) * 9
                            && admit_sample_start <= observation.occurrence.sample_start
                            && observation.occurrence.sample_end <= admit_sample_end
                            && let [owner] = intersecting.as_slice()
                            && ledger.is_qualified(owner)
                            && !ledger.is_sealed(owner)
                            && (!ledger.decode_word_onset_complete(
                                owner,
                                admit_sample_start,
                                admit_sample_end,
                                observation.occurrence.sample_start,
                            ) || ledger.decode_word_fence_incomplete(
                                owner,
                                admit_sample_start,
                                admit_sample_end,
                                observation.occurrence.sample_start,
                                observation.occurrence.sample_end,
                            ))
                            && self
                                .capture_energy
                                .voiced_hops_in(
                                    &owner.session,
                                    owner.capture_epoch,
                                    observation.occurrence.sample_start.max(owner.sample_start),
                                    observation.occurrence.sample_end.min(owner.sample_end),
                                )
                                .is_some_and(|hops| !hops.is_empty())
                        {
                            let mut recovery =
                                ledger.next_word_observation(producer, request_id, owner);
                            recovery.request = request_id;
                            let mut pin = crate::pipeline::acoustic_ledger::WordPin::new(
                                observation.occurrence.sample_start,
                                observation.occurrence.sample_end,
                                text.clone(),
                            )
                            .with_decode_window(admit_sample_start, admit_sample_end);
                            pin.confidence = segments[index].confidence;
                            ledger.retain_rejected_decode_word(&recovery, &pin);
                        }
                        ledger.keep_visible_unanchored(&observation, &text, reason)
                    }
                    SidePin::NoVoicedHop => {
                        ledger.note_energy_lookup_without_voiced_hop();
                        ledger.refuse_replacement(
                            &observation,
                            &text,
                            RefuseReason::NoVoicedHopInPin,
                        )
                    }
                    // Hops did reach the floor, so the voiced-hop lookup
                    // counter is not touched; the refusal is the same.
                    SidePin::BelowExistence => ledger.refuse_replacement(
                        &observation,
                        &text,
                        RefuseReason::NoVoicedHopInPin,
                    ),
                }
            };
            let _ = ev_tx.send(EngineEvent::LedgerMutation {
                observation,
                label: text,
                receipt,
            });
        }
        routes
    }

    fn keep_routed_visible(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        request_id: u64,
        pins: &[RoutedPin],
    ) {
        for pin in pins {
            let observation = LedgerObservationIdentity::new(
                LedgerObservationProducer::Whisper,
                request_id,
                1_000 + pin.index as u64,
                pin.pin.clone(),
            );
            let receipt = {
                let mut ledger = self
                    .acoustic_ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                ledger.keep_visible_unanchored(
                    &observation,
                    &pin.text,
                    NoAuthorityReason::ExclusiveTailAwaitingWholeSpan,
                )
            };
            let _ = ev_tx.send(EngineEvent::LedgerMutation {
                observation,
                label: pin.text.clone(),
                receipt,
            });
        }
    }

    fn emit_span_refusal(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        request_id: u64,
        occurrence: &OccurrenceIdentity,
        label: &str,
        reason: RefuseReason,
    ) {
        let observation = LedgerObservationIdentity::new(
            LedgerObservationProducer::Whisper,
            request_id,
            10_000,
            occurrence.clone(),
        );
        let receipt = {
            let mut ledger = self
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ledger.refuse_replacement(&observation, label, reason)
        };
        let _ = ev_tx.send(EngineEvent::LedgerMutation {
            observation,
            label: label.to_string(),
            receipt,
        });
    }

    /// Only the exact, final Whisper return for accepted PCM may reach the
    /// occurrence ledger. Rejected work remains accounted by the capture plan.
    fn complete_whisper_window(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        completion: TailPatchCompletion,
        _now_secs: f32,
    ) {
        let TailPatchCompletion {
            submission_sequence,
            utterance_id,
            request_identity,
            payload,
            member_occurrences,
        } = completion;
        let key = request_identity
            .as_ref()
            .map(|identity| inflight_key(submission_sequence, identity));
        let matched = key.as_ref().is_some_and(|key| {
            self.refinement_submitted.get(key).is_some_and(|job| {
                job.utterance_id == utterance_id
                    && request_identity.as_ref() == Some(&job.request_identity)
                    && member_occurrences == job.member_occurrences
            })
        });
        if !matched {
            info!(
                submission_sequence,
                utterance_id,
                request_id = ?request_identity
                    .as_ref()
                    .map(|identity| identity.request_id),
                sample_start = ?request_identity
                    .as_ref()
                    .map(|identity| identity.range.sample_start),
                sample_end = ?request_identity
                    .as_ref()
                    .map(|identity| identity.range.sample_end),
                disposition = "unmatched_completion",
                "live_refinement"
            );
            return;
        }
        let job = self
            .refinement_submitted
            .remove(&key.expect("matched key"))
            .expect("matched job");
        // Any matched return, valid or not, proves the engine is past
        // cold start; later stops may hold the warm closure budget.
        self.first_whisper_window_completed = true;
        if !self.seal_ledger_enabled {
            let quiet_before = self.direct_suppressed_no_measured_speech;
            let outcome = if let Some(payload) = payload.filter(|payload| {
                payload.identity == job.request_identity
                    && payload.validate().is_ok()
                    && payload.evidence.source == TailEvidenceSource::Whisper
                    && payload.evidence.stability
                        == crate::stt::tail_provider::TailEvidenceStability::Final
            }) {
                self.window_plan_receipt(&job.request_identity.range, "returned");
                let text = direct_text_heard_by_silero(self, &payload);
                let every_word_over_silence =
                    text.trim().is_empty() && !payload.text.trim().is_empty();
                let range = payload.identity.range;
                let observation = LedgerObservationIdentity::new(
                    LedgerObservationProducer::Whisper,
                    job.request_identity.request_id,
                    0,
                    OccurrenceIdentity::new(
                        range.session,
                        range.capture_epoch,
                        range.sample_start,
                        range.sample_end,
                    ),
                );
                if every_word_over_silence
                    && !self.direct_range_measured_quiet(&observation.occurrence)
                {
                    // Speech exists in the window, but Silero heard none near
                    // any word: nothing to publish. A window with no speech at
                    // all stays with the whole-range suppression and its count.
                    DirectJobOutcome::Suppressed
                } else if self.publish_direct_transcript(ev_tx, observation, text, true) {
                    DirectJobOutcome::Published
                } else if self.direct_suppressed_no_measured_speech > quiet_before {
                    DirectJobOutcome::Suppressed
                } else {
                    DirectJobOutcome::Failed
                }
            } else {
                self.window_plan_receipt(&job.request_identity.range, "failed");
                let _ = ev_tx.send(EngineEvent::Warning {
                    code: "direct_decode_unavailable".into(),
                    message: "The accepted PCM request returned no valid final decoder payload."
                        .into(),
                });
                DirectJobOutcome::Failed
            };
            self.account_direct_job(outcome);
            return;
        }
        let valid = payload.as_ref().is_some_and(|payload| {
            payload.identity == job.request_identity
                && payload.validate().is_ok()
                && payload.evidence.source == crate::stt::tail_provider::TailEvidenceSource::Whisper
                && payload.evidence.timing_quality
                    == crate::stt::tail_provider::TailTimingQuality::ExactSampleRange
                && payload.evidence.stability
                    == crate::stt::tail_provider::TailEvidenceStability::Final
        });
        if let Some(payload) = payload.as_ref() {
            if payload.identity != job.request_identity {
                self.refuse_tail_patch(
                    ev_tx,
                    TAIL_PATCH_IDENTITY_MISMATCH_WARNING_CODE,
                    utterance_id,
                    "provider completion did not echo the accepted request identity",
                );
            } else if !valid {
                self.refuse_tail_patch(
                    ev_tx,
                    "tail_patch_untrusted_evidence",
                    utterance_id,
                    "provider completion requires valid, exact, final Whisper evidence",
                );
            }
        }
        if let Some(payload) = payload.filter(|_| valid) {
            self.window_plan_receipt(&job.request_identity.range, "returned");
            self.note_tail_vad(&payload);
            self.retain_word_decode(&payload);
            self.admit_completed_window(ev_tx, &payload, true);
            // An observed Silero onset fixes the past before that onset.
            // Recover complete Words in those older gaps now, without owning
            // the open utterance that Silero may still extend or split.
            let past_onset = self.fusion.as_ref().and_then(|fusion| {
                fusion
                    .ledger()
                    .utterances()
                    .last()
                    .filter(|utterance| {
                        utterance.range.session == self.session_id
                            && utterance.range.capture_epoch == self.capture_epoch
                    })
                    .map(|utterance| utterance.range.sample_start)
            });
            if let Some(before) = past_onset
                && admit_retained_unowned_words(self, ev_tx, &payload, Some(before))
            {
                // Retained windows supply existing witnesses before the
                // admission horizon can close a newly qualified owner.
                self.replay_completed_windows(ev_tx);
            }
        } else {
            // A matched provider failure is not measured uncovered speech.
            // Debt already recorded by qualification or reconciliation stays.
            // This return must not invent recovery debt over an admitted floor.
            self.window_plan_receipt(&job.request_identity.range, "failed");
            self.tail_patch_jobs_skipped = self.tail_patch_jobs_skipped.saturating_add(1);
            self.settle_failed_window_members(ev_tx, &job.member_occurrences);
        }
        self.close_admission_horizon(ev_tx, self.window_plan.admission_horizon());
        // Committed words may now hold speech the open partial restates. A
        // silent pause sends no new partial, so the mirror is refreshed here.
        self.publish_unadmitted_words(ev_tx);
    }

    /// One authenticated returned frame, shared by physical owners. Replay
    /// preserves request identity; the ledger decides which components have
    /// already observed this frame. Only its first return changes job counters.
    fn admit_completed_window(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        returned: &TailProviderPayload,
        fresh: bool,
    ) {
        let band = whisper_publication_band(self, &returned.identity.range);
        self.admit_window_publication(ev_tx, returned, fresh, band, false);
    }

    fn admit_window_publication(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        returned: &TailProviderPayload,
        fresh: bool,
        publication_band: (u64, u64),
        terminal_tail_only: bool,
    ) {
        let identity = &returned.identity;
        let (band_start, band_end) = publication_band;
        if band_start < identity.range.sample_start
            || band_start >= band_end
            || band_end > identity.range.sample_end
            || (terminal_tail_only
                && (!self.capture_stopping
                    || identity.range.sample_end != self.audio.session_sample_end()))
        {
            return;
        }
        let admit_sample_start = identity.range.sample_start;
        let admit_sample_end = identity.range.sample_end;
        let payload = Some(returned);
        let exact_open_members = self
            .unsealed_word_owners()
            .into_iter()
            .filter(|(_, owner)| range_overlaps_occurrence(&identity.range, owner))
            .collect::<Vec<_>>();
        let mut mutation_admitted = false;
        if let Some(payload) = payload.as_ref() {
            self.acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .bind_word_provider(payload);
        }

        let request_id = identity.request_id;
        let single_member = exact_open_members.len() == 1;
        let segments = payload
            .as_ref()
            .map(|payload| payload.segments.as_slice())
            .unwrap_or(&[]);
        let word_grain = !segments.is_empty()
            && segments
                .iter()
                .all(|segment| segment.grain == crate::stt::tail_provider::TailSegmentGrain::Word);
        if !word_grain && exact_open_members.is_empty() {
            if fresh {
                self.tail_patch_jobs_skipped = self.tail_patch_jobs_skipped.saturating_add(1);
            }
            return;
        }
        // The original payload remains bound as decode context. Only the
        // owning band's words enter admission or its speech-refusal path.
        let owned_segments = if word_grain {
            Some(
                segments
                    .iter()
                    .filter(|segment| {
                        let midpoint = segment.range.sample_start
                            + segment
                                .range
                                .sample_end
                                .saturating_sub(segment.range.sample_start)
                                / 2;
                        band_start <= midpoint && midpoint < band_end
                    })
                    .cloned()
                    .collect::<Vec<_>>(),
            )
        } else {
            None
        };
        // A successful grid return with no raw word in this band can still
        // close a measured-quiet tail. Context words outside the band are not
        // empty decoder output, and rejected in-band words are never silence.
        let timed_or_empty =
            owned_segments.is_some() || (segments.is_empty() && returned.text.trim().is_empty());
        if timed_or_empty
            && band_start < band_end
            && returned.validate().is_ok()
            && returned.evidence.source == crate::stt::tail_provider::TailEvidenceSource::Whisper
            && returned.evidence.stability
                == crate::stt::tail_provider::TailEvidenceStability::Final
            && returned.evidence.timing_quality
                == crate::stt::tail_provider::TailTimingQuality::ExactSampleRange
            && identity.range.session == self.session_id
            && identity.range.capture_epoch == self.capture_epoch
        {
            let band =
                OccurrenceIdentity::new(&self.session_id, self.capture_epoch, band_start, band_end);
            let speech = coverage_speech_evidence(self);
            let mut ledger = self
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ledger.record_speech_evidence(&speech);
            self.record_decode_fence_measurement(&mut ledger, Some((band_start, band_end)));
            for (_, owner) in &exact_open_members {
                let has_owner_word = owned_segments.as_ref().is_some_and(|words| {
                    words
                        .iter()
                        .any(|word| range_overlaps_occurrence(&word.range, owner))
                });
                if !has_owner_word
                    && band.sample_start < owner.sample_end
                    && owner.sample_start < band.sample_end
                {
                    let observation = ledger.next_word_observation(
                        LedgerObservationProducer::Whisper,
                        request_id,
                        owner,
                    );
                    ledger.record_quiet_publication_band(&observation, &band);
                }
            }
        }
        let segments = owned_segments.as_deref().unwrap_or(segments);
        let filtered_segments = payload
            .as_ref()
            .is_some_and(|payload| {
                payload.evidence.source == crate::stt::tail_provider::TailEvidenceSource::Whisper
            })
            .then(|| retain_whisper_words_with_own_speech(self, ev_tx, request_id, segments));
        let segments = filtered_segments.as_deref().unwrap_or(segments);
        let owners = if word_grain {
            self.word_owners_in_window(admit_sample_start, admit_sample_end, segments)
        } else {
            exact_open_members.clone()
        };
        // Pending words are retired by settle_whisper_stubs only after the
        // ledger identifies an admitted same-span word or its source lineage.
        // A later spelling, gap or estimated end cannot identify that speech.
        let mut routes = self.route_overlap_pins(
            ev_tx,
            AdmitWindow {
                request_id,
                sample_start: admit_sample_start,
                sample_end: admit_sample_end,
            },
            &owners,
            segments,
            LedgerObservationProducer::Whisper,
        );
        // Timestamp quantisation can hide a cut just inside the actual
        // decoder fence. The same acoustic predicate owns ledger completeness.
        let edge_incomplete = if word_grain {
            let speech = coverage_speech_evidence(self);
            let mut ledger = self
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ledger.record_speech_evidence(&speech);
            self.record_decode_fence_measurement(
                &mut ledger,
                Some((identity.range.sample_start, identity.range.sample_end)),
            );
            segments.last().is_some_and(|last| {
                ledger.decode_word_fence_incomplete(
                    &OccurrenceIdentity::from(&identity.range),
                    identity.range.sample_start,
                    identity.range.sample_end,
                    last.range.sample_start,
                    last.range.sample_end,
                )
            })
        } else {
            false
        };
        if edge_incomplete {
            let last_index = segments.len() - 1;
            for ((_, owner), route) in owners.iter().zip(&mut routes) {
                if self
                    .replayed_window_owners
                    .contains(&(request_id, owner.clone()))
                {
                    continue;
                }
                if let Some(position) = route
                    .exclusive
                    .iter()
                    .position(|pin| pin.index == last_index)
                {
                    let stub = route.exclusive.remove(position);
                    self.keep_routed_visible(ev_tx, request_id, std::slice::from_ref(&stub));
                    self.acoustic_ledger
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .require_text_recovery(owner);
                    self.pending_whisper_stubs
                        .push((identity.clone(), owner.clone(), stub));
                }
            }
        }
        for (index, ((member_id, occurrence), route)) in owners.iter().zip(&routes).enumerate() {
            if !self
                .replayed_window_owners
                .insert((request_id, occurrence.clone()))
            {
                // Returned work stays closed. A late diagnostic Word on a
                // sealed owner still reaches the ledger's visible refusal.
                let sealed_diagnostic = !fresh
                    && word_grain
                    && self
                        .acoustic_ledger
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .is_sealed(occurrence);
                if !sealed_diagnostic {
                    continue;
                }
            }
            if word_grain {
                if !route.exclusive.is_empty() {
                    let terminal_tail = terminal_tail_only
                        || (self.capture_stopping
                            && identity.range.sample_end == self.audio.session_sample_end());
                    mutation_admitted |= self.admit_routed_words_with_publication(
                        ev_tx,
                        *member_id,
                        occurrence,
                        RoutedDecode {
                            request_window: (
                                request_id,
                                Some((identity.range.sample_start, identity.range.sample_end)),
                            ),
                            publication: Some((returned, (band_start, band_end), terminal_tail)),
                        },
                        RoutedWords {
                            pins: &route.exclusive,
                            neighbours: &neighbour_pin_assignments(
                                &owners, &routes, segments, index,
                            ),
                        },
                        LedgerObservationProducer::Whisper,
                    );
                }
                continue;
            }
            // Successful empty work has its own issued decode scope. The
            // quiet new admit range need not repeat earlier accepted Words.
            if payload.as_ref().is_some_and(|payload| {
                payload.text.trim().is_empty()
                    && payload.segments.is_empty()
                    && payload.evidence.source
                        == crate::stt::tail_provider::TailEvidenceSource::Whisper
                    && payload.evidence.stability
                        == crate::stt::tail_provider::TailEvidenceStability::Final
            }) {
                let speech = coverage_speech_evidence(self);
                let accounted = {
                    let mut ledger = self
                        .acoustic_ledger
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let mut observation = ledger.next_word_observation(
                        LedgerObservationProducer::Whisper,
                        request_id,
                        occurrence,
                    );
                    // This work receipt names this issued request, not the
                    // prior lexical observation's request or decoder extent.
                    observation.request = request_id;
                    ledger.record_speech_evidence(&speech);
                    ledger.record_empty_decode_work(
                        &observation,
                        &OccurrenceIdentity::from(&identity.range),
                    )
                };
                if accounted {
                    self.refresh_pending_label(*member_id, occurrence);
                    self.refinement_receipt(occurrence, "completed");
                } else {
                    // Successful work is retained for later Word evidence;
                    // an empty payload never answers a lexical observation.
                    self.acoustic_ledger
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .require_text_recovery(occurrence);
                    self.refinement_receipt(occurrence, RefinementFailure::NoLabel.code());
                }
                continue;
            }
            // Whole utterances retain their range fence. Word-grain admission
            // above never inherits this veto or waits for full PCM coverage.
            if route.blocked
                || self.whisper_span_refused.contains(occurrence)
                || occurrence.sample_start < admit_sample_start
                || occurrence.sample_end > admit_sample_end
            {
                // A frame that only intersects this owner cannot permanently
                // veto a later issued frame that contains the entire owner.
                if route.blocked {
                    self.whisper_span_refused.insert(occurrence.clone());
                }
                self.acoustic_ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .require_text_recovery(occurrence);
                self.keep_routed_visible(ev_tx, request_id, &route.exclusive);
                self.emit_span_refusal(
                    ev_tx,
                    request_id,
                    occurrence,
                    &exclusive_label(&route.exclusive),
                    RefuseReason::IntersectingPinNotExclusive,
                );
                continue;
            }
            let text = exclusive_label(&route.exclusive);
            let label = if !text.is_empty() {
                text
            } else if single_member
                && payload.as_ref().is_some_and(|payload| {
                    payload.segments.is_empty()
                        && &OccurrenceIdentity::from(&payload.identity.range) == occurrence
                })
            {
                payload
                    .as_ref()
                    .map(|payload| payload.text.trim().to_string())
                    .unwrap_or_default()
            } else {
                String::new()
            };
            // Untimed or phrase-level Whisper has no publication-band pins.
            // It may fill an empty owner but cannot revise existing slots.
            let owner_has_slots = self
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .slots_of(occurrence)
                .is_some_and(|slots| !slots.is_empty());
            if !label.is_empty() && owner_has_slots {
                let (observation, receipt) = {
                    let mut ledger = self
                        .acoustic_ledger
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let observation = ledger.next_word_observation(
                        LedgerObservationProducer::Whisper,
                        request_id,
                        occurrence,
                    );
                    let sources = ledger.slots_of(occurrence).unwrap_or(&[]).to_vec();
                    ledger.retain_slot_alternative(
                        &observation,
                        &label,
                        sources,
                        "phrase_without_word_targets",
                    );
                    let receipt = ledger.refuse_replacement(
                        &observation,
                        &label,
                        RefuseReason::SlotAdmissionRejected,
                    );
                    (observation, receipt)
                };
                let _ = ev_tx.send(EngineEvent::LedgerMutation {
                    observation,
                    label,
                    receipt,
                });
                self.refinement_receipt(occurrence, RefinementFailure::NoLabel.code());
                continue;
            }
            // A phrase has no individual word ownership. It can fill an
            // empty owner only when its complete physical scope belongs to
            // this publication band, not merely to the overlapping decode.
            let phrase_scope_owned = returned.validate().is_ok()
                && returned.evidence.source
                    == crate::stt::tail_provider::TailEvidenceSource::Whisper
                && returned.evidence.stability
                    == crate::stt::tail_provider::TailEvidenceStability::Final
                && returned.evidence.timing_quality
                    == crate::stt::tail_provider::TailTimingQuality::ExactSampleRange
                && identity.range.session == self.session_id
                && identity.range.capture_epoch == self.capture_epoch
                && band_start <= occurrence.sample_start
                && occurrence.sample_end <= band_end
                && route.exclusive.iter().all(|pin| {
                    pin.pin.same_capture(occurrence)
                        && band_start <= pin.pin.sample_start
                        && pin.pin.sample_end <= band_end
                });
            if !label.is_empty() && !phrase_scope_owned {
                let (observation, receipt) = {
                    let mut ledger = self
                        .acoustic_ledger
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    ledger.require_text_recovery(occurrence);
                    let observation = ledger.next_word_observation(
                        LedgerObservationProducer::Whisper,
                        request_id,
                        occurrence,
                    );
                    let receipt = ledger.refuse_replacement(
                        &observation,
                        &label,
                        RefuseReason::SlotAdmissionRejected,
                    );
                    (observation, receipt)
                };
                let _ = ev_tx.send(EngineEvent::LedgerMutation {
                    observation,
                    label,
                    receipt,
                });
                self.refinement_receipt(occurrence, RefinementFailure::NoLabel.code());
                continue;
            }
            if !label.is_empty() {
                let observation = self
                    .acoustic_ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .next_word_observation(
                        LedgerObservationProducer::Whisper,
                        request_id,
                        occurrence,
                    );
                mutation_admitted |= admit_ledger_label(
                    self,
                    ev_tx,
                    LabelAdmission {
                        observation,
                        label: &label,
                        energy: EnergyAdmission::RequireExistingQualification,
                    },
                )
                .is_some_and(|receipt| receipt.grants_mutation());
                self.refresh_pending_label(*member_id, occurrence);
            } else {
                let observation = {
                    let mut ledger = self
                        .acoustic_ledger
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    ledger.require_text_recovery(occurrence);
                    ledger.next_word_observation(
                        LedgerObservationProducer::Whisper,
                        request_id,
                        occurrence,
                    )
                };
                let _ = admit_ledger_label(
                    self,
                    ev_tx,
                    LabelAdmission {
                        observation,
                        label: "",
                        energy: EnergyAdmission::RequireExistingQualification,
                    },
                );
                self.refinement_receipt(occurrence, RefinementFailure::NoLabel.code());
                if self.capture_turn == CaptureTurnIntent::AgentChannel {
                    let _ = ev_tx.send(EngineEvent::Warning {
                        code: RefinementFailure::NoLabel.code().into(),
                        message:
                            "Whisper returned no accepted label; refinement_not_completed=true"
                                .into(),
                    });
                }
            }
        }
        if word_grain {
            for (id, owner) in &owners {
                self.settle_whisper_stubs(ev_tx, *id, owner, false);
            }
        }
        self.publish_resolved_word_seals(ev_tx);
        if fresh && mutation_admitted {
            self.tail_patch_jobs_applied = self.tail_patch_jobs_applied.saturating_add(1);
            self.tail_patch_replacements = self.tail_patch_replacements.saturating_add(1);
        } else if fresh {
            self.tail_patch_jobs_skipped = self.tail_patch_jobs_skipped.saturating_add(1);
        }
    }

    fn unsealed_word_owners(&self) -> Vec<(u64, OccurrenceIdentity)> {
        let ledger = self
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.map_word_owners(ledger.unsealed_qualified_occurrences())
    }

    fn word_owners_overlapping(
        &self,
        sample_start: u64,
        sample_end: u64,
    ) -> Vec<(u64, OccurrenceIdentity)> {
        let range = OccurrenceIdentity::new(
            &self.session_id,
            self.capture_epoch,
            sample_start,
            sample_end,
        );
        let ledger = self
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.map_word_owners(ledger.qualified_occurrences_overlapping(&range))
    }

    fn word_owners_in_window(
        &self,
        sample_start: u64,
        sample_end: u64,
        segments: &[TimedTailSegment],
    ) -> Vec<(u64, OccurrenceIdentity)> {
        let start = segments
            .iter()
            .map(|segment| segment.range.sample_start)
            .min()
            .map_or(sample_start, |start| start.min(sample_start));
        let end = segments
            .iter()
            .map(|segment| segment.range.sample_end)
            .max()
            .map_or(sample_end, |end| end.max(sample_end));
        self.word_owners_overlapping(start, end)
    }

    fn map_word_owners<'a>(
        &self,
        owners: impl Iterator<Item = &'a OccurrenceIdentity>,
    ) -> Vec<(u64, OccurrenceIdentity)> {
        let pending_ids =
            self.pending_events
                .iter()
                .fold(BTreeMap::new(), |mut ids, (id, pending)| {
                    ids.entry(&pending.occurrence).or_insert(*id);
                    ids
                });
        owners
            .filter(|owner| {
                owner.session == self.session_id && owner.capture_epoch == self.capture_epoch
            })
            .map(|owner| {
                let id = pending_ids.get(owner).copied().unwrap_or(0);
                (id, owner.clone())
            })
            .collect()
    }

    fn refresh_pending_label(&mut self, id: u64, owner: &OccurrenceIdentity) {
        let ledger = self
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(label) = ledger.text_of(owner)
            && let Some(pending) = self.pending_events.get_mut(&id)
            && pending.occurrence == *owner
        {
            pending.layer1_baseline = label.to_string();
        }
        if !ledger.text_recovery_pending(owner)
            && let Some(fusion) = self.fusion.as_ref()
        {
            self.speech_progress
                .recovered_occurrence(owner, &fusion.acoustic_speech_evidence());
        }
    }

    /// Use the existing Silero observer and retained PCM, on each exact word
    /// range. A missing window or unresolved sound leaves the label untouched.
    fn adjudicate_current_word_slots(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        owner: &OccurrenceIdentity,
        request: u64,
    ) {
        use crate::pipeline::acoustic_ledger::SlotTarget;
        use crate::pipeline::acoustic_ledger::word_verdict::{WordVerdict, adjudicate_word_pcm};

        let Some(fusion) = self.fusion.as_ref() else {
            return;
        };
        let speech = fusion.acoustic_speech_evidence();
        let slots = self
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .slots_of(owner)
            .unwrap_or(&[])
            .to_vec();
        for slot in slots {
            if slot.producer == LedgerObservationProducer::ManualHuman {
                continue;
            }
            let ranges = self
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .slot_source_ranges(&slot);
            // Disjoint merge sources require separate evidence; the bounding
            // box is not this word's PCM and cannot authorize its deletion.
            let [range] = ranges.as_slice() else {
                continue;
            };
            let Some(window) = self.window_by_samples(range.sample_start, range.sample_end) else {
                continue;
            };
            let pcm_range = OccurrenceIdentity::new(
                &self.session_id,
                self.capture_epoch,
                window.sample_start,
                window.sample_end,
            );
            let verdict = adjudicate_word_pcm(range, &pcm_range, &window.samples, &speech);
            if verdict.verdict() != WordVerdict::ConfirmedNoSpeech {
                continue;
            }
            let mut ledger = self
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let observation =
                ledger.next_word_observation(LedgerObservationProducer::Whisper, request, owner);
            if ledger
                .remove_word_with_verdict(&observation, &SlotTarget::from(&slot), &verdict)
                .is_ok()
            {
                let receipt = ledger
                    .layer_trail()
                    .last()
                    .expect("word deletion decision")
                    .decision
                    .clone();
                let label = ledger.text_of(owner).unwrap_or("").to_string();
                let _ = ev_tx.send(EngineEvent::LedgerMutation {
                    observation,
                    label,
                    receipt,
                });
            }
        }
    }

    /// Fuzzy retrieval can label only settled, exact current word targets.
    /// Two independent complete Whisper frames must retrieve the same canonical
    /// from the raw labels for every source; at least one must actually hear a
    /// registered form of that canonical. Agreement on a fuzzy-only source is
    /// not evidence for replacing it. No guessed label becomes ASR input.
    fn apply_corroborated_lexicon(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        owner: &OccurrenceIdentity,
        request: u64,
    ) {
        use crate::pipeline::acoustic_ledger::{DictionarySlotRule, SlotTarget};
        if self.capture_stopping {
            return;
        }
        let mut ledger = self
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if ledger.is_sealed(owner) {
            return;
        }
        let sources = ledger.slots_of(owner).unwrap_or(&[]).to_vec();
        let lexicon = super::live_lexicon::FuzzyLexicon::load(&self.lexicon_custom_path);
        let mut cursor = 0;
        while cursor < sources.len() {
            let mut consumed = 1;
            for count in (1..=3.min(sources.len() - cursor)).rev() {
                let selected = &sources[cursor..cursor + count];
                if selected.iter().any(|source| {
                    source.producer == LedgerObservationProducer::ManualHuman
                        || source.surface_rewritten
                        || source.text.split_whitespace().count() != 1
                }) || selected.windows(2).any(|pair| {
                    pair[0].sample_end > pair[1].sample_start
                        || pair[0]
                            .text
                            .trim_matches(|c: char| !c.is_alphanumeric())
                            .to_lowercase()
                            == pair[1]
                                .text
                                .trim_matches(|c: char| !c.is_alphanumeric())
                                .to_lowercase()
                }) {
                    continue;
                }
                let input = selected
                    .iter()
                    .map(|source| source.text.clone())
                    .collect::<Vec<_>>();
                let text = input.join(" ");
                // No internal sentence boundary can be swallowed by a merge.
                if selected[..count - 1].iter().any(|source| {
                    source
                        .text
                        .chars()
                        .any(|c| matches!(c, ',' | '.' | '!' | '?' | ';' | ':' | '…'))
                }) {
                    continue;
                }
                let targets = selected.iter().map(SlotTarget::from).collect::<Vec<_>>();
                let witnesses = ledger.dictionary_witnesses(owner, &targets);
                if witnesses.len() < 2 {
                    continue;
                }
                let Some(canonical) = lexicon.candidate(&text) else {
                    continue;
                };
                if text.trim_matches(|c: char| !c.is_alphanumeric()) == canonical {
                    continue;
                }
                if witnesses
                    .iter()
                    .any(|(_, raw)| lexicon.candidate(raw).as_deref() != Some(canonical.as_str()))
                    || !witnesses.iter().any(|(_, raw)| {
                        lexicon.registered_candidate(raw).as_deref() == Some(canonical.as_str())
                    })
                {
                    continue;
                }
                let prefix = text
                    .chars()
                    .take_while(|c| !c.is_alphanumeric())
                    .collect::<String>();
                let suffix = text
                    .chars()
                    .rev()
                    .take_while(|c| !c.is_alphanumeric())
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect::<String>();
                let rule = DictionarySlotRule {
                    id: format!(
                        "corroborated-lexicon/v1/{:?}/{}=>{}",
                        witnesses, text, canonical
                    ),
                    input,
                    canonical: format!("{prefix}{canonical}{suffix}"),
                };
                let observation = ledger.next_word_observation(
                    LedgerObservationProducer::Lexicon,
                    request,
                    owner,
                );
                let receipt = if count == 1 {
                    ledger
                        .rewrite_dictionary_slots(&observation, &[(targets[0].clone(), rule)])
                        .ok()
                        .filter(MutationReceipt::grants_mutation)
                } else {
                    ledger
                        .merge_word_slots(&observation, &targets, &rule)
                        .ok()
                        .and_then(|_| {
                            ledger
                                .layer_trail()
                                .last()
                                .map(|entry| entry.decision.clone())
                        })
                };
                let Some(receipt) = receipt else {
                    continue;
                };
                let label = ledger.text_of(owner).unwrap_or("").to_string();
                self.lexicon_rewrites = self.lexicon_rewrites.saturating_add(1);
                let _ = ev_tx.send(EngineEvent::LedgerMutation {
                    observation,
                    label,
                    receipt,
                });
                consumed = count;
                break;
            }
            cursor += consumed;
        }
    }

    /// Dictionary shape changes are separate from ASR admission. Only settled
    /// committed sources can be merged; the ledger retains their exact PCM and
    /// refuses sealed, stale, or protected targets. No new ASR witness is minted.
    fn apply_lexicon_merges(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        owner: &OccurrenceIdentity,
        request: u64,
    ) {
        self.apply_corroborated_lexicon(ev_tx, owner, request);
        use crate::pipeline::acoustic_ledger::SlotTarget;
        let mut ledger = self
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if ledger.is_sealed(owner) || !ledger.word_labels_settled(owner) {
            return;
        }
        let sources = ledger.slots_of(owner).unwrap_or(&[]).to_vec();
        let (merges, counts) = super::live_lexicon::registered_merges(
            &sources
                .iter()
                .map(|slot| slot.text.as_str())
                .collect::<Vec<_>>(),
            &self.lexicon_custom_path,
        );
        self.lexicon_entries_custom = counts.custom;
        for merge in merges {
            let targets = sources[merge.start..merge.end]
                .iter()
                .map(SlotTarget::from)
                .collect::<Vec<_>>();
            if sources[merge.start..merge.end]
                .iter()
                .any(|source| source.producer == LedgerObservationProducer::ManualHuman)
            {
                continue;
            }
            let observation =
                ledger.next_word_observation(LedgerObservationProducer::Lexicon, request, owner);
            if ledger
                .merge_word_slots(&observation, &targets, &merge.rule)
                .is_err()
            {
                continue;
            }
            let receipt = ledger
                .layer_trail()
                .last()
                .expect("dictionary merge decision")
                .decision
                .clone();
            let label = ledger.text_of(owner).unwrap_or("").to_string();
            self.lexicon_rewrites = self.lexicon_rewrites.saturating_add(1);
            let _ = ev_tx.send(EngineEvent::LedgerMutation {
                observation,
                label,
                receipt,
            });
        }
    }

    fn admit_routed_words_with_decode_window(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        id: u64,
        owner: &OccurrenceIdentity,
        request_window: (u64, Option<(u64, u64)>),
        batch: RoutedWords<'_>,
        producer: LedgerObservationProducer,
    ) -> bool {
        self.admit_routed_words_with_publication(
            ev_tx,
            id,
            owner,
            RoutedDecode {
                request_window,
                publication: None,
            },
            batch,
            producer,
        )
    }

    fn admit_routed_words_with_publication(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        id: u64,
        owner: &OccurrenceIdentity,
        decode: RoutedDecode<'_>,
        batch: RoutedWords<'_>,
        producer: LedgerObservationProducer,
    ) -> bool {
        let (request, decode_window) = decode.request_window;
        let publication = decode.publication;
        let RoutedWords { pins, neighbours } = batch;
        let merge_sources = super::live_lexicon::registered_merges(
            &pins.iter().map(|pin| pin.text.as_str()).collect::<Vec<_>>(),
            &self.lexicon_custom_path,
        )
        .0;
        let words = pins
            .iter()
            .enumerate()
            .map(|(index, pin)| {
                let text = if merge_sources
                    .iter()
                    .any(|merge| merge.start <= index && index < merge.end)
                {
                    pin.text.clone()
                } else {
                    let (text, counts) =
                        super::live_lexicon::rewrite(&pin.text, &self.lexicon_custom_path);
                    self.lexicon_entries_custom = counts.custom;
                    text
                };
                // d5: a lexicon-rewritten surface keeps its acoustic
                // confidence; the span is marked, not dropped.
                let surface_rewritten = text != pin.text;
                if surface_rewritten {
                    self.lexicon_rewrites = self.lexicon_rewrites.saturating_add(1);
                }
                let word = crate::pipeline::acoustic_ledger::WordPin::new(
                    pin.pin.sample_start,
                    pin.pin.sample_end,
                    text,
                );
                let word = match decode_window {
                    Some((start, end)) => word.with_decode_window(start, end),
                    None => word,
                };
                let word = match pin.confidence {
                    Some(confidence) => word.with_confidence(confidence),
                    None => word,
                };
                if surface_rewritten {
                    word.surface_rewritten()
                } else {
                    word
                }
            })
            .collect::<Vec<_>>();
        // A measured group refinement needs the capture's own speech ranges and
        // this batch's neighbour geometry; without them the group is kept.
        let speech = coverage_speech_evidence(self);
        let (observation, receipt, label, child_receipts) = {
            let mut ledger = self
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let observation = ledger.next_word_observation(producer, request, owner);
            let original = pins
                .iter()
                .map(|pin| {
                    let mut word = crate::pipeline::acoustic_ledger::WordPin::new(
                        pin.pin.sample_start,
                        pin.pin.sample_end,
                        &pin.text,
                    );
                    word.confidence = pin.confidence;
                    if let Some((start, end)) = decode_window {
                        word = word.with_decode_window(start, end);
                    }
                    word
                })
                .collect::<Vec<_>>();
            ledger.stage_word_evidence(&observation, &original, None, "unknown");
            if producer == LedgerObservationProducer::AppleDictation {
                ledger.stage_dt_boundary_evidence(
                    &observation,
                    &self.capture_energy,
                    self.audio.session_sample_end(),
                );
            }
            if let Some((payload, band, terminal_tail)) = publication {
                let decode = &payload.identity.range;
                let full = payload
                    .segments
                    .iter()
                    .filter(|segment| {
                        segment.grain == crate::stt::tail_provider::TailSegmentGrain::Word
                    })
                    .map(|segment| {
                        let mut word = crate::pipeline::acoustic_ledger::WordPin::new(
                            segment.range.sample_start,
                            segment.range.sample_end,
                            &segment.text,
                        )
                        .with_decode_window(decode.sample_start, decode.sample_end);
                        word.confidence = segment.confidence;
                        word
                    })
                    .collect::<Vec<_>>();
                ledger.stage_word_publication(&observation, band, terminal_tail, &full);
            }
            ledger.record_speech_evidence(&speech);
            self.record_decode_fence_measurement(
                &mut ledger,
                (producer == LedgerObservationProducer::Whisper)
                    .then_some(decode_window)
                    .flatten(),
            );
            ledger.record_assigned_word_pins(&observation, neighbours);
            let trail_start = ledger.layer_trail().len();
            let receipt = ledger.admit_word_slots(&observation, &words);
            let child_receipts = ledger.layer_trail()[trail_start..]
                .iter()
                .filter(|entry| entry.observation != observation)
                .cloned()
                .collect::<Vec<_>>();
            let label = match &receipt {
                MutationReceipt::KeepVisibleUnanchored { label, .. } => label.clone(),
                _ => ledger.text_of(owner).unwrap_or("").to_string(),
            };
            (observation, receipt, label, child_receipts)
        };
        if let MutationReceipt::KeepVisibleUnanchored { reason, .. } = &receipt {
            let _ = ev_tx.send(EngineEvent::Warning {
                code: reason.as_str().into(),
                message: format!(
                    "owner={}..{} request={request}",
                    owner.sample_start, owner.sample_end
                ),
            });
        }
        for entry in child_receipts {
            let _ = ev_tx.send(EngineEvent::LedgerMutation {
                observation: entry.observation,
                label: entry.candidate_label,
                receipt: entry.decision,
            });
        }
        let admitted = receipt.grants_mutation();
        let _ = ev_tx.send(EngineEvent::LedgerMutation {
            observation,
            label,
            receipt,
        });
        self.adjudicate_current_word_slots(ev_tx, owner, request);
        self.apply_lexicon_merges(ev_tx, owner, request);
        self.refresh_pending_label(id, owner);
        self.refinement_receipt(owner, "completed");
        admitted
    }

    fn settle_whisper_stubs(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        _id: u64,
        owner: &OccurrenceIdentity,
        terminal: bool,
    ) {
        let pending = std::mem::take(&mut self.pending_whisper_stubs);
        for (request_identity, stub_owner, stub) in pending {
            let decode_range = OccurrenceIdentity::from(&request_identity.range);
            if &stub_owner == owner
                && decode_range.same_capture(owner)
                && stub.pin.same_capture(owner)
            {
                let request = request_identity.request_id;
                let superseded = {
                    let mut ledger = self
                        .acoustic_ledger
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let observation = ledger.next_word_observation(
                        LedgerObservationProducer::Whisper,
                        request,
                        owner,
                    );
                    if let Some(receipt) =
                        ledger.refuse_superseded_stub(&observation, &stub.text, &stub.pin)
                    {
                        let _ = ev_tx.send(EngineEvent::LedgerMutation {
                            observation,
                            label: stub.text.clone(),
                            receipt,
                        });
                        true
                    } else {
                        false
                    }
                };
                if superseded {
                    continue;
                }
                if !terminal {
                    self.pending_whisper_stubs
                        .push((request_identity, stub_owner, stub));
                    continue;
                }
                // Terminal drain is not whole-word evidence. Keep the edge
                // hypothesis visible and its owner unresolved without admission.
                self.keep_routed_visible(ev_tx, request, std::slice::from_ref(&stub));
                let (observation, receipt) = {
                    let mut ledger = self
                        .acoustic_ledger
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    ledger.require_text_recovery(owner);
                    let observation = ledger.next_word_observation(
                        LedgerObservationProducer::Whisper,
                        request,
                        owner,
                    );
                    let mut pin = crate::pipeline::acoustic_ledger::WordPin::new(
                        stub.pin.sample_start,
                        stub.pin.sample_end,
                        stub.text.clone(),
                    )
                    .with_decode_window(decode_range.sample_start, decode_range.sample_end);
                    pin.confidence = stub.confidence;
                    ledger.retain_rejected_decode_word(&observation, &pin);
                    let receipt = ledger.refuse_replacement(
                        &observation,
                        &stub.text,
                        RefuseReason::UnrecoveredSpeech,
                    );
                    (observation, receipt)
                };
                let _ = ev_tx.send(EngineEvent::LedgerMutation {
                    observation,
                    label: stub.text.clone(),
                    receipt,
                });
            } else {
                self.pending_whisper_stubs
                    .push((request_identity, stub_owner, stub));
            }
        }
    }

    /// A submitted, queued or still unplanned frame may yet return words for
    /// this owner. A held window is future work even before transport enqueue.
    fn whisper_work_may_cover(&self, owner: &OccurrenceIdentity) -> bool {
        self.refinement_submitted.values().any(|job| {
            job.request_identity.range.sample_start < owner.sample_end
                && job.request_identity.range.sample_end > owner.sample_start
        }) || self.refinement_pending.iter().any(|job| {
            job.provider_request.identity.range.sample_start < owner.sample_end
                && job.provider_request.identity.range.sample_end > owner.sample_start
        }) || self.window_plan.admission_horizon() < owner.sample_end
    }

    /// A matched failure is the last frame some members can ever receive.
    /// Without waiting for Stop, return the reservation of each labelled
    /// member that has no Whisper return and no frame left to cover it, under
    /// the code Stop records for "matched, then refused". The admitted label
    /// stays; blank owners still owe a witness and keep their frontier open.
    fn settle_failed_window_members(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        members: &[(u64, OccurrenceIdentity)],
    ) {
        for (id, owner) in members {
            if self.whisper_work_may_cover(owner) {
                continue;
            }
            let unanswered_label = {
                let ledger = self
                    .acoustic_ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                !ledger.is_sealed(owner)
                    && ledger.frontier_of(owner).is_some_and(|frontier| {
                        frontier
                            .open_producers()
                            .contains(&LedgerObservationProducer::Whisper)
                    })
                    && !ledger
                        .layer_trail_for(owner)
                        .any(|entry| entry.producer() == LedgerObservationProducer::Whisper)
                    && ledger
                        .text_of(owner)
                        .is_some_and(|label| !label.trim().is_empty())
            };
            if unanswered_label {
                self.fail_refinement(ev_tx, *id, owner, RefinementFailure::NotScheduled);
            }
        }
    }

    /// Monotonic evidence about future window starts. A completed newer job
    /// cannot seal ahead of an older submitted or queued job that may own words.
    fn close_admission_horizon(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        sample_start: u64,
    ) {
        if !self.seal_ledger_enabled {
            return;
        }
        let sample_start = sample_start.min(self.window_plan.admission_horizon());
        self.admission_horizon = self.admission_horizon.max(sample_start);
        let owners = self.unsealed_word_owners();
        let mut expired_words = false;
        // Owners whose ledger can still expire a conflict. Unresolved owners
        // accumulate behind the horizon over a long take; every one of them was
        // re-scanned against every component on each worker tick. The set is
        // exact for the ledger it was read from, so it is dropped after any
        // call below that can mutate components.
        let mut open_conflicts: Option<BTreeSet<OccurrenceIdentity>> = None;
        for (id, owner) in owners {
            if owner.sample_end > self.admission_horizon {
                continue;
            }
            if self.whisper_work_may_cover(&owner) {
                continue;
            }
            let (is_open, has_return) = {
                let ledger = self
                    .acoustic_ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                (
                    ledger.frontier_of(&owner).is_some_and(|frontier| {
                        frontier
                            .open_producers()
                            .contains(&LedgerObservationProducer::Whisper)
                    }),
                    ledger
                        .layer_trail_for(&owner)
                        .any(|entry| entry.producer() == LedgerObservationProducer::Whisper),
                )
            };
            if !has_return {
                if is_open && sample_start == u64::MAX {
                    self.fail_refinement(ev_tx, id, &owner, RefinementFailure::NotScheduled);
                    open_conflicts = None;
                }
                continue;
            }
            // With all covering planned work returned, the ledger records any
            // remaining local uncertainty without inventing a new witness.
            {
                let mut ledger = self
                    .acoustic_ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let open = open_conflicts
                    .get_or_insert_with(|| ledger.owners_with_unattempted_conflicts())
                    .contains(&owner);
                let expired = if open {
                    open_conflicts = None;
                    ledger.close_word_adjudication_horizon(&owner)
                } else {
                    0
                };
                drop(ledger);
                expired_words |= expired > 0;
                if expired > 0 {
                    info!(
                        sample_start = owner.sample_start,
                        sample_end = owner.sample_end,
                        words_expired = expired,
                        "word_adjudication_horizon_closed"
                    );
                }
            }
            // A return parked behind the in-flight witness is already settled;
            // the witness answer resumes it. Returning it again on every tick
            // re-logged it thousands of times per second until Stop.
            if !is_open || self.dictation_witness_waiting.contains(&owner) {
                continue;
            }
            self.refinement_receipt(&owner, "admission_horizon_closed");
            self.return_whisper_without_label(ev_tx, id, &owner);
            self.emit_pending_seal(ev_tx, id);
            open_conflicts = None;
        }
        if expired_words {
            self.publish_resolved_word_seals(ev_tx);
        }
    }

    fn finish_whisper_frontier(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        occurrence: &OccurrenceIdentity,
    ) {
        let captured_end = self.audio.session_sample_end();
        maybe_spawn_utterance_witness(self, ev_tx, captured_end);
        if self.dictation_witness_inflight
            && !self.dictation_witness_owners.contains(occurrence)
            && !self.capture_stopping
        {
            if !self.dictation_witness_waiting.contains(occurrence) {
                self.dictation_witness_waiting.push(occurrence.clone());
            }
            return;
        }
        let mut ledger = self
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let closed = ledger.note_frontier_return(occurrence, LedgerObservationProducer::Whisper);
        if closed && let Ok(receipt) = ledger.seal(occurrence).cloned() {
            let _ = ev_tx.send(EngineEvent::LedgerSeal { receipt });
        }
        schedule_formatter_for_sealed_label(
            &ledger,
            self.formatter.as_ref(),
            &mut self.formatter_requested,
            occurrence,
        );
    }

    /// Return one launched Whisper slot with an explicit no-label receipt.
    fn return_whisper_without_label(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        utterance_id: u64,
        occurrence: &OccurrenceIdentity,
    ) {
        self.settle_whisper_stubs(ev_tx, utterance_id, occurrence, true);
        let mut ledger = self
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !ledger.frontier_of(occurrence).is_some_and(|frontier| {
            frontier
                .open_producers()
                .contains(&LedgerObservationProducer::Whisper)
        }) {
            return;
        }
        if ledger.text_of(occurrence).is_none()
            && !ledger
                .layer_trail_for(occurrence)
                .any(|entry| entry.producer() == LedgerObservationProducer::Whisper)
        {
            let observation = ledger.next_word_observation(
                LedgerObservationProducer::Whisper,
                utterance_id,
                occurrence,
            );
            let receipt = ledger.admit(&observation, "");
            let _ = ev_tx.send(EngineEvent::LedgerMutation {
                observation,
                label: String::new(),
                receipt,
            });
        }
        drop(ledger);
        self.finish_whisper_frontier(ev_tx, occurrence);
    }

    /// Return every still-open launched Whisper slot after the bounded stop
    /// drain expires. This closes only producer obligations; it admits no text.
    fn return_outstanding_whisper_without_label(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    ) {
        let occurrences = {
            let ledger = self
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ledger
                .qualified_occurrences()
                .filter(|occurrence| {
                    ledger.frontier_of(occurrence).is_some_and(|frontier| {
                        frontier
                            .open_producers()
                            .contains(&LedgerObservationProducer::Whisper)
                    })
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        for occurrence in occurrences {
            let id = self
                .pending_events
                .iter()
                .find_map(|(id, pending)| (pending.occurrence == occurrence).then_some(*id))
                .unwrap_or(0);
            let has_return = self
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .layer_trail_for(&occurrence)
                .any(|entry| entry.producer() == LedgerObservationProducer::Whisper);
            let has_work = self.window_plan.admission_horizon() < occurrence.sample_end
                || self.refinement_pending.iter().any(|job| {
                    job.member_occurrences
                        .iter()
                        .any(|(_, owner)| owner == &occurrence)
                })
                || self.refinement_submitted.values().any(|job| {
                    job.member_occurrences
                        .iter()
                        .any(|(_, owner)| owner == &occurrence)
                });
            if !has_return || has_work {
                self.fail_refinement(ev_tx, id, &occurrence, RefinementFailure::StopDeadline);
            }
        }
        for job in self.refinement_submitted.values() {
            self.window_plan_receipt(&job.request_identity.range, "expired_after_acceptance");
        }
        self.refinement_pending.clear();
        self.refinement_submitted.clear();
        while let Some(range) = self
            .window_plan
            .next_due(self.audio.session_sample_end(), self.capture_stopping)
        {
            self.window_plan_receipt(&range, "expired_before_acceptance");
            self.window_plan.account(&range);
        }
        self.close_admission_horizon(ev_tx, u64::MAX);
    }

    fn refuse_tail_patch(
        &mut self,
        ev_tx: &mpsc::UnboundedSender<EngineEvent>,
        code: &str,
        utterance_id: u64,
        reason: &str,
    ) {
        self.tail_patch_refusals = self.tail_patch_refusals.saturating_add(1);
        let _ = ev_tx.send(EngineEvent::Warning {
            code: code.to_string(),
            message: format!("utterance {utterance_id}: {reason}; Apple text preserved"),
        });
    }

    /// End-of-session drain for retained reducer inputs. AcousticLedger owns
    /// finality; this method only forwards the already admitted Apple payload.
    fn seal_remaining_at_session_end(&mut self, ev_tx: &mpsc::UnboundedSender<EngineEvent>) {
        let pending_ids: Vec<u64> = self.pending_events.keys().copied().collect();
        for utterance_id in pending_ids {
            self.emit_pending_seal(ev_tx, utterance_id);
        }
    }

    fn emit_pending_seal(&mut self, ev_tx: &mpsc::UnboundedSender<EngineEvent>, utterance_id: u64) {
        let ready = self
            .pending_events
            .get(&utterance_id)
            .is_some_and(|pending| {
                self.acoustic_ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .is_sealed(&pending.occurrence)
            });
        if !ready {
            return;
        }
        let Some(pending) = self.pending_events.remove(&utterance_id) else {
            return;
        };
        self.sealed_count = self.sealed_count.saturating_add(1);
        if !self.sealed_prefix.is_empty() {
            self.sealed_prefix.push(' ');
        }
        self.sealed_prefix.push_str(&pending.layer1_baseline);
        let _ = ev_tx.send(EngineEvent::UtteranceFinal {
            utterance_id,
            text: pending.layer1_baseline,
            raw_text: pending.raw_text,
            start_ts: pending.start_ts,
            end_ts: pending.end_ts,
            segments: pending.segments,
            vad_speech_pct: None,
            avg_logprob: None,
            compression_ratio: None,
            confidence_flags: Vec::new(),
        });
    }
}

/// What the worker sealed, and what seal-time postprocess filtered away.
struct AppleStreamOutcome {
    sealed: u64,
    filtered_empty_drops: u64,
    lexicon_rewrites: u64,
    lexicon_entries_custom: usize,
    unresolved_windows: u64,
    /// How many seals escalated an unplaceable Layer 1 under-commit (W-C).
    under_commit_escalations: u64,
    /// Bounded Layer 1 events that crossed the rewrite fence before seal.
    tail_patch_replacements: u64,
    /// Completions refused by identity, replay, range, or sealed-fence checks.
    tail_patch_refusals: u64,
    /// Job-level outcome after the single rewrite fence adjudicated it.
    tail_patch_jobs_applied: u64,
    /// Completed jobs with no accepted mutation, including no-change.
    tail_patch_jobs_skipped: u64,
    /// Jobs still outstanding when the bounded closure wait expired.
    tail_patch_timeout_residue: u64,
    /// Ledger and window census read at worker exit. Not derived from job buckets.
    conservation: SessionConservationReceipt,
}

/// Clamp an observed timestamp to the captured PCM timeline.
fn seconds_to_captured_sample(seconds: f32, sample_rate: u32, captured_end: u64) -> u64 {
    if !seconds.is_finite() || seconds <= 0.0 {
        return 0;
    }
    ((seconds as f64 * sample_rate.max(1) as f64).round() as u64).min(captured_end)
}

#[cfg(any())]
fn timed_words_to_segments(words: &[TimedTailSegment], sample_rate: u32) -> Vec<TranscriptSegment> {
    let rate = sample_rate.max(1) as f32;
    words
        .iter()
        .filter(|word| {
            word.range.sample_end > word.range.sample_start && !word.text.trim().is_empty()
        })
        .map(|word| TranscriptSegment {
            confidence: None,
            text: word.text.clone(),
            start_ts: word.range.sample_start as f32 / rate,
            end_ts: word.range.sample_end as f32 / rate,
        })
        .collect()
}

fn apple_segments_on_pcm_clock(
    state: &AppleSealState,
    segments: &[TranscriptSegment],
) -> Vec<TimedTailSegment> {
    let captured_end = state.audio.session_sample_end();
    segments
        .iter()
        .map(|segment| {
            let sample_start =
                seconds_to_captured_sample(segment.start_ts, state.sample_rate, captured_end);
            let sample_end =
                seconds_to_captured_sample(segment.end_ts, state.sample_rate, captured_end)
                    .max(sample_start);
            TimedTailSegment {
                grain: crate::stt::tail_provider::TailSegmentGrain::Phrase,
                text: segment.text.clone(),
                range: TailSampleRange {
                    session: state.session_id.clone(),
                    capture_epoch: state.capture_epoch,
                    sample_start,
                    sample_end,
                },
                confidence: segment.confidence,
            }
        })
        .collect()
}

/// Extra copies of a canvas token are new acoustic occurrences, not revisions.
#[cfg(any())]
fn cap_known_prefix_to_canvas_token_counts(probe: &[String], canvas: &[&str], k: usize) -> usize {
    let mut canvas_counts = std::collections::HashMap::<&str, usize>::new();
    for word in canvas {
        *canvas_counts.entry(*word).or_insert(0) += 1;
    }
    let mut used = std::collections::HashMap::<&str, usize>::new();
    let k = k.min(probe.len());
    for (index, token) in probe[..k].iter().enumerate() {
        let Some(&canvas_n) = canvas_counts
            .get(token.as_str())
            .filter(|count| **count > 0)
        else {
            continue;
        };
        let used_n = used.entry(token.as_str()).or_insert(0);
        *used_n += 1;
        if *used_n > canvas_n {
            return index;
        }
    }
    k
}

/// Slice a cumulative Apple final onto Silero-minted utterance ranges.
///
/// Returns `true` when at least one Silero span accepted words (the callback
/// is consumed). `false` leaves the caller on the Apple-boundary path so
/// speech is never dropped when Silero has not yet opened an edge.
fn seal_sliced_by_silero(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    disjoint: &[TranscriptSegment],
) -> bool {
    if !state.seal_ledger_enabled {
        // Cloud still consumes the same Silero silence decisions. A transport
        // commit needs no acoustic occurrence or admitted source-language text.
        if state.cloud_commit_tx.is_some() {
            // Commits are cumulative capture ranges; the latest measured
            // silence fence subsumes earlier closes without cloning a history.
            let close = state.fusion.as_ref().and_then(|fusion| {
                fusion
                    .ledger()
                    .utterances()
                    .iter()
                    .rev()
                    .filter_map(|utterance| fusion.ledger().close_receipt(utterance.id))
                    .find(|receipt| {
                        receipt.cause == super::silero_fusion::UtteranceCloseCause::SilenceFence
                    })
            });
            if let Some(receipt) = close {
                state.commit_cloud_close(ev_tx, receipt);
            }
        }
        return false;
    }
    let Some(fusion) = state.fusion.as_ref() else {
        return false;
    };
    let utterances = fusion.ledger().utterances();
    // Only an identical, already reconciled callback is silent. A new Apple
    // observation over consumed audio must still publish its refusal receipt.
    if !disjoint.is_empty()
        && state.silero_slice_words.len() == disjoint.len()
        && state
            .silero_slice_words
            .iter()
            .zip(disjoint)
            .all(|(prior, current)| {
                prior.text == current.text
                    && prior.start_ts == current.start_ts
                    && prior.end_ts == current.end_ts
            })
        && utterances
            .iter()
            .all(|utterance| utterance.closed && state.reconciled_silero.contains(&utterance.id))
        && state
            .silero_slice_revision
            .as_ref()
            .is_some_and(|(len, last)| {
                *len == utterances.len() && last.as_ref() == utterances.last()
            })
    {
        return true;
    }
    if disjoint.is_empty()
        && state
            .silero_slice_revision
            .as_ref()
            .is_some_and(|(len, last)| {
                *len == utterances.len() && last.as_ref() == utterances.last()
            })
    {
        return true;
    }
    // Check before cloning the ledger or converting/copying retained words.
    // A newly opened, extended or closed range must still retry the leftovers
    // and reconcile pending words, including the terminal flush.
    state.silero_slice_revision = Some((utterances.len(), utterances.last().cloned()));
    state.silero_slice_words = disjoint.to_vec();
    let ledger = fusion.ledger().clone();
    let consumed = reconcile_silero_ledger(state, ev_tx, &ledger, disjoint);
    state.publish_unadmitted_words(ev_tx);
    consumed
}

/// PCM each closed utterance owns once boundary overlaps are given to the
/// tightest window. Context pads stay on the Silero range; they are not a
/// second physical identity over the same samples.
fn exclusive_closed_spans(
    utterances: &[super::silero_fusion::SileroUtterance],
) -> BTreeMap<u64, (u64, u64)> {
    let closed: Vec<_> = utterances
        .iter()
        .filter(|utterance| utterance.closed)
        .collect();
    let mut points = Vec::with_capacity(closed.len().saturating_mul(2));
    for utterance in &closed {
        points.push(utterance.range.sample_start);
        points.push(utterance.range.sample_end);
    }
    points.sort_unstable();
    points.dedup();
    let mut pieces: BTreeMap<u64, Vec<(u64, u64)>> = BTreeMap::new();
    for pair in points.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        if end <= start {
            continue;
        }
        let owner = closed
            .iter()
            .filter(|utterance| {
                utterance.range.sample_start <= start && end <= utterance.range.sample_end
            })
            .min_by_key(|utterance| {
                (
                    utterance
                        .range
                        .sample_end
                        .saturating_sub(utterance.range.sample_start),
                    utterance.range.sample_start,
                    utterance.id,
                )
            });
        if let Some(owner) = owner {
            pieces.entry(owner.id).or_default().push((start, end));
        }
    }
    let mut spans = BTreeMap::new();
    for utterance in &closed {
        let Some(parts) = pieces.get(&utterance.id) else {
            continue;
        };
        let start = parts[0].0;
        let mut end = parts[0].1;
        let mut contiguous = true;
        for (part_start, part_end) in parts.iter().skip(1) {
            if *part_start != end {
                contiguous = false;
                break;
            }
            end = *part_end;
        }
        if contiguous && end > start {
            spans.insert(utterance.id, (start, end));
        } else {
            spans.insert(
                utterance.id,
                (utterance.range.sample_start, utterance.range.sample_end),
            );
        }
    }
    spans
}

/// Accounting only: the ledger remains the authority for every slot decision.
#[derive(Default)]
struct LateAppleAdmission {
    admitted_into: BTreeMap<OccurrenceIdentity, usize>,
    dropped_by_slot_rules: usize,
    unmatched: usize,
    kept_unanchored: usize,
}

fn retain_apple_words_at_exit(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    utterance_id: u64,
    words: &[FusionWord],
    current_slice: &[TranscriptSegment],
    exit: &str,
) {
    let outcome = admit_late_apple_words(state, ev_tx, utterance_id, words, current_slice);
    let admitted_into = outcome
        .admitted_into
        .into_iter()
        .map(|(occurrence, count)| {
            serde_json::json!({
                "occurrence": {
                    "session": occurrence.session,
                    "capture_epoch": occurrence.capture_epoch,
                    "sample_start": occurrence.sample_start,
                    "sample_end": occurrence.sample_end,
                },
                "count": count,
            })
        })
        .collect::<Vec<_>>();
    let _ = ev_tx.send(EngineEvent::Warning {
        code: "apple_words_retained".into(),
        message: serde_json::json!({
            "exit": exit,
            "utterance_id": utterance_id,
            "words": words.len(),
            "admitted_into": admitted_into,
            "dropped_by_slot_rules": outcome.dropped_by_slot_rules,
            "unmatched": outcome.unmatched,
            "kept_unanchored": outcome.kept_unanchored,
        })
        .to_string(),
    });
}

/// Production reconciliation seam; tests supply physical edges without a model.
/// Late Apple labels use the same PCM owners and slot store as live Whisper.
/// Reconciled means identity has been assigned; it never means words may vanish.
fn admit_late_apple_words(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    request: u64,
    words: &[FusionWord],
    current_slice: &[TranscriptSegment],
) -> LateAppleAdmission {
    let mut outcome = LateAppleAdmission::default();
    // Every reconciled Silero utterance comes back here on each worker tick,
    // almost always with nothing late. Without words there is no owner to
    // route to and nothing to admit, so the owner lookup is skipped.
    if words.is_empty() {
        return outcome;
    }
    let owners = state.word_owners_overlapping(
        words
            .iter()
            .map(|word| word.sample_start)
            .min()
            .unwrap_or(0),
        words.iter().map(|word| word.sample_end).max().unwrap_or(0),
    );
    let owner_ranges = owners
        .iter()
        .map(|(_, owner)| owner.clone())
        .collect::<Vec<_>>();
    let mut by_owner: BTreeMap<OccurrenceIdentity, Vec<FusionWord>> = BTreeMap::new();
    for word in words {
        let pin = OccurrenceIdentity::new(
            state.session_id.clone(),
            state.capture_epoch,
            word.sample_start,
            word.sample_end,
        );
        let owner_index = state
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .word_owner_index(&pin, &owner_ranges);
        match owner_index {
            Some(index) => by_owner
                .entry(owner_ranges[index].clone())
                .or_default()
                .push(word.clone()),
            _ => {
                state.unmatched_silero_words.push(word.clone());
                outcome.unmatched += 1;
            }
        }
    }
    for (owner, words) in by_owner {
        let (selected, superseded) = select_current_apple_words(words);
        outcome.dropped_by_slot_rules += superseded.len();
        for word in &superseded {
            retain_superseded_apple_word(state, ev_tx, request, word);
        }
        let words = selected
            .into_iter()
            .map(|word| {
                (
                    word.sample_start,
                    word.sample_end,
                    word.text,
                    word.confidence,
                )
            })
            .collect::<Vec<_>>();
        let mut ledger = state
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.bind_apple_word_measurement(&mut ledger, &owner);
        for (start, end, text, confidence) in words {
            let pin =
                OccurrenceIdentity::new(owner.session.clone(), owner.capture_epoch, start, end);
            let observation =
                ledger.next_word_observation(LedgerObservationProducer::Apple, request, &owner);
            let exact_current = start < end
                && owner.sample_start <= start
                && end <= owner.sample_end
                && current_slice.iter().any(|segment| {
                    let rate = f64::from(state.sample_rate.max(1));
                    let current_start = (f64::from(segment.start_ts) * rate).round();
                    let current_end = (f64::from(segment.end_ts) * rate).round();
                    segment.start_ts.is_finite()
                        && segment.end_ts.is_finite()
                        && segment.start_ts >= 0.0
                        && segment.end_ts > segment.start_ts
                        && current_end <= state.audio.session_sample_end() as f64
                        && current_start as u64 == start
                        && current_end as u64 == end
                        && segment.text == text
                });
            if !exact_current {
                outcome.kept_unanchored += 1;
                // Evidence names the word's PCM, not its containing owner.
                // This records an observation without minting an occurrence.
                let observation = LedgerObservationIdentity {
                    occurrence: pin,
                    ..observation
                };
                let receipt = ledger.keep_visible_unanchored(
                    &observation,
                    &text,
                    NoAuthorityReason::LateAppleWordNotCurrent,
                );
                let _ = ev_tx.send(EngineEvent::LedgerMutation {
                    observation,
                    label: text,
                    receipt,
                });
                continue;
            }
            let word = crate::pipeline::acoustic_ledger::WordPin::new(start, end, text);
            let word = match confidence {
                Some(confidence) => word.with_confidence(confidence),
                None => word,
            };
            let candidate = word.text.clone();
            let receipt = ledger.admit_word_slots(&observation, &[word]);
            let admitted = ledger
                .slots_of(&owner)
                .unwrap_or(&[])
                .iter()
                .filter(|slot| slot.observation == observation)
                .count();
            if admitted > 0 {
                *outcome.admitted_into.entry(owner.clone()).or_default() += admitted;
            }
            if matches!(&receipt, MutationReceipt::KeepVisibleUnanchored { .. }) {
                outcome.kept_unanchored += 1;
            } else {
                outcome.dropped_by_slot_rules += 1usize.saturating_sub(admitted);
            }
            let label = match &receipt {
                MutationReceipt::KeepVisibleUnanchored { label, reason, .. } => {
                    let _ = ev_tx.send(EngineEvent::Warning {
                        code: reason.as_str().into(),
                        message: format!(
                            "owner={}..{} request={request}",
                            owner.sample_start, owner.sample_end
                        ),
                    });
                    label.clone()
                }
                MutationReceipt::Refuse { .. } => candidate,
                _ => ledger.text_of(&owner).unwrap_or("").to_string(),
            };
            let _ = ev_tx.send(EngineEvent::LedgerMutation {
                observation,
                label,
                receipt,
            });
        }
        drop(ledger);
        if let Some((id, _)) = owners.iter().find(|(_, range)| range == &owner) {
            state.refresh_pending_label(*id, &owner);
        }
    }
    outcome
}

/// Arrival order chooses the current Apple hypothesis for one PCM slot.
/// Only exact native callback coordinates identify a revised transport row.
/// Physical word identity is decided later by the ledger's speech anchors.
/// even when timestamp edges cross.
fn select_current_apple_words(words: Vec<FusionWord>) -> (Vec<FusionWord>, Vec<FusionWord>) {
    let mut selected: Vec<FusionWord> = Vec::new();
    let mut superseded = Vec::new();
    for word in words {
        if let Some(index) = selected.iter().position(|held| {
            held.sample_start == word.sample_start && held.sample_end == word.sample_end
        }) {
            if selected[index] == word {
                continue;
            }
            superseded.push(std::mem::replace(&mut selected[index], word));
        } else {
            selected.push(word);
        }
    }
    (selected, superseded)
}

fn retain_superseded_apple_word(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    request: u64,
    word: &FusionWord,
) {
    let pin = OccurrenceIdentity::new(
        state.session_id.clone(),
        state.capture_epoch,
        word.sample_start,
        word.sample_end,
    );
    let (observation, receipt) = {
        let mut ledger = state
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let observation =
            ledger.next_word_observation(LedgerObservationProducer::Apple, request, &pin);
        let receipt = ledger.keep_visible_unanchored(
            &observation,
            &word.text,
            NoAuthorityReason::LateAppleWordNotCurrent,
        );
        (observation, receipt)
    };
    let _ = ev_tx.send(EngineEvent::LedgerMutation {
        observation,
        label: word.text.clone(),
        receipt,
    });
}

fn reconcile_silero_ledger(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    ledger: &super::silero_fusion::UtteranceLedger,
    disjoint: &[TranscriptSegment],
) -> bool {
    if ledger.utterances().iter().any(|utterance| {
        utterance.range.session != state.session_id
            || utterance.range.capture_epoch != state.capture_epoch
    }) {
        let _ = ev_tx.send(EngineEvent::Warning {
            code: RefinementFailure::InvalidIdentity.code().into(),
            message: "Silero evidence belongs to another capture; current occurrence unchanged"
                .into(),
        });
        return false;
    }
    let ledger = ledger_with_voiced_onsets(state, ledger);
    let ledger = &ledger;
    let apple_words = apple_segments_on_pcm_clock(state, disjoint);
    let mut fusion_words = std::mem::take(&mut state.unmatched_silero_words);
    fusion_words.extend(apple_words.iter().map(FusionWord::from_timed));
    // Re-delivery of one timed word is one entry. Preserve arrival order and
    // every distinct pin or text, including equal text on different PCM.
    let mut seen_fusion_words = BTreeSet::new();
    fusion_words.retain(|word| {
        seen_fusion_words.insert((word.sample_start, word.sample_end, word.text.clone()))
    });
    let (sliced, leftover) = slice_apple_words(ledger, &fusion_words);
    let mut newly_unmatched = 0;
    for word in &leftover {
        if state
            .warned_unmatched_words
            .insert((word.sample_start, word.sample_end))
        {
            newly_unmatched += 1;
        }
    }
    if newly_unmatched > 0 {
        state.no_time_overlap_warnings += 1;
        let _ = ev_tx.send(EngineEvent::Warning {
            code: SkipReasonCode::NoTimeOverlap.as_str().to_string(),
            message: format!(
                "apple leftover_words={} new_unmatched_words={} sliced_utterances={}",
                leftover.len(),
                newly_unmatched,
                sliced.len()
            ),
        });
    }
    state.unmatched_silero_words = leftover;

    let rate = state.sample_rate.max(1) as f32;
    // Candidate labels remain paint until the physical extent closes. Retain
    // every observation until reconciliation; equality of text is irrelevant.
    for (id, words) in sliced {
        state
            .pending_silero_words
            .entry(id)
            .or_default()
            .extend(words);
    }
    // Overlapping Silero windows are one physical claim per sample. Words move
    // with their midpoint onto the tightest closed span before any occurrence
    // is minted, so a pad shared with a neighbour cannot erase the word.
    let exclusive = exclusive_closed_spans(ledger.utterances());
    if !exclusive.is_empty() {
        let mut held = Vec::new();
        let closed_ids: Vec<u64> = ledger
            .utterances()
            .iter()
            .filter(|utterance| utterance.closed)
            .map(|utterance| utterance.id)
            .collect();
        for id in closed_ids {
            if let Some(words) = state.pending_silero_words.remove(&id) {
                held.extend(words);
            }
        }
        for word in held {
            let mid = word.sample_start + word.sample_end.saturating_sub(word.sample_start) / 2;
            let owner = exclusive
                .iter()
                .find_map(|(&id, &(start, end))| (start <= mid && mid < end).then_some(id));
            match owner {
                Some(id) => {
                    state.pending_silero_words.entry(id).or_default().push(word);
                }
                None => state.unmatched_silero_words.push(word),
            }
        }
    }
    for silero in ledger
        .utterances()
        .iter()
        .filter(|utterance| utterance.closed)
    {
        let utterance_id = silero.id;
        if state.reconciled_silero.contains(&utterance_id) {
            let late = state
                .pending_silero_words
                .remove(&utterance_id)
                .unwrap_or_default();
            admit_late_apple_words(state, ev_tx, utterance_id, &late, disjoint);
            continue;
        }
        let candidates = state
            .pending_silero_words
            .remove(&utterance_id)
            .unwrap_or_default();
        let (words, superseded) = select_current_apple_words(candidates);
        for word in &superseded {
            retain_superseded_apple_word(state, ev_tx, utterance_id, word);
        }
        let ambiguous = words
            .windows(2)
            .any(|pair| pair[0].sample_end > pair[1].sample_start);
        if ambiguous {
            let _ = ev_tx.send(EngineEvent::Warning {
                code: "apple_closed_occurrence_ambiguous_word_ranges".into(),
                message: format!("utterance={utterance_id} requires fresh exact-PCM evidence"),
            });
        }
        let text = words
            .iter()
            .map(|word| word.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        // Blank Apple evidence neither consumes this identity nor blocks its PCM.
        let has_apple_label = !text.trim().is_empty();
        if !has_apple_label
            && state.tail_patch.is_none()
            && !state.refinement_lane_lost
            && state.cloud_commit_tx.is_none()
        {
            // An explicitly Apple-only take has no recovery observer to launch.
            // Leave the occurrence unlabelled until Apple actually observes it;
            // absent text remains uncovered rather than an invented empty seal.
            continue;
        }
        let span_start = words
            .first()
            .map(|word| word.sample_start as f32 / rate)
            .unwrap_or(silero.range.sample_start as f32 / rate);
        let span_end = words
            .last()
            .map(|word| word.sample_end as f32 / rate)
            .unwrap_or(silero.range.sample_end as f32 / rate);
        let slice_segments = words
            .iter()
            .map(|word| TranscriptSegment {
                text: word.text.clone(),
                start_ts: word.sample_start as f32 / rate,
                end_ts: word.sample_end as f32 / rate,
                confidence: word.confidence,
            })
            .collect::<Vec<_>>();

        // Silero has already selected the physical occurrence. The identity is
        // the exclusive PCM, not the pad it shares with a neighbour. Admit the
        // slice-local Apple label before the raw final can escape as telemetry.
        // Lexicon rewrites that exact slice in its own ledger observation;
        // callback-wide text is never copied across sliced occurrences.
        let (owned_start, owned_end) = match exclusive.get(&utterance_id) {
            Some(&(start, end)) if end > start => (start, end),
            _ => {
                let swallowed = exclusive.iter().any(|(&id, &(start, end))| {
                    id != utterance_id
                        && start <= silero.range.sample_start
                        && silero.range.sample_end <= end
                });
                if swallowed {
                    let _ = ev_tx.send(EngineEvent::Warning {
                        code: "silero_occurrence_overlap_refused".into(),
                        message: format!(
                            "utterance={utterance_id} range=[{}..{}] has no exclusive PCM after overlap",
                            silero.range.sample_start, silero.range.sample_end
                        ),
                    });
                    retain_apple_words_at_exit(
                        state,
                        ev_tx,
                        utterance_id,
                        &words,
                        disjoint,
                        "overlap_refused",
                    );
                    state.reconciled_silero.insert(utterance_id);
                    continue;
                }
                (silero.range.sample_start, silero.range.sample_end)
            }
        };
        // A contained re-partition is already owned, including sealed speech.
        // This decision cannot mint identity or plan another job.
        let contained_owner = {
            let ledger = state
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ledger.qualified_occurrences().any(|owner| {
                owner.session == state.session_id
                    && owner.capture_epoch == state.capture_epoch
                    && owner.sample_start <= owned_start
                    && owned_end <= owner.sample_end
                    && (owned_start, owned_end) != (owner.sample_start, owner.sample_end)
            })
        };
        if contained_owner {
            admit_late_apple_words(state, ev_tx, utterance_id, &words, disjoint);
            state.reconciled_silero.insert(utterance_id);
            continue;
        }
        let occurrence = OccurrenceIdentity::new(
            state.session_id.clone(),
            state.capture_epoch,
            owned_start,
            owned_end,
        );
        let first_attempt = state
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .frontier_of(&occurrence)
            .is_none();
        if first_attempt {
            state.refinement_receipt(&occurrence, "closed");
        }
        if !qualify_owned_occurrence(state, &occurrence) {
            let reason = if state
                .window_by_samples(occurrence.sample_start, occurrence.sample_end)
                .is_none()
            {
                RefinementFailure::PcmUnavailable
            } else {
                RefinementFailure::QualificationRefused
            };
            state.fail_refinement(ev_tx, utterance_id, &occurrence, reason);
            let exit = match reason {
                RefinementFailure::PcmUnavailable => "qualification_failed_pcm_unavailable",
                _ => "qualification_failed_refused",
            };
            retain_apple_words_at_exit(state, ev_tx, utterance_id, &words, disjoint, exit);
            state.reconciled_silero.insert(utterance_id);
            continue;
        }
        if !has_apple_label && !first_attempt {
            continue;
        }
        let already_labelled = state
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .text_of(&occurrence)
            .is_some();
        if !first_attempt && already_labelled {
            admit_late_apple_words(state, ev_tx, utterance_id, &words, disjoint);
            state.reconciled_silero.insert(utterance_id);
            continue;
        }
        let owes_recovery = !has_apple_label
            || state.fusion.as_ref().is_some_and(|fusion| {
                state.speech_progress.occurrence_has_debt(
                    &occurrence,
                    &fusion.acoustic_speech_evidence(),
                    state.sample_rate,
                )
            });
        if owes_recovery {
            state
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .require_text_recovery(&occurrence);
        }
        if has_apple_label {
            let mut ledger = state
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if ledger.is_sealed(&occurrence) {
                // Successful seals stay immutable; retain the refusal evidence.
                let observation = LedgerObservationIdentity::new(
                    LedgerObservationProducer::Apple,
                    utterance_id,
                    0,
                    occurrence.clone(),
                );
                let receipt = ledger.admit(&observation, &text);
                let _ = ev_tx.send(EngineEvent::LedgerMutation {
                    observation,
                    label: text.clone(),
                    receipt,
                });
                drop(ledger);
                state.reconciled_silero.insert(utterance_id);
                continue;
            }
            // A completed Whisper job without a label did not seal lexical
            // truth. Extend its accounting only with the real words now held;
            // the ledger preserves prior returns and rejects producer replay.
            let observation = LedgerObservationIdentity::new(
                LedgerObservationProducer::Apple,
                utterance_id,
                0,
                occurrence.clone(),
            );
            if !ledger.schedule_late_apple_label(&observation, &text)
                && ledger.frontier_of(&occurrence).is_some()
            {
                ledger.schedule_observer(occurrence.clone(), LedgerObservationProducer::Apple);
                ledger.schedule_observer(occurrence.clone(), LedgerObservationProducer::Lexicon);
            }
            state.reconciled_silero.insert(utterance_id);
        }
        let word_ranges = if ambiguous {
            Vec::new()
        } else {
            words
                .iter()
                .map(|word| {
                    let pin = crate::pipeline::acoustic_ledger::WordPin::new(
                        word.sample_start,
                        word.sample_end,
                        word.text.clone(),
                    );
                    match word.confidence {
                        Some(confidence) => pin.with_confidence(confidence),
                        None => pin,
                    }
                })
                .collect()
        };
        let apple_admitted = if has_apple_label {
            admit_ledger_label(
                state,
                ev_tx,
                RangedLabelAdmission {
                    admission: LabelAdmission {
                        observation: LedgerObservationIdentity::new(
                            LedgerObservationProducer::Apple,
                            utterance_id,
                            0,
                            occurrence.clone(),
                        ),
                        label: &text,
                        energy: EnergyAdmission::RequireExistingQualification,
                    },
                    words: &word_ranges,
                },
            )
        } else {
            None
        };
        state.pending_events.insert(
            utterance_id,
            PendingAppleSeal {
                occurrence: occurrence.clone(),
                raw_text: text.clone(),
                layer1_baseline: text.clone(),
                start_ts: span_start,
                end_ts: span_end,
                segments: slice_segments,
            },
        );

        state.track_cloud_occurrence(&occurrence);
        state.register_whisper_owner(ev_tx, utterance_id, &occurrence);
        if apple_admitted.is_some() {
            let _ = admit_ledger_label(
                state,
                ev_tx,
                LabelAdmission {
                    observation: LedgerObservationIdentity::new(
                        LedgerObservationProducer::Lexicon,
                        utterance_id,
                        0,
                        occurrence.clone(),
                    ),
                    label: &text,
                    energy: EnergyAdmission::RequireExistingQualification,
                },
            );
        }
        state.emit_pending_seal(ev_tx, utterance_id);
        state.utterance_id = state.utterance_id.max(utterance_id);
    }
    for utterance in ledger.utterances() {
        if let Some(receipt) = ledger.close_receipt(utterance.id) {
            state.commit_cloud_close(ev_tx, receipt);
        }
    }
    true
}

/// Whether one label admission carries the right to establish this
/// occurrence's acoustic qualification from the PCM window it owns.
///
/// Only an observer that owns the capture window may measure it. A later
/// observer of the same occurrence rides the existing qualification: it never
/// re-measures energy the ledger already judged, and never invents evidence for
/// a window the ledger refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EnergyAdmission {
    /// Measure the owned capture window and qualify it through the calibration.
    QualifyFromOwnedPcm,
    /// A terminal final-pass job owns an uncovered measured speech range. It
    /// qualifies that PCM and schedules only the observer whose evidence is
    /// being admitted; Apple/Lexicon never observed this occurrence.
    QualifyFinalPassGap,
    /// Refuse unless the ledger already qualified this exact occurrence.
    RequireExistingQualification,
}

/// One occurrence-authenticated label admission.
///
/// `observation` is the whole authority: the producing engine, its
/// request/generation ordinals, and the exact `(session, capture_epoch,
/// sample_start, sample_end)` occurrence being described. Grouping them in one
/// value is not cosmetic — it makes it unrepresentable to admit a label under an
/// observation identity that names a different occurrence than the one whose
/// energy was qualified. `label` is payload and never an identity key.
struct LabelAdmission<'a> {
    observation: LedgerObservationIdentity,
    label: &'a str,
    energy: EnergyAdmission,
}

/// Optional word timing carried into the same locked admission and publication.
/// Label-only producers supply no ranges and retain one occurrence-wide slot.
struct RangedLabelAdmission<'a> {
    admission: LabelAdmission<'a>,
    words: &'a [crate::pipeline::acoustic_ledger::WordPin],
}

impl<'a> From<LabelAdmission<'a>> for RangedLabelAdmission<'a> {
    fn from(admission: LabelAdmission<'a>) -> Self {
        Self {
            admission,
            words: &[],
        }
    }
}

/// Qualification consumes owned PCM and calibration, never a candidate label.
fn qualify_owned_occurrence(state: &AppleSealState, occurrence: &OccurrenceIdentity) -> bool {
    if !state.seal_ledger_enabled {
        return false;
    }
    if occurrence.session != state.session_id || occurrence.capture_epoch != state.capture_epoch {
        return false;
    }
    let Some(calibration) = state.energy_calibration.as_ref() else {
        return false;
    };
    let mut ledger = state
        .acoustic_ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if ledger.is_qualified(occurrence) {
        return true;
    }
    let Some(window) = state.window_by_samples(occurrence.sample_start, occurrence.sample_end)
    else {
        return false;
    };
    if window.samples.is_empty() {
        return false;
    }
    let energy = |samples: &[f32]| {
        samples
            .iter()
            .map(|sample| f64::from(*sample) * f64::from(*sample))
            .sum::<f64>()
    };
    let span_energy = energy(window.samples.as_slice());
    // `min_energy_integral` is the energy of the shortest region that sits at
    // the calibrated existence floor, so only PCM inside hops that reach that
    // floor counts toward it. Summed over the whole span, sub-floor noise
    // qualified 1.17 s of gated EarPods capture whose hops cleared the floor
    // for 20 ms of a 64 ms region (take agent-channel-0-73e58303), and Whisper
    // then wrote "Dziękuję." into that owner. Without a measured floor or a
    // hop clock covering the span nothing is excluded: a missing channel is
    // not silence.
    let energy_integral = floor_clearing_energy(state, occurrence, &window).unwrap_or(span_energy);
    let mean_rms = (span_energy / window.samples.len() as f64).sqrt();
    let peak = window
        .samples
        .iter()
        .map(|sample| f64::from(sample.abs()))
        .fold(0.0_f64, f64::max);
    let dbfs = |linear: f64| {
        if linear > 0.0 {
            20.0 * linear.log10()
        } else {
            f64::NEG_INFINITY
        }
    };
    let evidence = AcousticEvidence {
        occurrence: occurrence.clone(),
        duration_ms: occurrence.sample_len() as f64 * 1_000.0 / state.sample_rate.max(1) as f64,
        energy_integral,
        mean_rms_dbfs: dbfs(mean_rms),
        peak_dbfs: dbfs(peak),
        vad_open_sample: Some(occurrence.sample_start),
        vad_close_sample: Some(occurrence.sample_end),
        evidence_calibration_version: calibration.version.clone(),
    };
    if !ledger.qualify(&evidence, calibration).is_qualified() {
        return false;
    }
    true
}

fn admit_ledger_label<'a>(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    admission: impl Into<RangedLabelAdmission<'a>>,
) -> Option<MutationReceipt> {
    let RangedLabelAdmission { admission, words } = admission.into();
    let LabelAdmission {
        observation,
        label,
        energy,
    } = admission;
    if !state.seal_ledger_enabled
        || (!state.apple_live_enabled
            && matches!(
                observation.producer,
                LedgerObservationProducer::Apple | LedgerObservationProducer::AppleDictation
            ))
    {
        return None;
    }
    let occurrence = observation.occurrence.clone();
    let producer = observation.producer;
    if occurrence.session != state.session_id || occurrence.capture_epoch != state.capture_epoch {
        return None;
    }
    let rewritten = if matches!(
        producer,
        LedgerObservationProducer::Lexicon
            | LedgerObservationProducer::Whisper
            | LedgerObservationProducer::CloudLive
            | LedgerObservationProducer::AppleDictation
    ) {
        let (text, counts) = super::live_lexicon::rewrite(label, &state.lexicon_custom_path);
        state.lexicon_entries_custom = counts.custom;
        if text != label {
            state.lexicon_rewrites = state.lexicon_rewrites.saturating_add(1);
        }
        Some(text)
    } else {
        None
    };
    let label = rewritten.as_deref().unwrap_or(label);
    if matches!(
        energy,
        EnergyAdmission::QualifyFromOwnedPcm | EnergyAdmission::QualifyFinalPassGap
    ) && !qualify_owned_occurrence(state, &occurrence)
    {
        return None;
    }
    let formatter = state.formatter.clone();
    let mut ledger = state
        .acoustic_ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !ledger.is_qualified(&occurrence) {
        return None;
    }
    if producer == LedgerObservationProducer::Apple && !words.is_empty() {
        state.bind_apple_word_measurement(&mut ledger, &occurrence);
    }
    if ledger.frontier_of(&occurrence).is_none() {
        let producers = if energy == EnergyAdmission::QualifyFinalPassGap {
            // A terminal gap qualifies only the observer whose evidence is
            // being admitted: Whisper for a Whisper final pass, the DT witness
            // for a DictationTranscriber return. Apple/Lexicon never observed
            // this occurrence.
            vec![producer]
        } else {
            let mut producers = vec![
                LedgerObservationProducer::Apple,
                LedgerObservationProducer::Lexicon,
            ];
            if state.cloud_commit_tx.is_some() {
                producers.push(LedgerObservationProducer::CloudLive);
            }
            // Reserve the expected acoustic layer before Apple/Lexicon can close
            // the frontier. The PCM plan may return before this owner qualifies.
            if state.tail_patch.is_some() || state.refinement_lane_lost {
                producers.push(LedgerObservationProducer::Whisper);
                state.refinement_receipt(&occurrence, "reserved");
            }
            producers
        };
        ledger.schedule_frontier(occurrence.clone(), producers);
    } else if state.cloud_commit_tx.is_some()
        && energy != EnergyAdmission::QualifyFinalPassGap
        && matches!(
            producer,
            LedgerObservationProducer::Apple | LedgerObservationProducer::Lexicon
        )
    {
        ledger.schedule_observer(occurrence.clone(), LedgerObservationProducer::CloudLive);
    }
    if state.cloud_commit_tx.is_some()
        && producer != LedgerObservationProducer::CloudLive
        && ledger.frontier_of(&occurrence).is_some_and(|frontier| {
            frontier
                .open_producers()
                .contains(&LedgerObservationProducer::CloudLive)
        })
        && !state
            .cloud_inflight
            .iter()
            .any(|commit| commit.occurrences.contains(&occurrence))
    {
        state.cloud_uncommitted.insert(occurrence.clone());
    }
    let receipt = if producer == LedgerObservationProducer::Lexicon
        && !ledger.is_sealed(&occurrence)
    {
        use crate::pipeline::acoustic_ledger::{DictionarySlotRule, SlotTarget};
        let sources = ledger.slots_of(&occurrence).unwrap_or(&[]);
        let merge_sources = super::live_lexicon::registered_merges(
            &sources
                .iter()
                .map(|source| source.text.as_str())
                .collect::<Vec<_>>(),
            &state.lexicon_custom_path,
        )
        .0;
        let rewrites = sources
            .iter()
            .enumerate()
            .filter_map(|(index, source)| {
                if merge_sources
                    .iter()
                    .any(|merge| merge.start <= index && index < merge.end)
                {
                    return None;
                }
                let canonical =
                    super::live_lexicon::rewrite(&source.text, &state.lexicon_custom_path).0;
                (canonical != source.text
                    && source.producer != LedgerObservationProducer::ManualHuman)
                    .then(|| {
                        (
                            SlotTarget::from(source),
                            DictionarySlotRule {
                                id: format!("registered-lexicon/v1/{}=>{}", source.text, canonical),
                                input: vec![source.text.clone()],
                                canonical,
                            },
                        )
                    })
            })
            .collect::<Vec<_>>();
        if rewrites.is_empty() {
            ledger.admit_pinned_label(&observation, label, words)
        } else {
            ledger
                .rewrite_dictionary_slots(&observation, &rewrites)
                .unwrap_or_else(|_| ledger.admit(&observation, label))
        }
    } else {
        ledger.admit_pinned_label(&observation, label, words)
    };
    let request = observation.request;
    let _ = ev_tx.send(EngineEvent::LedgerMutation {
        observation,
        label: match &receipt {
            MutationReceipt::KeepVisibleUnanchored { label, .. } => label.clone(),
            _ => ledger.text_of(&occurrence).unwrap_or(label).to_string(),
        },
        receipt: receipt.clone(),
    });
    drop(ledger);
    state.adjudicate_current_word_slots(ev_tx, &occurrence, request);
    if producer == LedgerObservationProducer::Lexicon {
        state.apply_lexicon_merges(ev_tx, &occurrence, request);
    }
    if matches!(
        producer,
        LedgerObservationProducer::Apple | LedgerObservationProducer::Lexicon
    ) {
        let captured_end = state.audio.session_sample_end();
        maybe_spawn_utterance_witness(state, ev_tx, captured_end);
    }
    let mut ledger = state
        .acoustic_ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if matches!(
        producer,
        LedgerObservationProducer::Whisper | LedgerObservationProducer::CloudLive
    ) && energy != EnergyAdmission::QualifyFinalPassGap
    {
        return Some(receipt);
    }
    let closed = ledger.note_frontier_return(&occurrence, producer);
    if closed && let Ok(seal) = ledger.seal(&occurrence).cloned() {
        let _ = ev_tx.send(EngineEvent::LedgerSeal { receipt: seal });
    }
    schedule_formatter_for_sealed_label(
        &ledger,
        formatter.as_ref(),
        &mut state.formatter_requested,
        &occurrence,
    );
    drop(ledger);
    Some(receipt)
}

struct WhisperWindowSpeech {
    facts: Vec<WhisperWordSpeech>,
    decisions: Vec<WhisperWordSpeechDecision>,
}

/// Measure each Whisper word on the capture clock and judge it.
///
/// Silero comes from the session fusion evidence. Energy uses the calibrated
/// existence floor only. A missing channel stays `None`.
fn measure_whisper_window_speech(
    state: &AppleSealState,
    segments: &[TimedTailSegment],
) -> WhisperWindowSpeech {
    let mut facts = Vec::with_capacity(segments.len());
    let mut previous_key: Option<String> = None;
    let mut previous_span: Option<(u64, u64)> = None;
    for segment in segments {
        let key = repeat_key(&segment.text);
        let separated = match (key.as_ref(), previous_key.as_ref(), previous_span) {
            (Some(key), Some(previous), Some(span)) if key == previous => combine_word_separation(
                energy_valley_between(
                    state,
                    span,
                    (segment.range.sample_start, segment.range.sample_end),
                ),
                silero_dip_between(
                    state,
                    span,
                    (segment.range.sample_start, segment.range.sample_end),
                ),
            ),
            _ => None,
        };
        facts.push(WhisperWordSpeech {
            text: segment.text.clone(),
            sample_start: segment.range.sample_start,
            sample_end: segment.range.sample_end,
            sample_rate: state.sample_rate,
            silero_speech: silero_speech_on_span(
                state,
                segment.range.sample_start,
                segment.range.sample_end,
            ),
            energy_above_floor: energy_above_existence_floor(
                state,
                segment.range.sample_start,
                segment.range.sample_end,
            ),
            speech_near_span: speech_near_span(
                state,
                segment.range.sample_start,
                segment.range.sample_end,
            ),
            separated_from_previous: separated,
            silero_speech_distance: silero_speech_distance(
                state,
                segment.range.sample_start,
                segment.range.sample_end,
            ),
            silence_under_span: silence_under_span(
                state,
                segment.range.sample_start,
                segment.range.sample_end,
            ),
        });
        previous_key = key;
        previous_span = Some((segment.range.sample_start, segment.range.sample_end));
    }
    let decisions = judge_whisper_words_tuned(&facts, &state.engine_tuning);
    if decisions
        .iter()
        .any(|decision| matches!(decision.verdict, WhisperWordSpeechVerdict::Unmeasured))
    {
        debug!(
            words = facts.len(),
            "whisper word speech evidence unmeasured; visibility only"
        );
    }
    WhisperWindowSpeech { facts, decisions }
}

/// Direct-lane text without the Whisper words that have no sound of their own.
///
/// The direct lane has no ledger to receipt a refusal, so every drop is
/// logged. Two drops apply here; the other repeat rules stay with the ledger:
/// - a pinned word Silero heard no speech near, logged with its range,
///   silence class and distance;
/// - an unpinned word identical to the word before it. Pinning leaves out
///   words with no lexical duration, which is what a decoder loop writes
///   (`to, to, to, …` all at the window's last instant), so such a copy has
///   nothing on the capture clock to show for itself.
///
/// The window text is walked token by token against the pins. An unpinned
/// word that is not a copy stays: a failed pin is not evidence of silence.
/// When every pinned word is over silence, nothing stays. When the pins do not
/// line up with the text, the pins alone decide, as before. The text is
/// rejoined only when something was dropped.
fn direct_text_heard_by_silero(
    state: &mut AppleSealState,
    payload: &TailProviderPayload,
) -> String {
    if payload.evidence.segment_grain != TailSegmentGrain::Word {
        return payload.text.clone();
    }
    let window = measure_whisper_window_speech(state, &payload.segments);
    let mut over_silence = vec![false; payload.segments.len()];
    for (((segment, decision), fact), dropped) in payload
        .segments
        .iter()
        .zip(&window.decisions)
        .zip(&window.facts)
        .zip(over_silence.iter_mut())
    {
        if let WhisperWordSpeechVerdict::Refuse(reason) = decision.verdict {
            *dropped = true;
            info!(
                session = %state.session_id,
                sample_start = segment.range.sample_start,
                sample_end = segment.range.sample_end,
                reason = reason.as_str(),
                silence = fact.silence_under_span.as_str(),
                silero_distance = ?fact.silero_speech_distance,
                "direct_word_over_silence"
            );
        }
    }
    let dropped = over_silence.iter().filter(|dropped| **dropped).count();
    state.direct_words_over_silence = state
        .direct_words_over_silence
        .saturating_add(dropped as u64);
    if dropped == payload.segments.len() {
        return String::new();
    }
    let Some(pins) = pin_text_tokens(&payload.text, &payload.segments) else {
        if dropped == 0 {
            return payload.text.clone();
        }
        let kept: Vec<&str> = payload
            .segments
            .iter()
            .zip(&over_silence)
            .filter(|(_, dropped)| !**dropped)
            .map(|(segment, _)| segment.text.trim())
            .filter(|word| !word.is_empty())
            .collect();
        return kept.join(" ");
    };
    let mut kept = Vec::with_capacity(pins.len());
    let mut copies = 0u64;
    let mut previous: Option<String> = None;
    for (token, pin) in payload.text.split_whitespace().zip(pins) {
        let key = repeat_key(token);
        let copy = pin.is_none() && key.is_some() && key == previous;
        previous = key;
        match pin {
            Some(index) if over_silence[index] => {}
            Some(_) => kept.push(token),
            None if copy => copies += 1,
            None => kept.push(token),
        }
    }
    if copies > 0 {
        state.direct_unpinned_repeats = state.direct_unpinned_repeats.saturating_add(copies);
        info!(
            session = %state.session_id,
            sample_start = payload.identity.range.sample_start,
            sample_end = payload.identity.range.sample_end,
            copies,
            "direct_unpinned_repeats_dropped"
        );
    }
    if dropped == 0 && copies == 0 {
        return payload.text.clone();
    }
    kept.join(" ")
}

/// The pin each whitespace token of a window's text belongs to, in order;
/// `None` for a token no pin covers. `None` overall when the pins' own tokens
/// do not appear in the text in that order.
fn pin_text_tokens(text: &str, segments: &[TimedTailSegment]) -> Option<Vec<Option<usize>>> {
    let pinned: Vec<(&str, usize)> = segments
        .iter()
        .enumerate()
        .flat_map(|(index, segment)| {
            segment
                .text
                .split_whitespace()
                .map(move |token| (token, index))
        })
        .collect();
    let mut next = 0;
    let tokens = text
        .split_whitespace()
        .map(|token| {
            let pin = pinned
                .get(next)
                .filter(|(pinned, _)| *pinned == token)
                .map(|&(_, index)| index);
            if pin.is_some() {
                next += 1;
            }
            pin
        })
        .collect();
    (next == pinned.len()).then_some(tokens)
}

/// Decode context overlaps; publication belongs to exactly one grid window.
fn whisper_publication_band(state: &AppleSealState, range: &TailSampleRange) -> (u64, u64) {
    state.window_plan.publication_band(
        range,
        state.audio.session_sample_end(),
        state.capture_stopping,
    )
}

fn whisper_word_in_publication_band(
    state: &AppleSealState,
    decode: &TailSampleRange,
    word: &TailSampleRange,
) -> bool {
    let (start, end) = whisper_publication_band(state, decode);
    let midpoint = word.sample_start + word.sample_end.saturating_sub(word.sample_start) / 2;
    start <= midpoint && midpoint < end
}

fn retain_whisper_words_with_own_speech(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    request_id: u64,
    segments: &[TimedTailSegment],
) -> Vec<TimedTailSegment> {
    let window = measure_whisper_window_speech(state, segments);
    let mut kept = Vec::with_capacity(segments.len());
    for (index, (segment, decision)) in segments.iter().zip(&window.decisions).enumerate() {
        if decision.admits() {
            kept.push(segment.clone());
            continue;
        }
        let energy_silent = window
            .facts
            .get(index)
            .is_some_and(|fact| fact.energy_above_floor == Some(false));
        emit_whisper_word_nonadmission(
            state,
            ev_tx,
            LedgerObservationIdentity::new(
                LedgerObservationProducer::Whisper,
                request_id,
                40_000 + index as u64,
                OccurrenceIdentity::from(&segment.range),
            ),
            segment.text.trim(),
            *decision,
            energy_silent,
        );
    }
    kept
}

/// Every non-admission carries a durable receipt. Partial or unknown speech
/// keeps its label visible without entering a routing path that can insert it.
fn emit_whisper_word_nonadmission(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    observation: LedgerObservationIdentity,
    label: &str,
    decision: WhisperWordSpeechDecision,
    energy_silent: bool,
) {
    if let WhisperWordSpeechVerdict::Refuse(reason) = decision.verdict {
        emit_whisper_word_speech_refusal(state, ev_tx, observation, label, reason, energy_silent);
        return;
    }
    let receipt = state
        .acoustic_ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .keep_visible_unanchored(
            &observation,
            label,
            NoAuthorityReason::ExclusiveTailAwaitingWholeSpan,
        );
    let _ = ev_tx.send(EngineEvent::LedgerMutation {
        observation,
        label: label.to_string(),
        receipt,
    });
}

fn emit_whisper_word_speech_refusal(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    observation: LedgerObservationIdentity,
    label: &str,
    reason: RefuseReason,
    energy_silent: bool,
) {
    let receipt = {
        let mut ledger = state
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if energy_silent && reason == RefuseReason::NoVoicedHopInPin {
            ledger.note_energy_lookup_without_voiced_hop();
        }
        ledger.refuse_replacement(&observation, label, reason)
    };
    let _ = ev_tx.send(EngineEvent::LedgerMutation {
        observation,
        label: label.to_string(),
        receipt,
    });
}

fn combine_word_separation(energy_valley: Option<bool>, silero_dip: Option<bool>) -> Option<bool> {
    match (energy_valley, silero_dip) {
        (Some(true), _) | (_, Some(true)) => Some(true),
        (None, None) => None,
        // A measured channel found no split. The missing channel is not a valley.
        _ => Some(false),
    }
}

fn energy_above_existence_floor(state: &AppleSealState, start: u64, end: u64) -> Option<bool> {
    let floor = state.existence_floor_rms?;
    if !floor.is_finite() || floor <= 0.0 {
        return None;
    }
    let hops = state.capture_energy.hops_at_or_above(
        &state.session_id,
        state.capture_epoch,
        start,
        end,
        floor,
    )?;
    Some(!hops.is_empty())
}

/// Σx² of owned PCM inside the hops that reach the calibrated existence floor,
/// the quantity `min_energy_integral` is defined against. `None` when the take
/// has no floor or the hop clock cannot speak for the whole span.
fn floor_clearing_energy(
    state: &AppleSealState,
    span: &OccurrenceIdentity,
    window: &ResolvedAudioWindow,
) -> Option<f64> {
    let hops = existence_floor_hops(state, span)?;
    let offset = |sample: u64| {
        usize::try_from(sample.saturating_sub(window.sample_start))
            .unwrap_or(usize::MAX)
            .min(window.samples.len())
    };
    Some(
        hops.iter()
            .map(|&(start, end)| {
                let from = offset(start);
                window.samples[from..offset(end).max(from)]
                    .iter()
                    .map(|sample| f64::from(*sample) * f64::from(*sample))
                    .sum::<f64>()
            })
            .sum(),
    )
}

/// `true` only when the take's calibration, floor, hop clock and retained PCM
/// all measured `span` below the existence predicate qualification applies.
/// Any missing channel answers `false`: unmeasured sound is not refused.
fn pcm_measured_below_existence(state: &AppleSealState, span: &OccurrenceIdentity) -> bool {
    let Some(calibration) = state.energy_calibration.as_ref() else {
        return false;
    };
    let Some(window) = state.window_by_samples(span.sample_start, span.sample_end) else {
        return false;
    };
    floor_clearing_energy(state, span, &window)
        .is_some_and(|energy| energy < calibration.min_energy_integral)
}

/// Sample ranges of one occurrence whose capture hops reach the calibrated
/// existence floor. `None` when the take has no floor or the hop clock cannot
/// speak for the whole occurrence; `Some` empty is measured sub-floor sound.
fn existence_floor_hops(
    state: &AppleSealState,
    occurrence: &OccurrenceIdentity,
) -> Option<Vec<(u64, u64)>> {
    let floor = state.existence_floor_rms?;
    if !floor.is_finite() || floor <= 0.0 {
        return None;
    }
    state.capture_energy.hops_at_or_above(
        &state.session_id,
        state.capture_epoch,
        occurrence.sample_start,
        occurrence.sample_end,
        floor,
    )
}

fn energy_valley_between(
    state: &AppleSealState,
    previous: (u64, u64),
    current: (u64, u64),
) -> Option<bool> {
    let floor = state.existence_floor_rms?;
    if !floor.is_finite() || floor <= 0.0 {
        return None;
    }
    let Some(window) = separation_window(previous, current) else {
        // Measured, and the current word does not start after the previous one.
        return state
            .capture_energy
            .hops_at_or_above(
                &state.session_id,
                state.capture_epoch,
                current.0,
                current.1,
                floor,
            )
            .map(|_| false);
    };
    let voiced = state.capture_energy.hops_at_or_above(
        &state.session_id,
        state.capture_epoch,
        window.0,
        window.1,
        floor,
    )?;
    Some(window_has_energy_valley(window, &voiced))
}

/// Silero and the calibrated floor on the span widened by the take's speech
/// radius ([`EngineTuning::whisper_word_speech_near_ms`]) on each side. Where the widened end is past
/// what a channel has observed yet, that channel reads the span's own end.
fn speech_near_span(state: &AppleSealState, start: u64, end: u64) -> Option<bool> {
    let pad =
        u64::from(state.sample_rate) * state.engine_tuning.whisper_word_speech_near_ms / 1_000;
    let start = start.saturating_sub(pad);
    let wide_end = end.saturating_add(pad);
    let silero = silero_speech_on_span(state, start, wide_end)
        .or_else(|| silero_speech_on_span(state, start, end));
    let energy = energy_above_existence_floor(state, start, wide_end)
        .or_else(|| energy_above_existence_floor(state, start, end));
    match (silero, energy) {
        (Some(true), _) | (_, Some(true)) => Some(true),
        (None, None) => None,
        _ => Some(false),
    }
}

/// Silero alone: how far the nearest speech it heard lies from this span.
///
/// Speech already heard within the radius proves nearness at once. A larger
/// distance, or none, is a measurement only once Silero has observed the
/// whole radius after the span: speech may still start right after it.
fn silero_speech_distance(state: &AppleSealState, start: u64, end: u64) -> SileroSpeechDistance {
    let Some(evidence) = observed_silero(state) else {
        return SileroSpeechDistance::Unmeasured;
    };
    if span_inside_observed(&evidence, end).is_none() {
        return SileroSpeechDistance::Unmeasured;
    }
    let rate = u64::from(state.sample_rate.max(1));
    let speech = voiced_onset_silero_ranges(state, &evidence);
    let nearest = speech
        .iter()
        // Zero when the range overlaps the span; otherwise the gap on either side.
        .map(|&(range_start, range_end)| {
            start
                .saturating_sub(range_end)
                .max(range_start.saturating_sub(end))
        })
        .min()
        .map(|samples| samples.saturating_mul(1_000) / rate);
    if let Some(distance) = nearest
        && distance <= state.engine_tuning.whisper_word_speech_near_ms
    {
        return SileroSpeechDistance::Ms(distance);
    }
    let mut radius_end =
        end.saturating_add(rate * state.engine_tuning.whisper_word_speech_near_ms / 1_000);
    if state.capture_stopping {
        // No audio follows a stopped capture, so its end closes the radius.
        radius_end = radius_end.min(state.audio.session_sample_end().max(end));
    }
    if span_inside_observed(&evidence, radius_end).is_none() {
        return SileroSpeechDistance::Unmeasured;
    }
    nearest.map_or(
        SileroSpeechDistance::NoSpeechHeard,
        SileroSpeechDistance::Ms,
    )
}

/// What the span holds on the energy clock: exact digital zero, sound under
/// the calibrated existence floor, or sound reaching it.
fn silence_under_span(state: &AppleSealState, start: u64, end: u64) -> SilenceUnderSpan {
    let Some(audible) = state.capture_energy.hops_at_or_above(
        &state.session_id,
        state.capture_epoch,
        start,
        end,
        DIGITAL_ZERO_ABS,
    ) else {
        return SilenceUnderSpan::EnergyUnmeasured;
    };
    if audible.is_empty() {
        return SilenceUnderSpan::DigitalZero;
    }
    match energy_above_existence_floor(state, start, end) {
        Some(true) => SilenceUnderSpan::SoundWithoutSpeech,
        Some(false) => SilenceUnderSpan::BelowFloor,
        None => SilenceUnderSpan::EnergyUnmeasured,
    }
}

/// Maximum continuous voiced onset before a Silero speech range that Silero's
/// RNN warm-up can miss at the start of an utterance (`2 * SPEECH_ISLAND_GAP_MS`).
const SILERO_VOICED_ONSET_MAX_MS: u64 = 2 * crate::pipeline::speech_islands::SPEECH_ISLAND_GAP_MS;

fn extend_silero_start_to_voiced_onset(
    anchors: &[SpeechAnchor],
    anchor_cursor: &mut usize,
    sample_rate: u32,
    start: u64,
    previous_end: u64,
) -> u64 {
    if *anchor_cursor > 0
        && anchors
            .get(*anchor_cursor - 1)
            .is_some_and(|anchor| anchor.range.sample_end > start)
    {
        *anchor_cursor = 0;
    }
    while anchors
        .get(*anchor_cursor)
        .is_some_and(|anchor| anchor.range.sample_end <= start)
    {
        *anchor_cursor += 1;
    }
    let max_onset_samples =
        u64::from(sample_rate.max(1)).saturating_mul(SILERO_VOICED_ONSET_MAX_MS) / 1_000;
    anchors
        .get(*anchor_cursor)
        .filter(|anchor| {
            anchor.range.sample_start < start
                && anchor.range.sample_end > start
                && start.saturating_sub(anchor.range.sample_start) <= max_onset_samples
        })
        .map_or(start, |anchor| {
            anchor.range.sample_start.max(previous_end.min(start))
        })
}

fn ledger_with_voiced_onsets<'a>(
    state: &AppleSealState,
    ledger: &'a super::silero_fusion::UtteranceLedger,
) -> std::borrow::Cow<'a, super::silero_fusion::UtteranceLedger> {
    let Some(anchors) = state.voiced_onset_anchors() else {
        return std::borrow::Cow::Borrowed(ledger);
    };
    if anchors.is_empty() {
        return std::borrow::Cow::Borrowed(ledger);
    }
    let mut previous_end = 0u64;
    let mut anchor_cursor = 0usize;
    let mut any_changed = false;
    for utterance in ledger.utterances() {
        let new_start = extend_silero_start_to_voiced_onset(
            &anchors,
            &mut anchor_cursor,
            state.sample_rate,
            utterance.range.sample_start,
            previous_end,
        );
        if new_start != utterance.range.sample_start {
            any_changed = true;
            break;
        }
        previous_end = previous_end.max(utterance.range.sample_end);
    }
    if !any_changed {
        return std::borrow::Cow::Borrowed(ledger);
    }
    let mut adjusted = ledger.clone();
    let mut previous_end = 0u64;
    let mut anchor_cursor = 0usize;
    for utterance in adjusted.utterances_mut() {
        utterance.range.sample_start = extend_silero_start_to_voiced_onset(
            &anchors,
            &mut anchor_cursor,
            state.sample_rate,
            utterance.range.sample_start,
            previous_end,
        );
        previous_end = previous_end.max(utterance.range.sample_end);
    }
    std::borrow::Cow::Owned(adjusted)
}

fn voiced_onset_silero_ranges(
    state: &AppleSealState,
    evidence: &AcousticSpeechEvidence,
) -> Vec<(u64, u64)> {
    let anchors = state.voiced_onset_anchors();
    let anchors = anchors.as_deref().unwrap_or(&[]);
    let mut previous_end = 0u64;
    let mut anchor_cursor = 0usize;
    evidence
        .ranges()
        .iter()
        .map(|range| {
            let start = extend_silero_start_to_voiced_onset(
                anchors,
                &mut anchor_cursor,
                state.sample_rate,
                range.sample_start,
                previous_end,
            );
            previous_end = previous_end.max(range.sample_end);
            (start, range.sample_end)
        })
        .collect()
}

/// Whether the span lies on a speech island: Silero speech joined across
/// pauses shorter than
/// [`SPEECH_ISLAND_GAP_MS`](crate::pipeline::speech_islands::SPEECH_ISLAND_GAP_MS),
/// the clamps the Voice Lab
/// take report draws. A pause inside a sentence belongs to its island, so a
/// word pinned there has the right to exist. `None` until Silero has
/// observed the span.
fn silero_speech_on_span(state: &AppleSealState, start: u64, end: u64) -> Option<bool> {
    let evidence = observed_silero(state)?;
    span_inside_observed(&evidence, end)?;
    let observed_samples = evidence.availability().observed_samples()?;
    let speech = voiced_onset_silero_ranges(state, &evidence);
    let islands = join_speech_islands(&speech, state.sample_rate, observed_samples);
    let idx = islands.partition_point(|&(island_start, _, _)| island_start < end);
    Some(idx > 0 && islands[idx - 1].1 > start)
}

fn silero_dip_between(
    state: &AppleSealState,
    previous: (u64, u64),
    current: (u64, u64),
) -> Option<bool> {
    let evidence = observed_silero(state)?;
    span_inside_observed(&evidence, previous.1.max(current.1))?;
    let previous_on = ranges_overlap_span(&evidence, previous.0, previous.1);
    let current_on = ranges_overlap_span(&evidence, current.0, current.1);
    let window = separation_window(previous, current);
    let (true, true, Some(window)) = (previous_on, current_on, window) else {
        return Some(false);
    };
    Some(window_has_silero_dip(
        window,
        evidence
            .ranges()
            .iter()
            .map(|range| (range.sample_start, range.sample_end)),
    ))
}

fn observed_silero(state: &AppleSealState) -> Option<AcousticSpeechEvidence> {
    let evidence = state.fusion.as_ref()?.acoustic_speech_evidence();
    if !evidence
        .identity()
        .matches(&state.session_id, state.capture_epoch)
    {
        return None;
    }
    match evidence.availability() {
        AcousticAvailability::Observed { .. } => Some(evidence),
        _ => None,
    }
}

fn span_inside_observed(evidence: &AcousticSpeechEvidence, end: u64) -> Option<()> {
    match evidence.availability() {
        AcousticAvailability::Observed { observed_samples } if end <= observed_samples => Some(()),
        _ => None,
    }
}

fn ranges_overlap_span(evidence: &AcousticSpeechEvidence, start: u64, end: u64) -> bool {
    evidence
        .ranges()
        .iter()
        .any(|range| range.sample_start < end && start < range.sample_end)
}

/// Admit only whole-session final-pass segments that fit wholly inside one
/// material uncovered speech range.
///
/// The segment's own PCM range becomes the occurrence. A segment that crosses
/// a coverage boundary is ambiguous: some of its words may already belong to
/// a committed Apple occurrence, so admitting its text against the whole gap
/// would duplicate speech. Such a segment is diagnostic evidence only and the
/// terminal coverage remains incomplete.
///
/// The observation producer follows the payload's own evidence source: a
/// Whisper final pass names Whisper, a DictationTranscriber witness names
/// AppleDictation. Nothing here may relabel one engine's return as another's.
fn admit_full_pass_gap_segments(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    full_pass: &TailProviderPayload,
    uncovered_speech_ranges: &[TailSampleRange],
    threshold_samples: u64,
    gap_energy: EnergyAdmission,
) -> usize {
    let producer = match full_pass.evidence.source {
        TailEvidenceSource::Whisper => LedgerObservationProducer::Whisper,
        TailEvidenceSource::AppleDictation => LedgerObservationProducer::AppleDictation,
        TailEvidenceSource::AppleSpeech => {
            let _ = ev_tx.send(EngineEvent::Warning {
                code: "seal_coverage_untrusted_producer".into(),
                message: "gap evidence from a non-decoder source cannot qualify speech".into(),
            });
            return 0;
        }
    };
    let exact_timing = full_pass.evidence.timing_quality
        == crate::stt::tail_provider::TailTimingQuality::ExactSampleRange
        || (cfg!(test)
            && full_pass.evidence.timing_quality
                == crate::stt::tail_provider::TailTimingQuality::Synthetic);
    if !exact_timing || full_pass.validate().is_err() {
        let _ = ev_tx.send(EngineEvent::Warning {
            code: "seal_coverage_untrusted_segment_clock".into(),
            message: "gap evidence requires valid source-PCM segment coordinates".into(),
        });
        return 0;
    }
    let material_gaps = uncovered_speech_ranges
        .iter()
        .filter(|range| range.sample_end.saturating_sub(range.sample_start) > threshold_samples)
        .collect::<Vec<_>>();
    let whisper_speech = (producer == LedgerObservationProducer::Whisper)
        .then(|| measure_whisper_window_speech(state, &full_pass.segments));
    let mut admitted = 0usize;
    let mut contained = 0usize;

    for (generation, segment) in full_pass.segments.iter().enumerate() {
        let label = segment.text.trim();
        if producer == LedgerObservationProducer::Whisper
            && !whisper_word_in_publication_band(state, &full_pass.identity.range, &segment.range)
        {
            continue;
        }
        if label.is_empty() {
            continue;
        }
        // Gap recovery is insertion-only. Even a stale supplied gap cannot
        // authorize a new occurrence over speech already owned by the ledger.
        if producer == LedgerObservationProducer::Whisper
            && state
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .qualified_occurrences()
                .any(|owner| range_overlaps_occurrence(&segment.range, owner))
        {
            continue;
        }
        let Some(_) = material_gaps
            .iter()
            .find(|gap| gap.contains(&segment.range))
        else {
            if material_gaps.iter().any(|gap| gap.overlaps(&segment.range)) {
                let _ = ev_tx.send(EngineEvent::Warning {
                    code: "seal_coverage_full_pass_segment_straddles_gap".to_string(),
                    message: format!(
                        "segment {}..{} crosses a committed/uncovered PCM boundary",
                        segment.range.sample_start, segment.range.sample_end
                    ),
                });
            }
            continue;
        };
        contained = contained.saturating_add(1);

        let observation = LedgerObservationIdentity::new(
            producer,
            full_pass.identity.request_id,
            generation as u64,
            OccurrenceIdentity::from(&segment.range),
        );
        if let Some(window) = whisper_speech.as_ref()
            && let Some(decision) = window.decisions.get(generation)
            && !decision.admits()
        {
            let energy_silent = window
                .facts
                .get(generation)
                .is_some_and(|fact| fact.energy_above_floor == Some(false));
            emit_whisper_word_nonadmission(
                state,
                ev_tx,
                observation,
                label,
                *decision,
                energy_silent,
            );
            continue;
        }
        if state
            .capture_energy
            .voiced_hops_in(
                &segment.range.session,
                segment.range.capture_epoch,
                segment.range.sample_start,
                segment.range.sample_end,
            )
            .is_some_and(|hops| hops.is_empty())
        {
            let receipt = {
                let mut ledger = state
                    .acoustic_ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                ledger.note_energy_lookup_without_voiced_hop();
                ledger.refuse_replacement(&observation, label, RefuseReason::NoVoicedHopInPin)
            };
            let _ = ev_tx.send(EngineEvent::LedgerMutation {
                observation,
                label: label.to_string(),
                receipt,
            });
            continue;
        }
        if admit_ledger_label(
            state,
            ev_tx,
            LabelAdmission {
                observation,
                label,
                energy: gap_energy,
            },
        )
        .is_some_and(|receipt| receipt.grants_mutation())
        {
            admitted = admitted.saturating_add(1);
        }
    }

    if admitted == 0 && contained == 0 && !material_gaps.is_empty() {
        let _ = ev_tx.send(EngineEvent::Warning {
            code: "seal_coverage_full_pass_gap_unresolved".to_string(),
            message: "material gap had no source-mapped segment wholly contained in it".to_string(),
        });
    }
    admitted
}

/// Padded fusion ownership windows, summed. Diagnostics only: this is the
/// number that read a whole archived take as 99.5% speech.
fn fusion_utterance_ranges(state: &AppleSealState) -> Vec<TailSampleRange> {
    state
        .fusion
        .as_ref()
        .map(|fusion| {
            fusion
                .ledger()
                .utterances()
                .iter()
                .filter(|utterance| utterance.closed)
                .map(|utterance| utterance.range.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

/// Choose the acoustic observer: the narrowest one that actually measured.
///
/// Silero crossings first — a set that exists is proof the VAD ran and heard
/// speech, which is the narrowest honest answer. Otherwise the capture energy
/// ladder, the other authenticated observer over the same PCM. Padded fusion
/// ownership windows are no longer a candidate: they stay open across pauses
/// and close on the capture cursor, so they measure ownership, never speech.
///
/// When neither observer measured, the answer is the unavailable evidence the
/// capture owner itself reports. Absence keeps its own name here instead of
/// arriving at the ledger as an empty set.
fn energy_lookup_without_voiced_hop(state: &AppleSealState) -> bool {
    let energy = state.capture_energy.session_active_speech_ranges(
        &state.session_id,
        state.capture_epoch,
        state.sample_rate,
    );
    matches!(energy.availability(), AcousticAvailability::Observed { .. })
        && energy.ranges().is_empty()
}

fn coverage_speech_evidence(state: &AppleSealState) -> AcousticSpeechEvidence {
    let captured_samples = state.audio.session_sample_end();
    let capture_energy = state.capture_energy.session_active_speech_ranges(
        &state.session_id,
        state.capture_epoch,
        state.sample_rate,
    );
    // The capture writer adjudicates the PCM it wrote. When it measured samples
    // it could not read, no later observer over the same buffer may certify
    // them: a downstream reader sees the identical NaN as a probability that
    // fails every threshold, which is indistinguishable from silence.
    if matches!(
        capture_energy.availability(),
        AcousticAvailability::InvalidMeasurement { .. }
    ) {
        return capture_energy;
    }
    if let Some(fusion) = state.fusion.as_ref() {
        let acoustic = fusion.acoustic_speech_evidence();
        if acoustic.observed_speech() {
            return within_capture(acoustic, captured_samples);
        }
        let silero = within_capture(acoustic, captured_samples);
        if loudness_is_measured_silence(state, &silero, &capture_energy) {
            return silero;
        }
    }
    within_capture(capture_energy, captured_samples)
}

/// The energy ladder measures loudness, not speech. Its ranges stop counting
/// as speech only when the speech model measured silence twice over them and
/// no recognizer produced a word anywhere in the take: Silero heard nothing
/// across the whole capture, and every loud range lies inside returned
/// Whisper windows whose own VAD selected nothing, with no window over it
/// that heard speech. Anything less keeps the ladder, and its refusal.
fn loudness_is_measured_silence(
    state: &AppleSealState,
    silero: &AcousticSpeechEvidence,
    energy: &AcousticSpeechEvidence,
) -> bool {
    let silero_silent = matches!(silero.availability(), AcousticAvailability::Observed { .. })
        && silero.ranges().is_empty();
    let energy_measured = matches!(energy.availability(), AcousticAvailability::Observed { .. })
        && !energy.ranges().is_empty();
    let apple_silent = state.last_apple_segment_end <= 0.0
        && state.open_partial.trim().is_empty()
        && state.closed_phrases.is_empty()
        && state.refused_untimed_words.is_empty();
    if !(silero_silent && energy_measured && apple_silent) {
        return false;
    }
    energy.ranges().iter().all(|loud| {
        let heard = state.tail_vad_windows.iter().any(|&(start, end, heard)| {
            heard && start < loud.sample_end && loud.sample_start < end
        });
        !heard && vad_silent_windows_cover(&state.tail_vad_windows, loud)
    })
}

fn vad_silent_windows_cover(windows: &[(u64, u64, bool)], range: &TailSampleRange) -> bool {
    let mut silent = windows
        .iter()
        .filter(|&&(_, _, heard)| !heard)
        .map(|&(start, end, _)| (start, end))
        .collect::<Vec<_>>();
    silent.sort_unstable();
    let mut reached = range.sample_start;
    for (start, end) in silent {
        if start > reached {
            break;
        }
        reached = reached.max(end);
        if reached >= range.sample_end {
            return true;
        }
    }
    reached >= range.sample_end
}

/// Hold an observer's measured extent against the capture the take produced.
///
/// An observer reports how much PCM reached *it*. Nothing in the pipeline
/// guarantees that equals what the microphone produced — a chunk lane that
/// stops forwarding leaves no hole to detect, because the extent simply ends
/// early. The ledger compares committed and measured spans against the extent
/// the observer claims, so without this the unheard remainder was certified
/// covered whenever no word happened to be committed inside it.
///
/// `state.audio` is the capture owner's own count of samples seen this
/// session — retained or evicted — so this compares two authenticated facts.
/// It only ever downgrades: a short observer becomes unavailable, and no
/// evidence is promoted.
fn within_capture(
    evidence: AcousticSpeechEvidence,
    captured_samples: u64,
) -> AcousticSpeechEvidence {
    let Some(observed_samples) = evidence.availability().observed_samples() else {
        return evidence;
    };
    if observed_samples >= captured_samples {
        return evidence;
    }
    AcousticSpeechEvidence::unavailable(
        evidence.identity().clone(),
        evidence.producer(),
        AcousticAvailability::Discontinuous { observed_samples },
    )
}

fn publish_terminal_coverage(
    state: &AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
) -> SealCoverageReceipt {
    let speech = coverage_speech_evidence(state);
    let speech_samples = sum_range_samples(speech.ranges());
    let utterance_ranges = fusion_utterance_ranges(state);

    // Both numbers on one line, text-free. The acoustic set is what the receipt
    // was measured against; the utterance sum is what the receipt used to be
    // measured against, kept visible so a regression on this lane is one grep
    // away instead of a re-derivation from a 50-minute take.
    info!(
        session_id = %state.session_id,
        capture_epoch = state.capture_epoch,
        producer = speech.producer(),
        availability = speech.availability().as_str(),
        observed_samples = speech.availability().observed_samples(),
        speech_ranges = speech.ranges().len(),
        speech_samples,
        utterance_ranges = utterance_ranges.len(),
        utterance_samples = sum_range_samples(&utterance_ranges),
        capture_samples = state.audio.session_sample_end(),
        "seal_speech_set"
    );

    let no_voiced_hop = energy_lookup_without_voiced_hop(state);
    let islands = measure_islands(state);
    let mut ledger = state
        .acoustic_ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if no_voiced_hop {
        ledger.note_energy_lookup_without_voiced_hop();
    }
    let receipt = ledger.assess_seal_coverage(
        &state.session_id,
        state.capture_epoch,
        &speech,
        u64::from(state.sample_rate) * SEAL_COVERAGE_INCOMPLETE_MS / 1_000,
    );
    ledger.record_seal_coverage(receipt.clone());
    if let Some(islands) = islands {
        ledger.record_speech_islands(islands);
    }
    let _ = ev_tx.send(EngineEvent::SealCoverage {
        receipt: receipt.clone(),
        comparison: None,
    });
    receipt
}

/// Silero speech of this capture joined into islands, with syllable nuclei
/// counted from the capture energy. `None` while Silero has not observed this
/// epoch: energy alone would turn noise into islands.
fn measure_islands(state: &AppleSealState) -> Option<SpeechIslandEvidence> {
    let silero = observed_silero(state)?;
    let observed_samples = silero
        .availability()
        .observed_samples()?
        .min(state.audio.session_sample_end());
    let speech: Vec<(u64, u64)> = silero
        .ranges()
        .iter()
        .map(|range| (range.sample_start, range.sample_end))
        .collect();
    let hops = state
        .capture_energy
        .session_hop_levels(&state.session_id, state.capture_epoch);
    Some(measure_speech_islands(SpeechIslandInput {
        session_id: &state.session_id,
        capture_epoch: state.capture_epoch,
        sample_rate: state.sample_rate,
        observed_samples,
        speech: &speech,
        hops: hops.as_deref(),
        existence_floor_rms: state.existence_floor_rms,
    }))
}

/// Total samples described by a set of ranges, without merging overlaps.
/// Diagnostics arithmetic only — the receipt does its own merge.
fn sum_range_samples(ranges: &[TailSampleRange]) -> u64 {
    ranges
        .iter()
        .map(|range| range.sample_end.saturating_sub(range.sample_start))
        .sum()
}

fn range_overlaps_occurrence(range: &TailSampleRange, occurrence: &OccurrenceIdentity) -> bool {
    // Integer tests first: this runs for every retained decode against every
    // unsealed owner on each worker tick, and the session string compare was
    // the hottest leaf of that product (CPU samples 2026-10-14, builds 982/983).
    range.sample_start < occurrence.sample_end
        && occurrence.sample_start < range.sample_end
        && range.capture_epoch == occurrence.capture_epoch
        && range.session == occurrence.session
}

// Returned Word clocks can precede the first VAD owner or fall in an owner
// gap. Keep the same ledger qualification and measured-word admission paths;
// a decoder label alone cannot own PCM or rewrite an existing occurrence.
fn admit_retained_unowned_words(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    payload: &TailProviderPayload,
    live_before: Option<u64>,
) -> bool {
    use crate::stt::tail_provider::{TailEvidenceSource, TailEvidenceStability, TailSegmentGrain};

    let decode = &payload.identity.range;
    if payload.validate().is_err()
        || payload.evidence.source != TailEvidenceSource::Whisper
        || payload.evidence.stability != TailEvidenceStability::Final
        || payload.evidence.timing_quality
            != crate::stt::tail_provider::TailTimingQuality::ExactSampleRange
        || decode.session != state.session_id
        || decode.capture_epoch != state.capture_epoch
        || decode.sample_end > state.audio.session_sample_end()
        || decode.sample_end.saturating_sub(decode.sample_start) > u64::from(state.sample_rate) * 9
    {
        return false;
    }
    let whisper_speech = measure_whisper_window_speech(state, &payload.segments);
    let mut qualified = false;
    for (index, segment) in payload.segments.iter().enumerate() {
        // Context words cannot emit visible refusal receipts before their
        // own publication window has had its turn.
        if segment.grain != TailSegmentGrain::Word
            || !whisper_word_in_publication_band(state, decode, &segment.range)
        {
            continue;
        }
        if let Some(decision) = whisper_speech.decisions.get(index)
            && !decision.admits()
        {
            let observation = LedgerObservationIdentity::new(
                LedgerObservationProducer::Whisper,
                payload.identity.request_id,
                50_000 + index as u64,
                OccurrenceIdentity::from(&segment.range),
            );
            let energy_silent = whisper_speech
                .facts
                .get(index)
                .is_some_and(|fact| fact.energy_above_floor == Some(false));
            emit_whisper_word_nonadmission(
                state,
                ev_tx,
                observation,
                segment.text.trim(),
                *decision,
                energy_silent,
            );
            continue;
        }
        if segment.text.split_whitespace().count() != 1
            || !decode.contains(&segment.range)
            || state
                .capture_energy
                .voiced_hops_in(
                    &segment.range.session,
                    segment.range.capture_epoch,
                    segment.range.sample_start,
                    segment.range.sample_end,
                )
                .is_none_or(|hops| hops.is_empty())
        {
            continue;
        }
        let already_owned = state
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .qualified_occurrences()
            .any(|owner| range_overlaps_occurrence(&segment.range, owner));
        if already_owned {
            continue;
        }
        let owner = OccurrenceIdentity::from(&segment.range);
        if let Some(before) = live_before {
            if owner.sample_end > before
                || state.fusion.as_ref().is_none_or(|fusion| {
                    fusion
                        .ledger()
                        .utterances()
                        .iter()
                        .any(|utterance| range_overlaps_occurrence(&utterance.range, &owner))
                })
            {
                continue;
            }
            let speech = coverage_speech_evidence(state);
            let mut ledger = state
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ledger.record_speech_evidence(&speech);
            state.record_decode_fence_measurement(
                &mut ledger,
                Some((decode.sample_start, decode.sample_end)),
            );
            // A non-origin left edge needs the same authenticated quiet
            // onset as admission. Keep origin visibility separate: an unknown
            // capture onset still cannot become a complete acoustic witness.
            if (owner.sample_start != 0
                && !ledger.decode_word_onset_complete(
                    &owner,
                    decode.sample_start,
                    decode.sample_end,
                    owner.sample_start,
                ))
                || ledger.decode_word_fence_incomplete(
                    &owner,
                    decode.sample_start,
                    decode.sample_end,
                    owner.sample_start,
                    owner.sample_end,
                )
            {
                continue;
            }
        }
        if !qualify_owned_occurrence(state, &owner) {
            continue;
        }
        qualified = true;
        {
            let mut ledger = state
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ledger.bind_word_provider(payload);
            ledger.schedule_frontier(owner.clone(), vec![LedgerObservationProducer::Whisper]);
        }
        let pin = RoutedPin {
            index,
            pin: owner.clone(),
            text: segment.text.trim().to_string(),
            confidence: segment.confidence,
        };
        state.admit_routed_words_with_decode_window(
            ev_tx,
            0,
            &owner,
            (
                payload.identity.request_id,
                Some((decode.sample_start, decode.sample_end)),
            ),
            RoutedWords {
                pins: std::slice::from_ref(&pin),
                neighbours: &[],
            },
            LedgerObservationProducer::Whisper,
        );
        // Leave Whisper open until all already-returned windows have had
        // their turn. Arrival order cannot seal the first spelling here.
    }
    qualified
}

/// Reconcile completed planned returns and publish the remaining acoustic debt.
/// This terminal path has no authority to construct or execute another request.
fn reconcile_terminal_coverage(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
) -> SealCoverageReceipt {
    state.replay_completed_windows(ev_tx);
    let threshold_samples =
        u64::from(state.sample_rate).saturating_mul(SEAL_COVERAGE_INCOMPLETE_MS) / 1_000;
    let speech = coverage_speech_evidence(state);
    // Settled owners already consumed their returned windows. Reconsider a
    // retained frame only where measured speech still has no qualified owner;
    // old quiet/settled frames must not re-enter admission at capture Stop.
    // Use zero tolerance for eligibility: a returned short real word remains
    // admissible even below the terminal coverage reporting threshold.
    let uncovered = {
        let ledger = state
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        uncovered_gap_ranges(&ledger, &state.session_id, state.capture_epoch, &speech, 0)
    };
    let retained = state
        .retained_word_decodes
        .iter()
        .filter(|payload| {
            uncovered
                .iter()
                .any(|gap| gap.overlaps(&payload.identity.range))
        })
        .cloned()
        .collect::<Vec<_>>();
    for payload in &retained {
        admit_retained_unowned_words(state, ev_tx, payload, None);
    }
    // These are retained results, not new model jobs. Newly qualified owners
    // now receive their original overlapping windows before frontier closure.
    state.replay_completed_windows(ev_tx);
    for payload in retained {
        let uncovered = {
            let ledger = state
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            uncovered_gap_ranges(
                &ledger,
                &state.session_id,
                state.capture_epoch,
                &speech,
                threshold_samples,
            )
        };
        admit_full_pass_gap_segments(
            state,
            ev_tx,
            &payload,
            &uncovered,
            threshold_samples,
            EnergyAdmission::QualifyFinalPassGap,
        );
    }
    state.close_admission_horizon(ev_tx, state.window_plan.admission_horizon());
    state.publish_resolved_word_seals(ev_tx);
    state.seal_remaining_at_session_end(ev_tx);
    let mut ledger = state
        .acoustic_ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let receipt = ledger.assess_seal_coverage(
        &state.session_id,
        state.capture_epoch,
        &speech,
        threshold_samples,
    );
    ledger.record_seal_coverage(receipt.clone());
    for range in &receipt.uncovered_speech_ranges {
        if range.sample_end.saturating_sub(range.sample_start) > threshold_samples {
            ledger.note_unrecovered_speech(&OccurrenceIdentity::from(range));
        }
    }
    drop(ledger);
    let _ = ev_tx.send(EngineEvent::SealCoverage {
        receipt: receipt.clone(),
        comparison: None,
    });
    receipt
}

/// Uncovered measured speech with every qualified owner's PCM subtracted.
///
/// This is the same gap arithmetic the terminal reconcile loop applies before
/// admitting final-pass evidence: a pending owner still punches its own debt
/// out of coverage inside `assess_seal_coverage`, so what remains here is
/// speech no committed or debt-held range can account for.
fn uncovered_gap_ranges(
    ledger: &AcousticLedger,
    session: &str,
    capture_epoch: u64,
    speech: &AcousticSpeechEvidence,
    threshold_samples: u64,
) -> Vec<TailSampleRange> {
    let mut gaps = ledger
        .assess_seal_coverage(session, capture_epoch, speech, threshold_samples)
        .uncovered_speech_ranges;
    for owner in ledger.qualified_occurrences() {
        gaps = gaps
            .into_iter()
            .flat_map(|gap| {
                if !range_overlaps_occurrence(&gap, owner) {
                    return vec![gap];
                }
                let mut remaining = Vec::new();
                if gap.sample_start < owner.sample_start {
                    remaining.push(TailSampleRange {
                        sample_end: owner.sample_start,
                        ..gap.clone()
                    });
                }
                if owner.sample_end < gap.sample_end {
                    remaining.push(TailSampleRange {
                        sample_start: owner.sample_end,
                        ..gap
                    });
                }
                remaining
            })
            .collect();
    }
    gaps
}

/// Closed-take context added on each side of a witness target. The read stays
/// inside the recorder-owned archive; the wider window lets the decoder hear
/// the phrase that contains the unresolved range.
const DICTATION_WITNESS_CONTEXT_SECS: u64 = 3;
/// One witness decode never reads more than this much capture PCM.
const DICTATION_WITNESS_MAX_WINDOW_SECS: u64 = 30;
/// One take issues at most this many witness decodes, in capture order.
const DICTATION_WITNESS_MAX_REQUESTS: usize = 4;
/// Per-request wall clock. The bridge child is killed and reaped past it.
const DICTATION_WITNESS_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Whole terminal witness phase. Whatever is undecoded past it stays an
/// explicit unresolved outcome; the take never parks on the witness.
const DICTATION_WITNESS_TOTAL_BUDGET: Duration = Duration::from_secs(90);
/// Request-id band for terminal witness decodes. The capture plan owns small
/// ids and lexical trials live above `1 << 63`; this band keeps a witness
/// return acoustically distinct from both.
const DICTATION_WITNESS_REQUEST_BASE: u64 = 1 << 62;
/// Live per-utterance witness requests mint ids in their own band, distinct
/// from the terminal ordinals sharing the witness base.
const DICTATION_WITNESS_LIVE_REQUEST_BASE: u64 = DICTATION_WITNESS_REQUEST_BASE | (1 << 61);

/// A witness thread's answer: the request identity on the capture clock and
/// the decode outcome, stringified on failure. The worker thread admits it.
type DictationWitnessReturn = (TailRequestIdentity, Result<TailProviderPayload, String>);

/// Outstanding recovery debt the witness may still answer: unsealed
/// recovery-pending owners plus material measured-speech gaps. Both witness
/// triggers (per-utterance and terminal) aim from this one picture.
fn witness_recovery_targets(
    state: &AppleSealState,
    threshold_samples: u64,
) -> (Vec<OccurrenceIdentity>, Vec<TailSampleRange>) {
    let speech = coverage_speech_evidence(state);
    let ledger = state
        .acoustic_ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let pending =
        ledger
            .qualified_occurrences()
            .filter(|owner| {
                owner.session == state.session_id
                    && owner.capture_epoch == state.capture_epoch
                    && !ledger.is_sealed(owner)
                    && (ledger.text_recovery_pending(owner)
                        || ledger.sparse_label_owner(owner)
                        || ledger.slots_of(owner).is_some_and(|slots| {
                            slots.iter().any(|slot| {
                    matches!(&slot.witness,
                        crate::pipeline::acoustic_ledger::SlotWitness::Unwitnessed
                        | crate::pipeline::acoustic_ledger::SlotWitness::Absent { .. })
                })
                        }))
            })
            .cloned()
            .collect::<Vec<_>>();
    let gaps = uncovered_gap_ranges(
        &ledger,
        &state.session_id,
        state.capture_epoch,
        &speech,
        threshold_samples,
    )
    .into_iter()
    .filter(|range| range.sample_end.saturating_sub(range.sample_start) > threshold_samples)
    .collect::<Vec<_>>();
    (pending, gaps)
}

/// Debit only debt actually covered by the issued witness question.
fn record_dictation_witness_attempts(
    attempts: &mut Vec<((u64, u64), u8)>,
    targets: &[(u64, u64)],
    window_start: u64,
    window_end: u64,
    max_window: u64,
) {
    for (index, target) in targets.iter().enumerate() {
        // An attempt binds only when the question was actually asked: the
        // issued window covers the whole target, or the target leads the
        // window while being wider than any legal window — its leading read
        // counts and the range still ends honestly unresolved. A sliver of
        // coverage, or an oversized range outside the window, costs nothing.
        let covered_whole = target.0 >= window_start && target.1 <= window_end;
        let leading_oversized = index == 0 && target.1.saturating_sub(target.0) > max_window;
        if !covered_whole && !leading_oversized {
            continue;
        }
        let entry = attempts.iter_mut().find(|(range, _)| range == target);
        match entry {
            Some((_, count)) => *count = count.saturating_add(1),
            None => attempts.push((*target, 1)),
        }
    }
}

/// A witness window and the targets it may debit, as capture sample ranges.
type WitnessQuestion = ((u64, u64), Vec<(u64, u64)>);

/// A live question names a whole owner that ends on closed PCM, under the
/// owner's own range, once per session.
fn live_witness_may_ask(
    owner: (u64, u64),
    sealed_end: u64,
    max_len: u64,
    attempts: &[((u64, u64), u8)],
) -> bool {
    owner.1 <= sealed_end
        && owner.1.saturating_sub(owner.0) <= max_len
        && !attempts
            .iter()
            .any(|(range, count)| *range == owner && *count > 0)
}

/// The per-utterance witness question over outstanding targets (sorted,
/// deduplicated, closed PCM only): the window and the targets it may debit.
///
/// Only debt the retained live capture still holds can be asked. Anchoring
/// the window on evicted debt fails the lookup on every seal and starves every
/// newer gap behind it, so older debt waits for the terminal archive pass. The
/// window's leading context never reaches before the retained start.
fn live_witness_question(
    targets: &[(u64, u64)],
    retained_start: u64,
    sealed_end: u64,
    context: u64,
    max_window: u64,
) -> Option<WitnessQuestion> {
    let askable: Vec<(u64, u64)> = targets
        .iter()
        .copied()
        .filter(|&(start, _)| start >= retained_start)
        .collect();
    let first_start = askable.first()?.0;
    let last_end = askable.last()?.1;
    let window_start = first_start.saturating_sub(context).max(retained_start);
    let window_end = last_end
        .saturating_add(context)
        .min(sealed_end)
        .min(window_start.saturating_add(max_window));
    (window_end > window_start).then_some(((window_start, window_end), askable))
}

/// Terminal witness windows: targets (sorted, deduplicated) clustered into
/// disjoint windows with bounded context, ordered by the debt each covers,
/// largest first, earlier window on a tie.
///
/// The request budget is bounded. Spent in time order, a long take's whole
/// budget went to its first minutes while later holes stayed empty.
fn terminal_witness_windows(
    targets: &[(u64, u64)],
    capture_samples: u64,
    context: u64,
    max_window: u64,
) -> Vec<(u64, u64)> {
    // (first target start, last target end, summed target length)
    let mut clusters: Vec<(u64, u64, u64)> = Vec::new();
    for &(start, end) in targets {
        let extend = match clusters.last() {
            Some(&(first_start, last_end, _)) => {
                start <= last_end.saturating_add(2 * context)
                    && end
                        .saturating_add(context)
                        .saturating_sub(first_start.saturating_sub(context))
                        <= max_window
            }
            None => false,
        };
        let debt = end.saturating_sub(start);
        if extend {
            let cluster = clusters
                .last_mut()
                .expect("a cluster matched the extension check");
            cluster.1 = cluster.1.max(end);
            cluster.2 = cluster.2.saturating_add(debt);
        } else {
            clusters.push((start, end, debt));
        }
    }
    let mut windows: Vec<(u64, u64, u64)> = clusters
        .iter()
        .map(|&(first_start, last_end, debt)| {
            let window_start = first_start.saturating_sub(context);
            // A single oversized target must not widen the window past the
            // cap either: the witness reads from the target's leading edge —
            // the lost-beginning failure mode — and whatever does not fit
            // stays honestly unresolved instead of silently exceeding the
            // per-request PCM bound.
            let window_end = last_end
                .saturating_add(context)
                .min(capture_samples)
                .min(window_start.saturating_add(max_window));
            (window_start, window_end, debt)
        })
        .filter(|&(start, end, _)| end > start)
        .collect();
    windows.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    windows
        .into_iter()
        .map(|(start, end, _)| (start, end))
        .collect()
}

/// Pending owners a live DT question may name now.
///
/// DT follows the Whisper tank (Maciej, 2026-10-22: Apple live, then the
/// Whisper windows, then DT, then the seal). A word sits in up to three
/// 9 s windows on the 3 s grid; while a submitted, queued or not yet planned
/// window may still cover an owner whose Whisper reservation is open, that
/// owner is not asked. Asked earlier, DT answered about 0.5 s after the owner
/// closed, and on 39 replays 57 Whisper corrections then landed on owners DT
/// had already answered (base: 0). The owner becomes askable when `close_admission_horizon`
/// returns its Whisper reservation, in `finish_whisper_frontier`, before that
/// return can seal it.
fn live_witness_askable_owners(
    state: &AppleSealState,
    pending_owners: Vec<OccurrenceIdentity>,
    sealed_end: u64,
) -> Vec<OccurrenceIdentity> {
    let max_len = u64::from(state.sample_rate).saturating_mul(DICTATION_WITNESS_MAX_WINDOW_SECS);
    let ledger = state
        .acoustic_ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    pending_owners
        .into_iter()
        .filter(|owner| {
            live_witness_may_ask(
                (owner.sample_start, owner.sample_end),
                sealed_end,
                max_len,
                &state.dictation_witness_attempts,
            ) && ledger.frontier_of(owner).is_some_and(|frontier| {
                !frontier.is_closed()
                    && !(frontier
                        .open_producers()
                        .contains(&LedgerObservationProducer::Whisper)
                        && state.whisper_work_may_cover(owner))
            })
        })
        .collect()
}

/// Reserve independent lexical work before producer return can seal an owner.
/// One live request is in flight; each owner gets one protected live question.
/// Terminal reconciliation owns retry work and its common time budget.
fn maybe_spawn_utterance_witness(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    sealed_end: u64,
) {
    if !state.dictation_witness_armed
        || state.dictation_witness_live_closed
        || state.dictation_witness_inflight
        || sealed_end == 0
        || state.dictation_witness_return.is_none()
    {
        return;
    }
    let threshold_samples =
        u64::from(state.sample_rate).saturating_mul(SEAL_COVERAGE_INCOMPLETE_MS) / 1_000;
    let (pending_owners, _gaps) = witness_recovery_targets(state, threshold_samples);
    // A live lexical job must acquire an open frontier exactly once. Retried
    // or oversized questions belong to serialized terminal reconciliation;
    // a returned producer cannot protect another live job from sealing.
    // Only whole owners on closed PCM are asked, under their own range: a
    // target clipped at a growing seal minted a new range, so the same owner
    // was asked again on every seal, about every 0.35 s. Speech no owner
    // covers yet is the live head Apple has not finalized; a live answer
    // admitted there landed beside Apple's later final for the same audio
    // (fe468a53: whole DT phrases doubled). Gaps belong to the terminal pass.
    let pending_owners = live_witness_askable_owners(state, pending_owners, sealed_end);

    let mut targets: Vec<(u64, u64)> = pending_owners
        .iter()
        .map(|owner| (owner.sample_start, owner.sample_end))
        .filter(|&(start, end)| end > start)
        .collect();
    targets.sort_unstable();
    targets.dedup();
    if targets.is_empty() {
        return;
    }
    let context = u64::from(state.sample_rate).saturating_mul(DICTATION_WITNESS_CONTEXT_SECS);
    let max_window = u64::from(state.sample_rate).saturating_mul(DICTATION_WITNESS_MAX_WINDOW_SECS);
    // Before Stop the live buffer is the only PCM; once the archive owns the
    // take, every target is askable.
    let retained_start = if state.terminal_pcm.is_some() {
        0
    } else {
        state.audio.retained_start_sample()
    };
    let Some(((window_start, window_end), targets)) =
        live_witness_question(&targets, retained_start, sealed_end, context, max_window)
    else {
        return;
    };
    let Some(window) = state.window_by_samples(window_start, window_end) else {
        let _ = ev_tx.send(EngineEvent::Warning {
            code: "dictation_witness_pcm_unavailable".into(),
            message: format!(
                "live window {window_start}..{window_end} is not in the retained capture"
            ),
        });
        return;
    };
    record_dictation_witness_attempts(
        &mut state.dictation_witness_attempts,
        &targets,
        window_start,
        window_end,
        max_window,
    );
    let identity = TailRequestIdentity {
        request_id: DICTATION_WITNESS_LIVE_REQUEST_BASE | state.dictation_witness_requests,
        range: TailSampleRange {
            session: state.session_id.clone(),
            capture_epoch: state.capture_epoch,
            sample_start: window.sample_start,
            sample_end: window.sample_end,
        },
    };
    state.dictation_witness_requests = state.dictation_witness_requests.saturating_add(1);
    state.dictation_witness_inflight = true;
    state.dictation_witness_owners = {
        let mut ledger = state
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut reserved = Vec::new();
        for owner in &pending_owners {
            if owner.sample_start >= identity.range.sample_start
                && owner.sample_end <= identity.range.sample_end
                && ledger
                    .schedule_observer(owner.clone(), LedgerObservationProducer::AppleDictation)
            {
                reserved.push(owner.clone());
            }
        }
        reserved
    };
    let return_tx = state
        .dictation_witness_return
        .clone()
        .expect("armed witness holds a return channel");
    let language = state.dictation_witness_language.clone();
    let sample_rate = state.sample_rate;
    let samples = window.samples;
    let thread_identity = identity.clone();
    std::thread::spawn(move || {
        let result = crate::stt::apple_stt::transcribe_dictation_witness(
            &thread_identity,
            &samples,
            sample_rate,
            language.as_deref(),
            DICTATION_WITNESS_REQUEST_TIMEOUT,
        )
        .map_err(|error| format!("{error:#}"));
        let _ = return_tx.send((thread_identity, result));
    });
    info!(
        session = %state.session_id,
        capture_epoch = state.capture_epoch,
        request_id = identity.request_id,
        sample_start = identity.range.sample_start,
        sample_end = identity.range.sample_end,
        "dictation_witness_live_request"
    );
}

/// Admit witness returns on the worker thread. The document may have moved
/// since the ask, so targets are recomputed at answer time; an owner that
/// settled meanwhile is skipped by admission itself.
fn drain_dictation_witness_returns(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    witness_done: &std_mpsc::Receiver<DictationWitnessReturn>,
) {
    if !state.apple_live_enabled || !state.seal_ledger_enabled {
        return;
    }
    while let Ok(returned) = witness_done.try_recv() {
        handle_dictation_witness_return(state, ev_tx, returned);
    }
}

fn handle_dictation_witness_return(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    returned: DictationWitnessReturn,
) {
    let (identity, result) = returned;
    let payload = match result {
        Ok(payload) => payload,
        Err(error) => {
            let _ = ev_tx.send(EngineEvent::Warning {
                code: "dictation_witness_unavailable".into(),
                message: format!(
                    "window {}..{}: {error}",
                    identity.range.sample_start, identity.range.sample_end
                ),
            });
            finish_dictation_witness_frontier(state, ev_tx);
            state.dictation_witness_inflight = false;
            resume_witness_waiting_closures(state, ev_tx);
            return;
        }
    };
    info!(
        session = %state.session_id,
        capture_epoch = state.capture_epoch,
        request_id = identity.request_id,
        sample_start = identity.range.sample_start,
        sample_end = identity.range.sample_end,
        segments = payload.segments.len(),
        elapsed_ms = payload.elapsed_ms,
        "dictation_witness_return"
    );
    let threshold_samples =
        u64::from(state.sample_rate).saturating_mul(SEAL_COVERAGE_INCOMPLETE_MS) / 1_000;
    // A live answer testifies about the whole owners it heard, never about
    // speech no owner covers yet: that is Apple's unfinalized head, and the
    // terminal pass owns real gaps.
    let (pending_owners, _gaps) = witness_recovery_targets(state, threshold_samples);
    let pending_owners = pending_owners
        .into_iter()
        .filter(|owner| {
            owner.sample_start >= identity.range.sample_start
                && owner.sample_end <= identity.range.sample_end
        })
        .collect::<Vec<_>>();
    admit_dictation_witness_payload(
        state,
        ev_tx,
        &identity,
        &payload,
        &pending_owners,
        &[],
        threshold_samples,
    );
    for owner in pending_owners {
        if !state.dictation_witness_heard.contains(&owner) {
            state.dictation_witness_heard.push(owner);
        }
    }
    finish_dictation_witness_frontier(state, ev_tx);
    state.dictation_witness_inflight = false;
    resume_witness_waiting_closures(state, ev_tx);
}

fn resume_witness_waiting_closures(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
) {
    let waiting = std::mem::take(&mut state.dictation_witness_waiting);
    if state.capture_stopping {
        {
            let mut ledger = state
                .acoustic_ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for owner in waiting {
                ledger.note_frontier_return(&owner, LedgerObservationProducer::Whisper);
            }
        }
        state.close_admission_horizon(ev_tx, state.admission_horizon);
    } else {
        for owner in waiting {
            state.finish_whisper_frontier(ev_tx, &owner);
        }
        state.close_admission_horizon(ev_tx, state.window_plan.admission_horizon());
        state.publish_resolved_word_seals(ev_tx);
    }
}

/// Return only observers reserved by this concrete job, on success or failure.
fn finish_dictation_witness_frontier(
    state: &mut AppleSealState,
    _ev_tx: &mpsc::UnboundedSender<EngineEvent>,
) {
    let owners = std::mem::take(&mut state.dictation_witness_owners);
    let mut ledger = state
        .acoustic_ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for owner in owners {
        ledger.note_frontier_return(&owner, LedgerObservationProducer::AppleDictation);
    }
}

/// Run the bounded terminal DictationTranscriber witness over the take's
/// remaining unresolved acoustic debt: committed owners whose text recovery is
/// still pending, and material measured-speech gaps no qualified owner covers.
///
/// The witness reads only the recorder-owned terminal PCM on this take's
/// session/epoch clock, decodes it through the bridge's pinned DT lane, and
/// hands the result to the ordinary ledger admission corridors:
///
/// - a single-word segment whose midpoint sits inside an unsealed
///   recovery-pending owner is offered to that owner as a measured word pin;
/// - a segment wholly contained in a material gap is admitted through the
///   same containment path a retained final pass uses;
/// - everything else stays diagnostic payload text and never mutates the
///   document. The witness never replaces a whole take and never reopens a
///   sealed occurrence.
///
/// Every request is deduplicated by its acoustic window, executed under the
/// DictationTranscriber bridge lock, bounded per request and in total. An unarmed
/// lane, a missing archive, a bridge error or a timeout leaves the debt
/// exactly as it was — the coverage receipt recomputed after this pass keeps
/// reporting the gap.
fn run_terminal_dictation_witness(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    language: Option<&str>,
    witness_done: &std_mpsc::Receiver<DictationWitnessReturn>,
) {
    state.dictation_witness_live_closed = true;
    if !state.dictation_witness_armed {
        return;
    }
    let budget_start = Instant::now();
    // Live witness answers that already returned land first; the terminal
    // target picture below then sees the debt they resolved.
    drain_dictation_witness_returns(state, ev_tx, witness_done);
    if state.dictation_witness_inflight {
        let remaining = DICTATION_WITNESS_TOTAL_BUDGET.saturating_sub(budget_start.elapsed());
        match witness_done.recv_timeout(DICTATION_WITNESS_REQUEST_TIMEOUT.min(remaining)) {
            Ok(returned) => handle_dictation_witness_return(state, ev_tx, returned),
            Err(_) => {
                state.dictation_witness_inflight = false;
                let _ = ev_tx.send(EngineEvent::Warning {
                    code: "dictation_witness_terminal_boundary".into(),
                    message: "bounded live witness did not return; terminal targets remain open"
                        .into(),
                });
                finish_dictation_witness_frontier(state, ev_tx);
                resume_witness_waiting_closures(state, ev_tx);
            }
        }
    }
    let threshold_samples =
        u64::from(state.sample_rate).saturating_mul(SEAL_COVERAGE_INCOMPLETE_MS) / 1_000;
    let (mut pending_owners, gaps) = witness_recovery_targets(state, threshold_samples);
    // The same PCM decodes to the same answer: an owner a live answer already
    // heard is not decoded again on the Stop path.
    pending_owners.retain(|owner| !state.dictation_witness_heard.contains(owner));
    if pending_owners.is_empty() && gaps.is_empty() {
        return;
    }
    let capture_samples = state.audio.session_sample_end();
    if state.terminal_pcm.is_none() {
        let _ = ev_tx.send(EngineEvent::Warning {
            code: "dictation_witness_pcm_unavailable".into(),
            message: "terminal archive PCM is unavailable; witness targets stay unresolved".into(),
        });
        return;
    }

    // Witness targets on the capture clock: pending-recovery owners and
    // material gaps, deduplicated and ordered. Two targets naming the same
    // PCM are one question, not two witnesses.
    let mut targets: Vec<(u64, u64)> = pending_owners
        .iter()
        .map(|owner| (owner.sample_start, owner.sample_end))
        .chain(
            gaps.iter()
                .map(|range| (range.sample_start, range.sample_end)),
        )
        .filter(|(start, end)| *end > *start && *start < capture_samples)
        .map(|(start, end)| (start, end.min(capture_samples)))
        .collect();
    targets.sort_unstable();
    targets.dedup();

    let context = u64::from(state.sample_rate).saturating_mul(DICTATION_WITNESS_CONTEXT_SECS);
    let max_window = u64::from(state.sample_rate).saturating_mul(DICTATION_WITNESS_MAX_WINDOW_SECS);
    let windows = terminal_witness_windows(&targets, capture_samples, context, max_window);

    let mut requests_issued = 0usize;
    for (ordinal, (window_start, window_end)) in windows.iter().copied().enumerate() {
        // The total budget binds every request, including the one in flight:
        // its deadline is clipped to what the budget still owns, so a late
        // request cannot overrun the terminal phase past the declared cap.
        let remaining = DICTATION_WITNESS_TOTAL_BUDGET.saturating_sub(budget_start.elapsed());
        if requests_issued >= DICTATION_WITNESS_MAX_REQUESTS || remaining < Duration::from_secs(1) {
            let _ = ev_tx.send(EngineEvent::Warning {
                code: "dictation_witness_budget_exhausted".into(),
                message: format!(
                    "{} witness window(s) stay unresolved beyond the terminal budget",
                    windows.len() - ordinal
                ),
            });
            break;
        }
        let request_timeout = DICTATION_WITNESS_REQUEST_TIMEOUT.min(remaining);
        let Some(window) = state
            .terminal_pcm
            .as_ref()
            .and_then(|pcm| pcm.window(window_start, window_end))
        else {
            let _ = ev_tx.send(EngineEvent::Warning {
                code: "dictation_witness_pcm_unavailable".into(),
                message: format!("window {window_start}..{window_end} is not in the owned archive"),
            });
            continue;
        };
        let identity = TailRequestIdentity {
            request_id: DICTATION_WITNESS_REQUEST_BASE | ordinal as u64,
            range: TailSampleRange {
                session: state.session_id.clone(),
                capture_epoch: state.capture_epoch,
                sample_start: window.sample_start,
                sample_end: window.sample_end,
            },
        };
        requests_issued = requests_issued.saturating_add(1);
        let payload = match crate::stt::apple_stt::transcribe_dictation_witness(
            &identity,
            &window.samples,
            state.sample_rate,
            language,
            request_timeout,
        ) {
            Ok(payload) => payload,
            Err(error) => {
                let _ = ev_tx.send(EngineEvent::Warning {
                    code: "dictation_witness_unavailable".into(),
                    message: format!(
                        "window {}..{}: {error:#}",
                        window.sample_start, window.sample_end
                    ),
                });
                continue;
            }
        };
        info!(
            session = %state.session_id,
            capture_epoch = state.capture_epoch,
            request_id = identity.request_id,
            sample_start = identity.range.sample_start,
            sample_end = identity.range.sample_end,
            segments = payload.segments.len(),
            elapsed_ms = payload.elapsed_ms,
            "dictation_witness_return"
        );
        admit_dictation_witness_payload(
            state,
            ev_tx,
            &identity,
            &payload,
            &pending_owners,
            &gaps,
            threshold_samples,
        );
    }
}

/// Hand measured witness segments to the existing word admission owner.
/// Existing unsealed slots are targets for independent lexical adjudication,
/// not replay refusals. Gap segments retain their containment contract.
fn admit_dictation_witness_payload(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    identity: &TailRequestIdentity,
    payload: &TailProviderPayload,
    pending_owners: &[OccurrenceIdentity],
    gaps: &[TailSampleRange],
    threshold_samples: u64,
) {
    if !state.apple_live_enabled || !state.seal_ledger_enabled {
        return;
    }
    let decode_window = (identity.range.sample_start, identity.range.sample_end);
    if payload.validate().is_err() || payload.identity != *identity {
        return;
    }
    state.dictation_witness_payload = Some(payload.clone());
    state
        .acoustic_ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .bind_word_provider(payload);
    for owner in pending_owners {
        if owner.sample_end <= identity.range.sample_start
            || owner.sample_start >= identity.range.sample_end
        {
            continue;
        }
        if state
            .acoustic_ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_sealed(owner)
        {
            continue;
        }
        // Preserve the returned grain. A phrase remains one measured group;
        // the ledger alone decides whether it targets an existing partition.
        let mut pins: Vec<RoutedPin> = Vec::new();
        for (index, segment) in payload.segments.iter().enumerate() {
            let midpoint = segment
                .range
                .sample_start
                .saturating_add(segment.range.sample_len().unwrap_or(0) / 2);
            if !(owner.sample_start <= midpoint && midpoint < owner.sample_end) {
                continue;
            }
            pins.push(RoutedPin {
                index,
                pin: OccurrenceIdentity::from(&segment.range),
                text: segment.text.clone(),
                confidence: segment.confidence,
            });
        }
        let _ = state.admit_routed_words_with_decode_window(
            ev_tx,
            0,
            owner,
            (identity.request_id, Some(decode_window)),
            RoutedWords {
                pins: &pins,
                neighbours: &[],
            },
            LedgerObservationProducer::AppleDictation,
        );
    }
    admit_full_pass_gap_segments(
        state,
        ev_tx,
        payload,
        gaps,
        threshold_samples,
        EnergyAdmission::QualifyFinalPassGap,
    );
    let pending = std::mem::take(&mut state.pending_untimed_finals);
    for (phrase_id, raw, start, end) in pending {
        if let Some(segments) = witness_timed_segments(state, &raw, start, end) {
            let timed = apple_segments_on_pcm_clock(state, &segments);
            let audio_secs = end as f32 / state.sample_rate.max(1) as f32;
            let _ = seal_utterance_final(state, ev_tx, &raw, segments, audio_secs);
            // Dispatch success is not admission. Retire refused diagnostics
            // only when every borrowed range has a committed lexical source.
            let admitted = {
                let ledger = state
                    .acoustic_ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                !timed.is_empty()
                    && timed.iter().all(|segment| {
                        let pin = OccurrenceIdentity::from(&segment.range);
                        ledger
                            .occurrences()
                            .filter(|owner| owner.same_capture(&pin))
                            .any(|owner| {
                                ledger.matching_word_slot(owner, &pin, &segment.text, false)
                                    || ledger.slots_of(owner).unwrap_or(&[]).iter().any(|slot| {
                                        slot.sample_start == pin.sample_start
                                            && slot.sample_end == pin.sample_end
                                            && slot.text.split_whitespace().collect::<Vec<_>>()
                                                == segment
                                                    .text
                                                    .split_whitespace()
                                                    .collect::<Vec<_>>()
                                    })
                            })
                    })
            };
            if admitted {
                state.refused_untimed_words.remove(&phrase_id);
            } else {
                state
                    .pending_untimed_finals
                    .push((phrase_id, raw, start, end));
            }
        } else {
            state
                .pending_untimed_finals
                .push((phrase_id, raw, start, end));
        }
    }
    state.publish_unadmitted_words(ev_tx);
}

/// Hand one sealed occurrence's committed label to the formatter as a text
/// job. Formatting is not an acoustic observer: it never joins the ledger
/// frontier, so a slow, failed or absent formatter cannot hold this seal, a
/// later seal or Stop. Each sealed label is requested at most once, and only
/// when bounded transport ownership is already held. Configuration intent and
/// queue availability alone never change the ledger.
fn schedule_formatter_for_sealed_label(
    ledger: &AcousticLedger,
    formatter: Option<&mpsc::Sender<FormatterRequest>>,
    requested: &mut BTreeSet<OccurrenceIdentity>,
    occurrence: &OccurrenceIdentity,
) -> bool {
    let Some(formatter) = formatter else {
        return false;
    };
    if !ledger.is_sealed(occurrence) || requested.contains(occurrence) {
        return false;
    }
    let Some(existing_label) = ledger
        .text_of(occurrence)
        .filter(|label| !label.trim().is_empty())
        .map(str::to_owned)
    else {
        return false;
    };
    // The derived result is bound to the last mutation that produced this
    // committed label; without that source the emitter could not prove CAS.
    let Some(source_observation) = ledger
        .layer_trail_for(occurrence)
        .filter(|decision| decision.decision.grants_mutation())
        .last()
        .map(|decision| decision.observation.clone())
    else {
        return false;
    };
    let Ok(permit) = formatter.try_reserve() else {
        return false;
    };
    requested.insert(occurrence.clone());
    permit.send(FormatterRequest {
        occurrence: occurrence.clone(),
        existing_label,
        source_observation: Some(source_observation),
        // The selected immutable generation supplies the policy before execution.
        policy: FormattingPolicy::Off,
    });
    true
}

/// Original sample geometry this capture still owns. A future or evicted span
/// stays refused: clamping it onto the buffer is not ownership.
fn segment_owns_exact_pcm(state: &AppleSealState, start_ts: f32, end_ts: f32) -> bool {
    if !start_ts.is_finite() || !end_ts.is_finite() || end_ts <= start_ts {
        return false;
    }
    let rate = f64::from(state.sample_rate.max(1));
    let start = f64::from(start_ts) * rate;
    let end = f64::from(end_ts) * rate;
    if !start.is_finite()
        || !end.is_finite()
        || start < 0.0
        || end < 0.0
        || start > u64::MAX as f64
        || end > u64::MAX as f64
    {
        return false;
    }
    let sample_start = start.round() as u64;
    let sample_end = end.round() as u64;
    if sample_end <= sample_start {
        return false;
    }
    state
        .window_by_samples(sample_start, sample_end)
        .is_some_and(|window| !window.samples.is_empty())
}

/// Seal one Apple utterance: run the shared lexicon + cleanup pass, then emit
/// `UtteranceFinal`. Returns `false` when postprocess filtered the text to
/// empty; an explicit `Drop` event is emitted instead of an empty final.
///
/// `raw_text` keeps the uncorrected engine output so the quality loop can see
/// exactly what the lexicon rewrote.
fn seal_utterance_final(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    raw: &str,
    segments: Vec<TranscriptSegment>,
    audio_secs: f32,
) -> bool {
    const BOUNDARY_EPSILON_SECS: f32 = 0.002;

    // The physical lane reconciles every timed candidate, including revised
    // earlier coordinates. The legacy cursor must not discard them first.
    if state.fusion_seal_armed {
        if segments.is_empty() {
            let _ = ev_tx.send(EngineEvent::Warning {
                code: "apple_final_without_pcm_timing".into(),
                message: "untimed Apple text cannot create an occurrence".into(),
            });
            state.publish_unadmitted_words(ev_tx);
            return false;
        }
        seal_sliced_by_silero(state, ev_tx, &segments);
        return true;
    }
    let callback_text = raw.trim().to_string();
    let original_segment_count = segments.len();
    let mut disjoint = Vec::with_capacity(original_segment_count);
    let mut cursor = state.last_apple_segment_end;
    let mut overlap_normalized = false;
    let mut refused_without_exact_pcm = false;

    for mut segment in segments {
        let text = segment.text.trim();
        if text.is_empty()
            || !segment.start_ts.is_finite()
            || !segment.end_ts.is_finite()
            || segment.end_ts <= segment.start_ts
        {
            continue;
        }
        if segment.end_ts <= cursor + BOUNDARY_EPSILON_SECS
            || segment.start_ts < cursor - BOUNDARY_EPSILON_SECS
        {
            overlap_normalized = true;
            continue;
        }
        if segment.start_ts < cursor {
            segment.start_ts = cursor;
        }
        // Jitter may lift the start onto the accepted boundary. The end stays
        // on the callback's original clock. Missing exact PCM is diagnostic
        // only: it must not move this cursor or the accepted Apple boundary.
        if !segment_owns_exact_pcm(state, segment.start_ts, segment.end_ts) {
            if !refused_without_exact_pcm {
                info!(
                    start_ts = segment.start_ts,
                    end_ts = segment.end_ts,
                    "apple_lifecycle: timed callback kept diagnostic; exact owned pcm is absent"
                );
                refused_without_exact_pcm = true;
            }
            continue;
        }
        segment.text = text.to_string();
        cursor = segment.end_ts;
        disjoint.push(segment);
    }

    if overlap_normalized {
        let _ = ev_tx.send(EngineEvent::Warning {
            code: APPLE_FINAL_OVERLAP_WARNING_CODE.to_string(),
            message: "Apple final overlap removed at segment boundary".to_string(),
        });
    }

    if disjoint.is_empty() {
        if callback_text.is_empty() || refused_without_exact_pcm {
            // Timed spans that lack their original PCM must not become a
            // synthesized occurrence, and must not consume an utterance id.
            return false;
        }
        // A segment-less final still names new session-clock audio. Preserve it
        // as one occurrence; text is payload and never a deduplication key.
        let start_ts = state.last_apple_segment_end;
        let end_ts = audio_secs.max(start_ts + BOUNDARY_EPSILON_SECS);
        info!(
            audio_secs,
            synthesized_start = start_ts,
            synthesized_end = end_ts,
            text_chars = callback_text.chars().count(),
            "apple_lifecycle: segment-less final bound to synthesized window"
        );
        disjoint.push(TranscriptSegment {
            text: callback_text.clone(),
            start_ts,
            end_ts,
            // A synthesized window is not a measured segment; no metric (A6).
            confidence: None,
        });
    }

    let start_ts = disjoint.first().map_or(0.0, |segment| segment.start_ts);
    let end_ts = disjoint.last().map_or(start_ts, |segment| segment.end_ts);
    let raw_text = if !overlap_normalized && disjoint.len() == original_segment_count {
        callback_text
    } else {
        disjoint
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    };
    if raw_text.is_empty() {
        return false;
    }

    // Consume the Apple boundary even if cleanup filters the text. A later
    // cumulative callback must not resurrect audio the product already judged.
    // `end_ts` is the last span that still owns its original PCM, so a refused
    // future or evicted sibling cannot poison this boundary.
    state.last_apple_segment_end = end_ts;

    // The seal path knows exactly how many acoustic spans this text covers, so
    // the repetition cleanup is told rather than left to guess. A run of
    // identical words with one span per copy is speech; only a run longer than
    // the audio can account for is a decoder loop.
    let after_lexicon = crate::quality::overlay_quality::apply_custom_lexicon(raw_text.trim());
    let apple_words = apple_segments_on_pcm_clock(state, &disjoint);
    let request_id = state.utterance_id.saturating_add(1);
    let captured_end = state.audio.session_sample_end();
    let span_sample_start = apple_words.first().map_or_else(
        || seconds_to_captured_sample(start_ts, state.sample_rate, captured_end),
        |word| word.range.sample_start,
    );
    let span_sample_end = apple_words
        .last()
        .map_or_else(
            || seconds_to_captured_sample(end_ts, state.sample_rate, captured_end),
            |word| word.range.sample_end,
        )
        .max(span_sample_start);
    // Bind this span to the spectrum even off the sliced path: when a Silero
    // edge already encloses every sample Apple claimed, the utterance range is
    // the canonical one and the span records which identity it came from. No
    // enclosing edge (Silero off, model missing, an edge still open, or a span
    // that straddles two utterances) leaves the Apple-derived range untouched —
    // binding never costs content.
    let apple_range = TailSampleRange {
        session: state.session_id.clone(),
        capture_epoch: state.capture_epoch,
        sample_start: span_sample_start,
        sample_end: span_sample_end,
    };
    let (span_range, silero_bound) = match state
        .fusion
        .as_ref()
        .filter(|_| state.fusion_seal_armed)
        .and_then(|fusion| {
            fusion
                .ledger()
                .utterance_enclosing(span_sample_start, span_sample_end)
                .filter(|utterance| utterance.closed)
        }) {
        Some(utterance) => (utterance.range.clone(), true),
        None => (apple_range, false),
    };
    let ledger_occurrence = OccurrenceIdentity::from(&span_range);
    let apple_admitted = admit_ledger_label(
        state,
        ev_tx,
        LabelAdmission {
            observation: LedgerObservationIdentity::new(
                LedgerObservationProducer::Apple,
                request_id,
                0,
                ledger_occurrence.clone(),
            ),
            label: &raw_text,
            energy: if silero_bound {
                EnergyAdmission::QualifyFromOwnedPcm
            } else {
                EnergyAdmission::RequireExistingQualification
            },
        },
    );
    // One id space. While the seal path can mint span ids FROM the ledger, the
    // fallback must burn its id there too, or Silero would later mint the same
    // id for a real utterance — `note_apple_commit_timed` is idempotent on id,
    // so the collision would silently merge two unrelated spans. With the seal
    // path disarmed no ledger id ever becomes a span id, and the counter stays
    // the plain monotonic one the Apple-boundary lane always used.
    let utterance_id = match state.fusion.as_mut().filter(|_| state.fusion_seal_armed) {
        Some(fusion) => fusion.ledger_mut().reserve_id(),
        None => state.utterance_id.saturating_add(1),
    };
    state.utterance_id = state.utterance_id.max(utterance_id);
    let committed_text = after_lexicon.clone();
    state.pending_events.insert(
        utterance_id,
        PendingAppleSeal {
            occurrence: ledger_occurrence.clone(),
            raw_text,
            layer1_baseline: committed_text.clone(),
            start_ts,
            end_ts,
            segments: disjoint,
        },
    );

    if apple_admitted.is_some() {
        state.register_whisper_owner(ev_tx, utterance_id, &ledger_occurrence);
    }

    if apple_admitted.is_some() {
        let _ = admit_ledger_label(
            state,
            ev_tx,
            LabelAdmission {
                observation: LedgerObservationIdentity::new(
                    LedgerObservationProducer::Lexicon,
                    request_id,
                    0,
                    ledger_occurrence,
                ),
                label: &after_lexicon,
                energy: EnergyAdmission::RequireExistingQualification,
            },
        );
    }

    state.emit_pending_seal(ev_tx, utterance_id);
    state.publish_unadmitted_words(ev_tx);
    true
}

// ═══════════════════════════════════════════════════════════
// Engine lifecycle: speech epochs (hands-free silence)
// ═══════════════════════════════════════════════════════════

/// Audio replayed into a fresh epoch ahead of the detected speech edge, so the
/// first phoneme is not eaten by bridge spin-up (~0.24 s measured). Same value
/// the fusion lane pads windows with.
const EPOCH_PREROLL_SECS: f32 = super::silero_fusion::DEFAULT_LEFT_PAD_SECS;

/// Lift one poll's worth of bridge events onto the session PCM clock.
///
/// Bridge time is **per request**: every `LiveStreamSession` restarts its
/// segment clock at zero, while every consumer downstream
/// ([`apple_segments_on_pcm_clock`], the seal windows, the Layer 1 ranges)
/// reads those seconds as session time. With one stream per take the two
/// clocks coincide and this is the identity; with an epoch lifecycle they
/// diverge by exactly the epoch base, so the shift happens once, here, before
/// any event reaches [`emit_stream_events`].
fn shift_events(events: Vec<LiveStreamEvent>, base_secs: f32) -> Vec<LiveStreamEvent> {
    if !base_secs.is_finite() || base_secs <= 0.0 {
        return events;
    }
    events
        .into_iter()
        .map(|event| match event {
            LiveStreamEvent::Partial { text, segments } => LiveStreamEvent::Partial {
                text,
                segments: shift_segments(segments, base_secs),
            },
            LiveStreamEvent::PhraseFinal { text, segments } => LiveStreamEvent::PhraseFinal {
                text,
                segments: shift_segments(segments, base_secs),
            },
            LiveStreamEvent::Summary {
                text,
                segments,
                ok,
                error,
            } => LiveStreamEvent::Summary {
                text,
                segments: shift_segments(segments, base_secs),
                ok,
                error,
            },
            other @ (LiveStreamEvent::Ready
            | LiveStreamEvent::End
            | LiveStreamEvent::Error { .. }) => other,
        })
        .collect()
}

fn shift_segments(segments: Vec<TranscriptSegment>, base_secs: f32) -> Vec<TranscriptSegment> {
    segments
        .into_iter()
        .map(|mut segment| {
            if segment.start_ts.is_finite() {
                segment.start_ts += base_secs;
            }
            if segment.end_ts.is_finite() {
                segment.end_ts += base_secs;
            }
            segment
        })
        .collect()
}

/// What the worker must do with one capture chunk under the epoch lifecycle.
#[derive(Debug, Clone, Copy, PartialEq)]
enum EpochDecision {
    /// Write the chunk into the currently open stream.
    Forward,
    /// Speech edge while asleep: open a stream based at `preroll_from`.
    Wake { preroll_from: u64 },
    /// Silence threshold crossed: close the epoch. Chunk is trailing silence.
    Sleep { silence_secs: f32 },
    /// Asleep and still silent — retain audio only.
    Idle,
}

/// Engine lifecycle for the Apple progressive lane: speech opens an SFSpeech
/// epoch, silence past the product threshold closes it, and the engine rests
/// (mic + Silero keep running) until the next speech edge.
///
/// Disarmed (`utterance_silence_sec: None`, or no Silero) it answers
/// [`EpochDecision::Forward`] to everything, which is the pre-epoch worker bit
/// for bit.
///
/// # The gate observes nothing itself
///
/// It is a pure state machine over one bit per chunk — `speech_live` — supplied
/// by [`SileroIngress::ingest`], the session's single VAD. It used to own a
/// second `SpeechSession` of its own, which meant two Silero instances scoring
/// the same PCM: the lifecycle woke and slept on one set of edges while the
/// fusion ledger minted utterance identity on another, and nothing kept the two
/// spectra in step. One session, one spectrum, one set of boundaries.
///
/// Speech is "live" while a Supervisor segment is open, and for the chunk a
/// segment closes in — so the silence counter starts at the segment close, i.e.
/// **after** Silero's own hysteresis (`0.55 s` by default) has already elapsed.
/// The wall silence before an epoch closes is therefore the product threshold
/// plus that hysteresis, never less than the setting.
struct EpochGate {
    armed: bool,
    sample_rate: u32,
    silence_threshold_samples: u64,
    preroll_samples: u64,
    awake: bool,
    /// Session cursor of the last chunk speech was live in.
    last_speech_sample: u64,
    /// Session cursor the previous epoch closed at — the pre-roll floor, so a
    /// new epoch never re-feeds audio the closed one already carried.
    epoch_closed_at: u64,
}

impl EpochGate {
    /// Legacy lane: one stream for the whole take.
    fn disarmed() -> Self {
        Self {
            armed: false,
            sample_rate: 1,
            silence_threshold_samples: 0,
            preroll_samples: 0,
            awake: false,
            last_speech_sample: 0,
            epoch_closed_at: 0,
        }
    }

    fn armed(sample_rate: u32, silence_sec: f32) -> Self {
        let rate = sample_rate.max(1);
        Self {
            armed: true,
            sample_rate: rate,
            silence_threshold_samples: (silence_sec.max(0.1) * rate as f32) as u64,
            preroll_samples: (EPOCH_PREROLL_SECS * rate as f32) as u64,
            awake: false,
            last_speech_sample: 0,
            epoch_closed_at: 0,
        }
    }

    /// Build the gate the session config asks for. No silence setting → legacy;
    /// no session Silero → legacy, because without edges an armed gate would
    /// rest forever and the take would be silent (fail open).
    fn for_session(
        sample_rate: u32,
        utterance_silence_sec: Option<f32>,
        speech_edges_available: bool,
    ) -> Self {
        let Some(silence_sec) = utterance_silence_sec else {
            return Self::disarmed();
        };
        if !speech_edges_available {
            warn!(
                utterance_silence_sec = silence_sec,
                "Silero unavailable — Apple engine lifecycle disarmed, falling back to one \
                 continuous stream for this session"
            );
            return Self::disarmed();
        }
        Self::armed(sample_rate, silence_sec)
    }

    fn is_armed(&self) -> bool {
        self.armed
    }

    /// One chunk. `speech_live` is the session Silero's verdict on it — the
    /// same observation the utterance ledger was minted from.
    fn feed_pcm(&mut self, samples: &[f32], samples_seen: u64, speech_live: bool) -> EpochDecision {
        if !self.armed {
            return EpochDecision::Forward;
        }
        let chunk_start = samples_seen.saturating_sub(samples.len() as u64);
        if speech_live {
            self.last_speech_sample = samples_seen;
            if self.awake {
                return EpochDecision::Forward;
            }
            self.awake = true;
            let preroll_from = chunk_start
                .saturating_sub(self.preroll_samples)
                .max(self.epoch_closed_at);
            return EpochDecision::Wake { preroll_from };
        }
        if !self.awake {
            return EpochDecision::Idle;
        }
        let silence = samples_seen.saturating_sub(self.last_speech_sample);
        if silence >= self.silence_threshold_samples {
            self.awake = false;
            self.epoch_closed_at = samples_seen;
            return EpochDecision::Sleep {
                silence_secs: silence as f32 / self.sample_rate as f32,
            };
        }
        EpochDecision::Forward
    }
}

/// Session-time base of the open epoch, in seconds.
fn epoch_base_secs(epoch_base_samples: u64, sample_rate: u32) -> f32 {
    epoch_base_samples as f32 / sample_rate.max(1) as f32
}

/// Borrow only measured segments on this open phrase's capture scope.
/// Equality corroborates the label after PCM selects the phrase; it never
/// locates a word or creates acoustic timing.
fn witness_timed_segments(
    state: &AppleSealState,
    raw: &str,
    start: u64,
    end: u64,
) -> Option<Vec<TranscriptSegment>> {
    let payload = state.dictation_witness_payload.as_ref()?;
    if start >= end
        || payload.identity.range.session != state.session_id
        || payload.identity.range.capture_epoch != state.capture_epoch
        || payload.identity.range.sample_start > start
        || payload.identity.range.sample_end < end
    {
        return None;
    }
    let timed = payload
        .segments
        .iter()
        .filter(|segment| segment.range.sample_start >= start && segment.range.sample_end <= end)
        .collect::<Vec<_>>();
    let witnessed = timed
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    if timed.is_empty()
        || witnessed.split_whitespace().collect::<Vec<_>>()
            != raw.split_whitespace().collect::<Vec<_>>()
    {
        return None;
    }
    let rate = state.sample_rate.max(1) as f32;
    Some(
        timed
            .into_iter()
            .map(|segment| TranscriptSegment {
                text: segment.text.clone(),
                start_ts: segment.range.sample_start as f32 / rate,
                end_ts: segment.range.sample_end as f32 / rate,
                confidence: segment.confidence,
            })
            .collect(),
    )
}

/// Close one phrase and publish the result of this exact seal call. Internal
/// mirror events are folded into the final replacement after ledger events.
fn close_apple_phrase(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    phrase_id: u64,
    raw: &str,
    segments: Vec<TranscriptSegment>,
    audio_secs: f32,
    closes_open: bool,
) -> (bool, Vec<EngineEvent>) {
    // Every caller has already cleared the open partial; its word clock ends
    // with it, so the next phrase cannot inherit stale arrivals.
    let segments = if segments.is_empty() {
        let end = seconds_to_captured_sample(
            audio_secs,
            state.sample_rate,
            state.audio.session_sample_end(),
        );
        witness_timed_segments(state, raw, state.open_partial_pin, end).unwrap_or(segments)
    } else {
        segments
    };
    let timed = apple_segments_on_pcm_clock(state, &segments);
    let (seal_tx, mut seal_rx) = mpsc::unbounded_channel();
    let committed = seal_utterance_final(state, &seal_tx, raw, segments, audio_secs);
    let events = std::iter::from_fn(|| seal_rx.try_recv().ok()).collect::<Vec<_>>();
    // The admission owner deliberately emits nothing for an identical,
    // reconciled callback. Without an open phrase this is not another close.
    if events.is_empty() && !closes_open {
        return (committed, events);
    }
    // The next phrase is spoken after this one's last timed word. Committed
    // words before that point belong to this phrase and can never be counted
    // as speech the next partial restates. Untimed segments sit at zero and
    // leave the floor where it was.
    let untimed = events.iter().any(|event| {
        matches!(event,
        EngineEvent::Warning { code, .. } if code == "apple_final_without_pcm_timing")
    });
    let mut outcomes = BTreeMap::new();
    if untimed {
        let end = seconds_to_captured_sample(
            audio_secs,
            state.sample_rate,
            state.audio.session_sample_end(),
        );
        if state.open_partial_pin < end
            && !state
                .pending_untimed_finals
                .iter()
                .any(|(id, _, _, _)| *id == phrase_id)
            && state.pending_untimed_finals.len() < 64
        {
            state.pending_untimed_finals.push((
                phrase_id,
                raw.to_owned(),
                state.open_partial_pin,
                end,
            ));
        }
        outcomes.insert(ApplePhraseOutcome::Untimed, raw.split_whitespace().count());
        state.refused_untimed_words.insert(
            phrase_id,
            raw.split_whitespace()
                .map(|text| UnadmittedAppleWord {
                    text: text.into(),
                    sample_start: state.open_partial_pin,
                    sample_end: state.open_partial_pin,
                    source: UnadmittedAppleWordSource::RefusedUntimed { phrase_id },
                })
                .collect(),
        );
    } else {
        if !timed.is_empty() {
            state.refused_untimed_words.remove(&phrase_id);
        }
        let mut remaining = timed
            .iter()
            .map(|word| word.text.split_whitespace().count())
            .collect::<Vec<_>>();
        for (word, count) in timed.iter().zip(&mut remaining) {
            let held = |candidate: &FusionWord| {
                candidate.sample_start == word.range.sample_start
                    && candidate.sample_end == word.range.sample_end
            };
            let outcome = if state.pending_silero_words.values().flatten().any(held) {
                Some(ApplePhraseOutcome::Pending)
            } else if state.unmatched_silero_words.iter().any(held) {
                Some(ApplePhraseOutcome::Unmatched)
            } else {
                None
            };
            if let Some(outcome) = outcome {
                *outcomes.entry(outcome).or_default() += *count;
                *count = 0;
            }
        }
        // A late mixed callback can emit replay receipts for individual words
        // and then one admitted label for the whole owner. Count its specific
        // results first, so that whole-owner label cannot count the old words
        // again. All budgets are bounded by this closing call's own input.
        for mutations in [false, true] {
            for event in &events {
                let EngineEvent::LedgerMutation {
                    observation,
                    label,
                    receipt,
                } = event
                else {
                    continue;
                };
                if observation.producer != LedgerObservationProducer::Apple {
                    continue;
                }
                let outcome = match receipt {
                    MutationReceipt::Insert { .. }
                    | MutationReceipt::Correct { .. }
                    | MutationReceipt::Preserve { .. } => ApplePhraseOutcome::Admitted,
                    MutationReceipt::KeepVisibleUnanchored { .. } => ApplePhraseOutcome::Unmatched,
                    MutationReceipt::Refuse {
                        reason:
                            RefuseReason::SealedReplay
                            | RefuseReason::AuthorityConflict
                            | RefuseReason::BatchDuplicate
                            | RefuseReason::ReplayedRangeIdentity,
                        ..
                    } => ApplePhraseOutcome::Replay,
                    MutationReceipt::Refuse { .. } => ApplePhraseOutcome::NoChange,
                };
                if (outcome == ApplePhraseOutcome::Admitted) != mutations {
                    continue;
                }
                let mut budget = label.split_whitespace().count();
                let range = &observation.occurrence;
                for (word, count) in timed.iter().zip(&mut remaining) {
                    let midpoint = word.range.sample_start
                        + word
                            .range
                            .sample_end
                            .saturating_sub(word.range.sample_start)
                            / 2;
                    if range.sample_start <= midpoint && midpoint < range.sample_end {
                        let accounted = (*count).min(budget);
                        if accounted > 0 {
                            *outcomes.entry(outcome).or_default() += accounted;
                            *count -= accounted;
                            budget -= accounted;
                        }
                    }
                }
            }
        }
        let unchanged: usize = remaining.iter().sum();
        if unchanged > 0 {
            outcomes.insert(
                ApplePhraseOutcome::NoChange,
                unchanged
                    + outcomes
                        .get(&ApplePhraseOutcome::NoChange)
                        .copied()
                        .unwrap_or(0),
            );
        }
        if outcomes.is_empty() {
            // An empty receipt set is no_change, never evidence of admission.
            outcomes.insert(ApplePhraseOutcome::NoChange, raw.split_whitespace().count());
        }
    }
    let arrival_index = state
        .closed_phrases
        .get(&phrase_id)
        .map_or(state.closed_phrases.len(), |phrase| phrase.arrival_index);
    let was_untimed = untimed
        || state
            .closed_phrases
            .get(&phrase_id)
            .is_some_and(|phrase| phrase.was_untimed);
    state.closed_phrases.insert(
        phrase_id,
        ClosedApplePhrase {
            arrival_index,
            outcomes,
            was_untimed,
        },
    );
    state.last_closed_phrase_id = Some(phrase_id);
    if phrase_id == state.phrase_id {
        state.phrase_id = state.phrase_id.saturating_add(1);
    }
    for event in &events {
        if !matches!(event, EngineEvent::UnadmittedAppleWords { .. }) {
            let _ = ev_tx.send(event.clone());
        }
    }
    state.publish_unadmitted_words(ev_tx);
    // A committed seal closes this phrase's PCM; outstanding debt on it may be
    // re-asked in the background without holding the next utterance.
    if committed {
        let sealed_end = timed
            .iter()
            .map(|word| word.range.sample_end)
            .max()
            .unwrap_or(0);
        maybe_spawn_utterance_witness(state, ev_tx, sealed_end);
    }
    (committed, events)
}

/// Seal an open partial that never received a phrase final.
///
/// Shared by the two places a stream can end without one: capture EOF (stop
/// mid-phrase) and an epoch close. Both must run the same seal-time correction
/// — a phrase that ends by silence must not be the one route that commits
/// uncorrected text, or dies in the preview lane.
fn seal_open_partial(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    audio_secs: f32,
) {
    if !state.seal_ledger_enabled {
        if state.direct_producer == LedgerObservationProducer::Apple
            && !state.open_partial.is_empty()
        {
            let text = std::mem::take(&mut state.open_partial);
            let segments = std::mem::take(&mut state.open_partial_segments);
            emit_direct_apple_events(
                std::iter::once(LiveStreamEvent::PhraseFinal { text, segments }),
                ev_tx,
                state,
            );
        }
        return;
    }
    let open = state.open_partial.trim().to_string();
    if open.is_empty() {
        return;
    }
    let segments = std::mem::take(&mut state.open_partial_segments);
    state.open_partial.clear();
    let phrase_id = state.phrase_id;
    close_apple_phrase(state, ev_tx, phrase_id, &open, segments, audio_secs, true);
    state.finishing_phrase_id = Some(phrase_id);
}

/// Finish the recognizer after the worker's stop seal. The async arm drains
/// admission events before forwarding this acknowledgement to the controller.
fn finish_capture_after_seal(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    audio_secs: f32,
    last_window_closed: tokio::sync::oneshot::Sender<()>,
    ack_before_finish: bool,
    finish: impl FnOnce() -> Result<Vec<LiveStreamEvent>>,
    close_residue: impl FnOnce(&mut AppleSealState),
) -> Result<()> {
    let mut acknowledgement = Some(last_window_closed);
    if ack_before_finish {
        let _ = acknowledgement
            .take()
            .expect("stop acknowledgement")
            .send(());
    }
    let trailing = match finish() {
        Ok(events) => events,
        Err(error) => {
            state.stop_trailing_finish = false;
            return Err(error);
        }
    };
    emit_stream_events(trailing, ev_tx, state, audio_secs);
    close_residue(state);
    state.publish_unadmitted_words(ev_tx);
    state.stop_trailing_finish = false;
    if let Some(acknowledgement) = acknowledgement {
        let _ = acknowledgement.send(());
    }
    Ok(())
}

/// A successful transport return alone does not prove a recognizer final:
/// `finish` can also return an error event or a disconnected partial stream.
fn apple_stop_final_received(events: &[LiveStreamEvent]) -> bool {
    let failed = events.iter().any(|event| {
        matches!(
            event,
            LiveStreamEvent::Error { .. } | LiveStreamEvent::Summary { ok: false, .. }
        )
    });
    !failed
        && events.iter().any(|event| {
            matches!(
                event,
                LiveStreamEvent::PhraseFinal { .. } | LiveStreamEvent::Summary { ok: true, .. }
            )
        })
}

/// Forward completion only after every preceding worker event reached the
/// reducer and its synchronous paint path. Channel loss is never success.
fn forward_live_finals_admitted(
    succeeded: bool,
    events: &mut mpsc::UnboundedReceiver<EngineEvent>,
    sink: &dyn EventSink,
    stream_log_path: Option<&std::path::Path>,
    completion: &mut Option<tokio::sync::oneshot::Sender<()>>,
) {
    while let Ok(event) = events.try_recv() {
        deliver_event(&event, sink, stream_log_path);
    }
    if let Some(sender) = completion.take()
        && succeeded
    {
        let _ = sender.send(());
    }
}

/// A clean transport close can precede a missing final. Only accepted typed
/// commit ranges prove that every offered capture sample reached a final;
/// this is a stop-completion predicate, never document or seal authority.
fn cloud_stop_finals_cover_capture(
    finals: &[crate::asr_session::events::TranscriptEvent],
    captured_samples: u64,
) -> bool {
    if captured_samples == 0 {
        return true;
    }
    let mut ranges = finals
        .iter()
        .filter_map(|event| event.commit.as_ref())
        .map(|commit| (commit.sample_start, commit.sample_end))
        .collect::<Vec<_>>();
    ranges.sort_unstable();
    let mut frontier = 0;
    for (start, end) in ranges {
        if start > frontier {
            return false;
        }
        frontier = frontier.max(end);
        if frontier >= captured_samples {
            return true;
        }
    }
    false
}

/// The provider's bounded `end` close supplies one terminal notice after its
/// finals. Drain only that live lane; pending refinement work cannot hold it.
fn drain_cloud_stop_finals(
    state: &mut AppleSealState,
    events: &mpsc::UnboundedSender<EngineEvent>,
    notices: &std_mpsc::Receiver<CloudWorkerNotice>,
) -> bool {
    loop {
        match notices.recv() {
            Ok(CloudWorkerNotice::EndSettled) => return true,
            Ok(CloudWorkerNotice::Final(event)) => state.admit_cloud_final(events, *event),
            Ok(CloudWorkerNotice::LaneLost) | Err(_) => {
                state.return_cloud_live_lost(events);
                return false;
            }
        }
    }
}

/// The direct owner closes delivery on success, error, panic and input loss.
/// It is deliberately independent of ledger seals and native decoder joining.
struct DirectTranscriptEndGuard {
    destination: mpsc::UnboundedSender<EngineEvent>,
    session_id: String,
    capture_epoch: u64,
}

impl Drop for DirectTranscriptEndGuard {
    fn drop(&mut self) {
        let _ = self.destination.send(EngineEvent::DirectTranscriptEnd {
            session_id: std::mem::take(&mut self.session_id),
            capture_epoch: self.capture_epoch,
        });
    }
}

/// Everything the blocking worker needs that is not a channel.
struct AppleWorkerConfig<'a> {
    input_enhanced: bool,
    input_archive: SpeechInputArchive,
    capture_turn: CaptureTurnIntent,
    consultation: Option<LiveConsultationCapture>,
    local_execution: Arc<LocalExecutionOwner>,
    sample_rate: u32,
    /// Device the recorder opened; selects the measured calibration profile.
    capture_device_name: Option<String>,
    language: Option<&'a str>,
    session_id: String,
    capture_epoch: u64,
    /// The capture arm's energy-ladder owner. The worker reads the same handle
    /// the async writer feeds; it never opens a ladder of its own.
    capture_energy: CaptureEnergyOwner,
    runtime_settings: Arc<RuntimeSettingsSnapshot>,
    acoustic_ledger: Arc<Mutex<AcousticLedger>>,
    settings_digest: String,
    /// Product "Hands-free silence". `Some` arms the engine lifecycle (speech
    /// epochs); `None` keeps one continuous SFSpeech stream for the whole take.
    utterance_silence_sec: Option<f32>,
    terminal_audio:
        Option<std_mpsc::Receiver<Result<super::live_audio_buffer::FinalizedPcmArchive, String>>>,
    last_window_closed: tokio::sync::oneshot::Sender<()>,
    apple_finished: tokio::sync::oneshot::Sender<()>,
    live_finals_admitted: tokio::sync::oneshot::Sender<()>,
    /// CLOUD mode only: silence commits to the live WS lane and its notices back.
    cloud: Option<CloudWorkerChannels>,
}

/// Blocking worker: owns the SFSpeech stream(s) for the session's full lifetime.
fn apple_stream_worker(
    pcm_rx: std_mpsc::Receiver<Result<Option<SpeechInputFrame>, String>>,
    ev_tx: mpsc::UnboundedSender<EngineEvent>,
    tail_patch: Option<mpsc::Sender<TailPatchRequest>>,
    tail_patch_done: std_mpsc::Receiver<TailPatchCompletion>,
    formatter: Option<mpsc::Sender<FormatterRequest>>,
    config: AppleWorkerConfig<'_>,
) -> anyhow::Result<AppleStreamOutcome> {
    let AppleWorkerConfig {
        input_enhanced,
        input_archive,
        capture_turn,
        mut consultation,
        local_execution,
        sample_rate,
        capture_device_name,
        language,
        session_id,
        capture_epoch,
        capture_energy,
        runtime_settings,
        acoustic_ledger,
        settings_digest,
        utterance_silence_sec,
        terminal_audio,
        last_window_closed,
        apple_finished,
        live_finals_admitted,
        cloud,
    } = config;
    let seal_ledger_enabled = runtime_settings.seal_ledger_enabled();
    let apple_live_enabled = runtime_settings.apple_live_enabled();
    let direct_transcript_end = (!seal_ledger_enabled).then(|| DirectTranscriptEndGuard {
        destination: ev_tx.clone(),
        session_id: session_id.clone(),
        capture_epoch,
    });
    if !apple_live_enabled && tail_patch.is_none() && cloud.is_none() {
        let reason = "No enabled recognizer: Apple live is disabled and the selected Layer 1 source is not armed.";
        let _ = ev_tx.send(EngineEvent::NoSpeech {
            reason: reason.into(),
        });
        let _ = last_window_closed.send(());
        let _ = apple_finished.send(());
        drop(live_finals_admitted);
        anyhow::bail!(reason);
    }
    debug_assert_eq!(settings_digest, runtime_settings.digest().as_str());
    let _slot_receipts = SlotReceiptSink::for_session(&session_id, capture_epoch, sample_rate);
    // Retain the real device profile even when word admission is disabled.
    // Missing calibration stays absent; only the ledger path requires it.
    // Direct publication never invents a floor or treats this lookup as a gate.
    let energy_calibration = match capture_device_name.as_deref() {
        Some(device) => {
            match runtime_settings.energy_calibration_for_capture(device, sample_rate) {
                Ok(calibration) => {
                    info!(
                        session = %session_id,
                        device,
                        sample_rate,
                        calibration_version = %calibration.version,
                        min_energy_integral = calibration.min_energy_integral,
                        min_valley_samples = calibration.min_valley_samples,
                        "acoustic admission calibration sealed for session"
                    );
                    Some(calibration)
                }
                Err(refusal) => {
                    warn!(
                        session = %session_id,
                        device,
                        sample_rate,
                        %refusal,
                        "acoustic admission calibration refused; session cannot qualify occurrences"
                    );
                    None
                }
            }
        }
        None => {
            warn!(
                session = %session_id,
                "no capture device bound to session; acoustic admission stays fail-closed"
            );
            None
        }
    };
    let mut state = match tail_patch {
        Some(tx) => AppleSealState::new_with_tail_patch_for_session(
            sample_rate,
            session_id,
            capture_epoch,
            tx,
            acoustic_ledger,
            energy_calibration,
        ),
        None => AppleSealState::new_for_session_with_ledger(
            sample_rate,
            session_id,
            capture_epoch,
            acoustic_ledger,
            energy_calibration,
        ),
    };
    state.seal_ledger_enabled = seal_ledger_enabled;
    state.apple_live_enabled = apple_live_enabled;
    state.direct_end = direct_transcript_end;
    if !seal_ledger_enabled {
        state.window_plan.set_non_overlapping_publications();
    }
    state.bind_capture_energy(capture_energy);
    if let Some(device) = capture_device_name.as_deref()
        && let Ok(floor) = runtime_settings
            .energy_calibration()
            .speech_anchor_floor_for_capture(device, sample_rate)
    {
        state
            .acoustic_ledger
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .bind_speech_anchor_floor(floor);
    }
    if let Some(device) = capture_device_name.as_deref()
        && let Ok(floor) = runtime_settings
            .energy_calibration()
            .existence_floor_rms_for_capture(device, sample_rate)
    {
        state.existence_floor_rms = Some(floor);
    }
    if input_enhanced {
        state.recognizer_audio = Some(LiveAudioBuffer::new(sample_rate, DEFAULT_RETENTION_SECS));
        state.recognizer_archive = input_archive;
    }
    state.capture_turn = capture_turn;
    state.terminal_archive_pending = terminal_audio.is_some();
    state.formatter = formatter;
    let (cloud_commit, cloud_notice) = cloud.map(|lane| (lane.commit, lane.notice)).unzip();
    state.cloud_commit_tx = cloud_commit;
    if !seal_ledger_enabled {
        state.direct_producer = if state.tail_patch.is_some() {
            LedgerObservationProducer::Whisper
        } else if state.cloud_commit_tx.is_some() {
            LedgerObservationProducer::CloudLive
        } else {
            LedgerObservationProducer::Apple
        };
    }
    // The session's ONE Silero. Both consumers of speech edges read it: the
    // utterance ledger (identity, ranges) and the engine lifecycle (wake/sleep).
    // It is built whenever either consumer wants it — the fusion flag decides
    // whether identity reaches the seal, not whether the VAD exists. The
    // ledger-free direct lane is a third consumer: opting out of admission
    // must not switch off the take's only audio observer.
    state.fusion_seal_armed = runtime_settings.seal_lane_armed();
    state.engine_tuning =
        EngineTuning::with_ledger_sensitivity(runtime_settings.values().ledger_sensitivity);
    if state.engine_tuning != EngineTuning::DEFAULT {
        info!(
            session = %state.session_id,
            ledger_sensitivity = runtime_settings.values().ledger_sensitivity,
            whisper_word_speech_near_ms = state.engine_tuning.whisper_word_speech_near_ms,
            whisper_repeat_min_ms_per_syllable =
                state.engine_tuning.whisper_repeat_min_ms_per_syllable,
            "ledger slider moved word admission off the default for this take"
        );
    }
    // Voice Lab witness: the immutable snapshot verdict replaces the
    // construction-time env read; the return lane belongs to this worker.
    state.dictation_witness_armed =
        seal_ledger_enabled && apple_live_enabled && runtime_settings.dictation_witness_armed();
    state.dictation_witness_language = language.map(str::to_owned);
    let (witness_done_tx, witness_done_rx) = std_mpsc::channel::<DictationWitnessReturn>();
    state.dictation_witness_return = Some(witness_done_tx);
    if state.fusion_seal_armed || utterance_silence_sec.is_some() || !seal_ledger_enabled {
        let ingress =
            SileroIngress::new(sample_rate, state.session_id.clone(), state.capture_epoch);
        if ingress.vad_available() {
            info!(
                seal_armed = state.fusion_seal_armed,
                lifecycle_armed = utterance_silence_sec.is_some(),
                "Silero ingress armed — single VAD feeding utterance identity and engine lifecycle"
            );
            state.fusion = Some(ingress);
        } else {
            warn!(
                "Silero model unavailable — no utterance identity and no engine lifecycle \
                 this session; Apple segment boundaries stay the seal authority"
            );
        }
    }
    // Engine lifecycle. Disarmed → one stream opened here for the whole take
    // (legacy). Armed → the bridge stays unspawned until the first speech edge,
    // and every epoch closes on the product silence threshold.
    let mut epoch =
        EpochGate::for_session(sample_rate, utterance_silence_sec, state.fusion.is_some());
    let mut stream = if !apple_live_enabled {
        None
    } else if epoch.is_armed() {
        info!(
            utterance_silence_sec = utterance_silence_sec.unwrap_or_default(),
            preroll_secs = EPOCH_PREROLL_SECS,
            "Apple progressive engine lifecycle armed — SFSpeech rests between utterances"
        );
        None
    } else {
        Some(LiveStreamSession::open(language, sample_rate)?)
    };
    // Session-time base of the open epoch. Zero for the legacy single stream,
    // which is what makes `shift_events` the identity on that path.
    let mut epoch_base_samples: u64 = 0;
    let mut samples_seen: u64 = 0;

    loop {
        state.tick_refinements(&ev_tx, Instant::now());
        state.flush_cloud_commits(&ev_tx);
        state.collect_ready_refinement_returns(
            &ev_tx,
            &tail_patch_done,
            cloud_notice.as_ref(),
            samples_seen as f32 / sample_rate.max(1) as f32,
        );
        drain_dictation_witness_returns(&mut state, &ev_tx, &witness_done_rx);
        // Interleave PCM wait with event polling so partials land
        // mid-utterance without waiting for the next audio chunk.
        match pcm_rx.recv_timeout(LIVE_WORKER_QUANTUM) {
            Ok(Ok(Some(frame))) => {
                let samples = &frame.samples;
                let raw_samples = frame.raw_samples();
                samples_seen += raw_samples.len() as u64;
                // Retain before forwarding, on the same counter `audio_secs` is
                // derived from, so the buffer and the seal clock cannot drift.
                // This is worker-side on purpose: the async select loop stays
                // lock-free (2026-07-27 interleave contract) because the buffer
                // is never shared across the thread boundary.
                //
                // Retention runs in every lifecycle state, including while the
                // engine rests: it is what the pre-roll of the next epoch is cut
                // from, and what Layer 1 windows still resolve against.
                state.audio.push(raw_samples);
                if let Some(audio) = state.recognizer_audio.as_mut() {
                    audio.push(samples);
                }
                state.pump_capture_windows(&ev_tx);
                // A final may have queued while recv_timeout waited for PCM.
                // Admit it before advancing the capture-clock expiry boundary.
                if let Some(notices) = cloud_notice.as_ref() {
                    while let Ok(notice) = notices.try_recv() {
                        state.handle_cloud_notice(&ev_tx, notice);
                    }
                }
                state.observe_cloud_capture_head(samples_seen);
                state.release_cloud_live_behind(&ev_tx, samples_seen);
                // One observation of the spectrum, two consumers: the ledger
                // mints identity from it and the lifecycle wakes/sleeps on it.
                let silero_ingest = state
                    .fusion
                    .as_mut()
                    .map(|fusion| fusion.ingest(raw_samples, samples, samples_seen));
                if let Some(ingest) = silero_ingest.as_ref() {
                    if let Some(owner) = consultation.as_mut()
                        && owner.observe(ingest, samples_seen).is_err()
                    {
                        owner.report_refusal(&ev_tx);
                    }
                    for evidence in &ingest.sideband {
                        let _ = ev_tx.send(EngineEvent::SidebandEvidence {
                            evidence: evidence.clone(),
                        });
                    }
                }
                publish_apple_phrase_at_valley(
                    &mut state,
                    &ev_tx,
                    samples_seen as f32 / sample_rate.max(1) as f32,
                    consultation.as_mut(),
                );
                if state.fusion_seal_armed {
                    if let Some(fusion) = state.fusion.as_ref() {
                        state.speech_progress.observe_speech(
                            &fusion.acoustic_speech_evidence(),
                            silero_ingest
                                .as_ref()
                                .is_some_and(|ingest| ingest.speech_live),
                            sample_rate,
                        );
                    }
                    seal_sliced_by_silero(&mut state, &ev_tx, &[]);
                }
                let speech_live = silero_ingest.is_some_and(|ingest| ingest.speech_live);
                let audio_secs = samples_seen as f32 / sample_rate.max(1) as f32;
                match epoch.feed_pcm(raw_samples, samples_seen, speech_live) {
                    EpochDecision::Forward => {
                        if let Some(session) = stream.as_mut() {
                            session.write_pcm(samples).inspect_err(|_| {
                                state.return_outstanding_cloud(&ev_tx);
                            })?;
                            let events = shift_events(
                                session.poll_events(),
                                epoch_base_secs(epoch_base_samples, sample_rate),
                            );
                            emit_stream_events(events, &ev_tx, &mut state, audio_secs);
                        }
                    }
                    EpochDecision::Wake { .. } if !state.apple_live_enabled => {}
                    EpochDecision::Wake { preroll_from } => {
                        let mut session = LiveStreamSession::open(language, sample_rate)
                            .inspect_err(|_| state.return_outstanding_cloud(&ev_tx))?;
                        let chunk_start = samples_seen.saturating_sub(samples.len() as u64);
                        // The base is whatever audio this epoch ACTUALLY starts
                        // with, never what was asked for: a pre-roll that fell
                        // off retention resolves to nothing, and basing the
                        // epoch on it would shift every timestamp in it earlier
                        // by the missing audio.
                        let preroll = state.recognizer_window_by_samples(preroll_from, chunk_start);
                        epoch_base_samples =
                            preroll.as_ref().map_or(chunk_start, |w| w.sample_start);
                        let preroll_samples =
                            preroll.as_ref().map_or(0, |window| window.samples.len());
                        if let Some(window) = preroll.filter(|w| !w.samples.is_empty()) {
                            session
                                .write_pcm(&window.samples)
                                .inspect_err(|_| state.return_outstanding_cloud(&ev_tx))?;
                        }
                        session.write_pcm(samples).inspect_err(|_| {
                            state.return_outstanding_cloud(&ev_tx);
                        })?;
                        info!(
                            audio_secs,
                            epoch_base_secs = epoch_base_secs(epoch_base_samples, sample_rate),
                            preroll_samples,
                            "apple_lifecycle: epoch open (speech edge)"
                        );
                        let events = shift_events(
                            session.poll_events(),
                            epoch_base_secs(epoch_base_samples, sample_rate),
                        );
                        emit_stream_events(events, &ev_tx, &mut state, audio_secs);
                        stream = Some(session);
                    }
                    EpochDecision::Sleep { silence_secs } => {
                        if let Some(session) = stream.take() {
                            let base_secs = epoch_base_secs(epoch_base_samples, sample_rate);
                            let trailing = shift_events(
                                session
                                    .finish()
                                    .inspect_err(|_| state.return_outstanding_cloud(&ev_tx))?,
                                base_secs,
                            );
                            emit_stream_events(trailing, &ev_tx, &mut state, audio_secs);
                            // Same close as capture EOF: whatever the engine
                            // left open is sealed here, because no later
                            // callback from this epoch can arrive.
                            seal_open_partial(&mut state, &ev_tx, audio_secs);
                            state.pump_capture_windows(&ev_tx);
                            info!(
                                audio_secs,
                                silence_secs,
                                epoch_base_secs = base_secs,
                                "apple_lifecycle: epoch close (hands-free silence)"
                            );
                        }
                    }
                    // Resting: audio is retained, the engine is not running.
                    EpochDecision::Idle => {}
                }
            }
            Ok(Ok(None)) => break, // EOF after the projection flushed its real tail
            Ok(Err(error)) => {
                state.return_outstanding_cloud(&ev_tx);
                return Err(anyhow::anyhow!(error));
            }
            Err(std_mpsc::RecvTimeoutError::Timeout) => {
                let audio_secs = samples_seen as f32 / sample_rate.max(1) as f32;
                if let Some(session) = stream.as_mut() {
                    let events = shift_events(
                        session.poll_events(),
                        epoch_base_secs(epoch_base_samples, sample_rate),
                    );
                    emit_stream_events(events, &ev_tx, &mut state, audio_secs);
                }
            }
            Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if let Some(owner) = consultation.as_mut() {
            owner.tick(&state, &ev_tx);
        }
        state.emit_speech_integrity(&ev_tx);
    }

    state.capture_stopping = true;
    state.pump_capture_windows(&ev_tx);
    let audio_secs = samples_seen as f32 / sample_rate.max(1) as f32;
    if let Some(fusion) = state.fusion.as_mut() {
        fusion.flush(samples_seen);
    }
    // The armed lane alone seals before finish. The unarmed lane must retain
    // finish-first ordering: it synthesizes ranges after the consumed cursor
    // and admits Summary only at utterance_id == 0. Amendment 1: a take with
    // no Apple text waits for finish and residue admission before its ack.
    // Later refusals and K5 visibility receipts are NOT document revisions;
    // only an accepted mutation can revise the stop document.
    let ack_before_finish = if state.seal_ledger_enabled && state.fusion_seal_armed {
        let has_text = state.has_stop_canvas_text();
        seal_sliced_by_silero(&mut state, &ev_tx, &[]);
        seal_open_partial(&mut state, &ev_tx, audio_secs);
        state.stop_trailing_finish = true;
        has_text
    } else {
        false
    };
    let mut apple_final_received = stream.is_none();
    finish_capture_after_seal(
        &mut state,
        &ev_tx,
        audio_secs,
        last_window_closed,
        ack_before_finish,
        || {
            stream.take().map_or_else(
                || Ok(Vec::new()),
                |session| {
                    session.finish().map(|events| {
                        apple_final_received = apple_stop_final_received(&events);
                        shift_events(events, epoch_base_secs(epoch_base_samples, sample_rate))
                    })
                },
            )
        },
        |state| {
            seal_open_partial(state, &ev_tx, audio_secs);
            if state.fusion_seal_armed {
                seal_sliced_by_silero(state, &ev_tx, &[]);
            }
        },
    )
    .inspect_err(|_| state.return_outstanding_cloud(&ev_tx))?;

    // `finish` has delivered the post-endAudio Apple events. CLOUD now ends
    // its live transport while this worker still owns admission. Neither the
    // archive handoff nor Whisper/formatter/recovery work owns this receipt.
    let _ = apple_finished.send(());
    let live_finals_complete = match cloud_notice.as_ref() {
        Some(notices) if state.cloud_commit_tx.is_some() => {
            drain_cloud_stop_finals(&mut state, &ev_tx, notices)
        }
        Some(_) => false,
        None => true,
    };
    // Poll the same capture plan before evaluating readiness. A final Apple
    // callback says nothing about outstanding Whisper or recovery work.
    state.capture_stopping = true;
    state.pump_capture_windows(&ev_tx);
    state.close_admission_horizon(&ev_tx, u64::MAX);
    state.publish_unadmitted_words(&ev_tx);
    let direct_live_finals_admitted = if state.seal_ledger_enabled {
        if live_finals_complete && apple_final_received && state.stop_document_settled() {
            let _ = live_finals_admitted.send(());
        } else {
            // Ledger-mode terminal refinement retains the existing early
            // failure receipt so its caller can hand off the archive.
            drop(live_finals_admitted);
        }
        None
    } else {
        // Direct-mode callers publish their real PCM before waiting. Keep this
        // native receipt until its selected decoder has actually drained.
        Some(live_finals_admitted)
    };

    if let Some(receiver) = terminal_audio {
        let archive = receiver
            .recv_timeout(Duration::from_secs(30))
            .map_err(|error| anyhow::anyhow!("terminal archive handoff failed: {error}"))
            .and_then(|receipt| receipt.map_err(anyhow::Error::msg))
            .and_then(|receipt| {
                receipt.load(
                    &state.session_id,
                    state.capture_epoch,
                    sample_rate,
                    samples_seen,
                )
            });
        match archive {
            Ok(pcm) => {
                state.terminal_pcm = Some(pcm);
                state.terminal_archive_pending = false;
            }
            Err(error) => {
                let _ = ev_tx.send(EngineEvent::Warning {
                    code: "terminal_owned_pcm_unavailable".into(),
                    message: error.to_string(),
                });
                state.return_outstanding_cloud(&ev_tx);
                return Err(error);
            }
        }
    }
    state.pump_capture_windows(&ev_tx);

    // Every accepted Layer 1 request must close (success, no-change, or)
    // explicit skip) before the session task returns. This is bounded by the
    // queue cap and happens while the async side is still draining jobs.
    //
    // Wait on the *jobs*, not on the pending-seal queue. Those are different
    // conditions: a span still pending can be blocked by the Apple volatile
    // window rather than by a missing Whisper window, and no completion will
    // ever clear that gate. Waiting on the seal queue therefore parked the stop
    // path on the full timeout whenever the last span was volatile-blocked —
    // measured 2026-08-12, `rec_stop=36.701s` of which 30.005s was this loop
    // waiting for a completion that had already arrived for every job it sent.
    let mut tail_patch_timeout_residue = 0;
    // The warm budget is honest only after the engine proved it executes:
    // a session that still owes its first window may be paying the model
    // cold-load with its only accepted text, so it gets the cold budget.
    // When nothing is outstanding the loops below never wait, so the wider
    // budget cannot slow an owing-nothing stop.
    let owes_first_window = !state.first_whisper_window_completed
        && (state.planned_work_pending()
            || !state.refinement_submitted.is_empty()
            || !state.refinement_pending.is_empty());
    let closure_budget = if owes_first_window {
        TAIL_PATCH_COLD_START_CLOSURE_TIMEOUT
    } else {
        TAIL_PATCH_CLOSURE_TIMEOUT
    };
    let stop_deadline = local_execution.begin_drain(closure_budget);
    if state.cloud_commit_tx.is_none()
        && state.cloud_uncommitted.is_empty()
        && state.cloud_commit_retry.is_empty()
        && state.cloud_inflight.is_empty()
    {
        while state.planned_work_pending()
            || !state.refinement_submitted.is_empty()
            || !state.refinement_pending.is_empty()
        {
            // Account ready returns before the deadline can retire their
            // registered requests. Count only the work still outstanding.
            state.collect_ready_refinement_returns(
                &ev_tx,
                &tail_patch_done,
                cloud_notice.as_ref(),
                audio_secs,
            );
            let outstanding = state.tail_patch_awaiting_completion();
            if !state.stop_refinements_tick(&ev_tx, Instant::now(), stop_deadline) {
                tail_patch_timeout_residue = outstanding;
                break;
            }
            match tail_patch_done.recv_timeout(LIVE_WORKER_QUANTUM) {
                Ok(completion) => state.complete_whisper_window(&ev_tx, completion, audio_secs),
                Err(std_mpsc::RecvTimeoutError::Timeout) => continue,
                Err(error) => {
                    warn!(
                        "tail-patch closure wait ended before all observations returned: {error}"
                    );
                    tail_patch_timeout_residue = state.tail_patch_awaiting_completion();
                    state.return_outstanding_whisper_without_label(&ev_tx);
                    break;
                }
            }
        }
    } else {
        // Live-final admission has already settled before the archive handoff.
        // Remaining observer/refinement work stays a post-delivery revision.
        while state.planned_work_pending()
            || !state.refinement_submitted.is_empty()
            || !state.refinement_pending.is_empty()
            || !state.cloud_inflight.is_empty()
        {
            // Cloud finals have the same race as local completions: returning
            // their observers first can seal Apple before a queued final lands.
            state.collect_ready_refinement_returns(
                &ev_tx,
                &tail_patch_done,
                cloud_notice.as_ref(),
                audio_secs,
            );
            let outstanding = state.tail_patch_awaiting_completion();
            let now = Instant::now();
            if (state.planned_work_pending()
                || !state.refinement_submitted.is_empty()
                || !state.refinement_pending.is_empty())
                && !state.stop_refinements_tick(&ev_tx, now, stop_deadline)
            {
                tail_patch_timeout_residue = outstanding;
            }
            if now >= stop_deadline {
                state.return_outstanding_cloud(&ev_tx);
                break;
            }
            if state.refinement_submitted.is_empty() && state.refinement_pending.is_empty() {
                if state.cloud_inflight.is_empty() {
                    break;
                }
                match cloud_notice
                    .as_ref()
                    .map(|notices| notices.recv_timeout(LIVE_WORKER_QUANTUM))
                {
                    Some(Ok(notice)) => state.handle_cloud_notice(&ev_tx, notice),
                    Some(Err(std_mpsc::RecvTimeoutError::Timeout)) => continue,
                    Some(Err(std_mpsc::RecvTimeoutError::Disconnected)) | None => {
                        state.return_outstanding_cloud(&ev_tx);
                        break;
                    }
                }
                continue;
            }
            match tail_patch_done.recv_timeout(LIVE_WORKER_QUANTUM) {
                Ok(completion) => state.complete_whisper_window(&ev_tx, completion, audio_secs),
                Err(std_mpsc::RecvTimeoutError::Timeout) => continue,
                Err(error) => {
                    warn!(
                        "tail-patch closure wait ended before all observations returned: {error}"
                    );
                    tail_patch_timeout_residue = state.tail_patch_awaiting_completion();
                    state.return_outstanding_whisper_without_label(&ev_tx);
                    if !state.cloud_inflight.is_empty() {
                        state.return_outstanding_cloud(&ev_tx);
                    }
                    break;
                }
            }
        }
    }

    state.return_outstanding_cloud(&ev_tx);
    state.close_admission_horizon(&ev_tx, u64::MAX);

    // Formatting is not awaited here: it consumes sealed text on the async
    // side and cannot extend Stop, hold a seal or keep the microphone open.

    if !state.seal_ledger_enabled {
        let complete = live_finals_complete
            && (state.direct_producer != LedgerObservationProducer::Apple || apple_final_received)
            && state.direct_end.is_some()
            && tail_patch_timeout_residue == 0
            && state.tail_patch_jobs_skipped == 0
            && state.unresolved_windows == 0
            && !state.refinement_lane_lost;
        // Queue the typed end before the native acknowledgement. Its forwarding
        // arm drains these events through the reducer before releasing Stop.
        drop(state.direct_end.take());
        if let Some(sender) = direct_live_finals_admitted
            && complete
        {
            let _ = sender.send(());
        }
        return Ok(AppleStreamOutcome {
            sealed: 0,
            filtered_empty_drops: state.filtered_empty_drops,
            lexicon_rewrites: 0,
            lexicon_entries_custom: 0,
            unresolved_windows: state.unresolved_windows,
            under_commit_escalations: 0,
            tail_patch_replacements: 0,
            tail_patch_refusals: state.tail_patch_refusals,
            tail_patch_jobs_applied: state.tail_patch_jobs_applied,
            tail_patch_jobs_skipped: state.tail_patch_jobs_skipped,
            tail_patch_timeout_residue,
            conservation: state.session_conservation(),
        });
    }

    // Capture is over: no later Apple callback can revise a span and no further
    // Whisper window can arrive, so both double-close gates are satisfied by
    // definition. Seal the remainder here instead of leaving it to the residual
    // path — the machine's own span timestamps are the clock, because the audio
    // clock is frozen at EOF and can sit milliseconds behind them.
    state.seal_remaining_at_session_end(&ev_tx);
    reconcile_terminal_coverage(&mut state, &ev_tx);
    // Bounded terminal DictationTranscriber witness: one last evidence source
    // for debt the retained replays left unresolved (lost take beginnings,
    // negations, uncovered speech gaps). It reads only the recorder-owned
    // terminal PCM and every returned segment enters through ordinary ledger
    // admission; it never rewrites the document wholesale.
    run_terminal_dictation_witness(&mut state, &ev_tx, language, &witness_done_rx);
    state.close_admission_horizon(&ev_tx, u64::MAX);
    state.publish_resolved_word_seals(&ev_tx);
    state.seal_remaining_at_session_end(&ev_tx);
    if let Some(owner) = consultation.as_mut() {
        owner.settle(&state, &ev_tx, samples_seen);
    }
    state.publish_unadmitted_words(&ev_tx);
    let seal_coverage = publish_terminal_coverage(&state, &ev_tx);
    state.emit_speech_integrity(&ev_tx);
    info!(
        session_id = %state.session_id,
        capture_epoch = state.capture_epoch,
        no_time_overlap_warnings = state.no_time_overlap_warnings,
        unmatched_word_identities = state.warned_unmatched_words.len(),
        retained_unmatched_words = state.unmatched_silero_words.len(),
        "apple_fusion_session_receipt"
    );
    if state.cloud_commit_tx.is_some()
        || state.cloud_live_admitted > 0
        || state.cloud_live_lane_lost > 0
        || state.cloud_live_timed_out > 0
        || state.cloud_live_unmatched_final > 0
        || state.cloud_uncommitted_span_ms > 0
    {
        info!(
            session_id = %state.session_id,
            cloud_live_admitted = state.cloud_live_admitted,
            cloud_live_replaced_apple = state.cloud_live_replaced_apple,
            cloud_live_refused_sealed = state.cloud_live_refused_sealed,
            cloud_live_unowned_routed = state.cloud_live_unowned_routed,
            cloud_live_lane_lost = state.cloud_live_lane_lost,
            cloud_live_timed_out = state.cloud_live_timed_out,
            cloud_live_unmatched_final = state.cloud_live_unmatched_final,
            cloud_uncommitted_span_ms = state.cloud_uncommitted_span_ms,
            "cloud_live_take"
        );
    }
    // Any non-complete verdict takes the same non-success path. The warning
    // names which one it was; neither may reach the terminal seal below.
    if !seal_coverage.status.is_complete() {
        let (code, message) = match seal_coverage.status.unavailable_reason() {
            Some(gap) => (
                "terminal_seal_coverage_unavailable".to_string(),
                format!(
                    "reason={} producer={} availability={}",
                    gap.as_str(),
                    seal_coverage.speech_producer,
                    seal_coverage.availability,
                ),
            ),
            None => (
                "terminal_seal_coverage_incomplete".to_string(),
                format!(
                    "covered={}/{} max_uncovered={} threshold={}",
                    seal_coverage.covered_samples,
                    seal_coverage.speech_samples,
                    seal_coverage.max_uncovered_samples,
                    seal_coverage.incomplete_threshold_samples,
                ),
            ),
        };
        let _ = ev_tx.send(EngineEvent::Warning { code, message });
        return Ok(AppleStreamOutcome {
            sealed: state.sealed_count,
            filtered_empty_drops: state.filtered_empty_drops,
            lexicon_rewrites: state.lexicon_rewrites,
            lexicon_entries_custom: state.lexicon_entries_custom,
            unresolved_windows: state.unresolved_windows,
            under_commit_escalations: state.under_commit_escalations,
            tail_patch_replacements: state.tail_patch_replacements,
            tail_patch_refusals: state.tail_patch_refusals,
            tail_patch_jobs_applied: state.tail_patch_jobs_applied,
            tail_patch_jobs_skipped: state.tail_patch_jobs_skipped,
            tail_patch_timeout_residue,
            conservation: state.session_conservation(),
        });
    }
    let terminal = state
        .acoustic_ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .seal_terminal(&state.session_id, state.capture_epoch);
    match terminal {
        Ok(receipt) => {
            let _ = ev_tx.send(EngineEvent::LedgerSeal { receipt });
        }
        Err(refusal) => report_terminal_seal_refusal(&ev_tx, refusal),
    }

    Ok(AppleStreamOutcome {
        sealed: state.sealed_count,
        filtered_empty_drops: state.filtered_empty_drops,
        lexicon_rewrites: state.lexicon_rewrites,
        lexicon_entries_custom: state.lexicon_entries_custom,
        unresolved_windows: state.unresolved_windows,
        under_commit_escalations: state.under_commit_escalations,
        tail_patch_replacements: state.tail_patch_replacements,
        tail_patch_refusals: state.tail_patch_refusals,
        tail_patch_jobs_applied: state.tail_patch_jobs_applied,
        tail_patch_jobs_skipped: state.tail_patch_jobs_skipped,
        tail_patch_timeout_residue,
        conservation: state.session_conservation(),
    })
}

/// A valid later onset follows the complete timed extent of the prior phrase.
/// This retains physical repetitions even when their labels are identical.
/// No earlier segment may lack timing or extend beyond the claimed boundary.
pub(crate) fn apple_partials_are_disjoint(
    previous: &[TranscriptSegment],
    next: &[TranscriptSegment],
) -> bool {
    let Some(next) = next.first() else {
        return false;
    };
    if next.text.trim().is_empty()
        || !next.start_ts.is_finite()
        || !next.end_ts.is_finite()
        || next.start_ts < 0.0
        || next.end_ts <= next.start_ts
        || previous.is_empty()
    {
        return false;
    }
    previous.iter().all(|segment| {
        !segment.text.trim().is_empty()
            && segment.start_ts.is_finite()
            && segment.end_ts.is_finite()
            && segment.start_ts >= 0.0
            && segment.end_ts > segment.start_ts
            && segment.end_ts <= next.start_ts
    })
}

fn apple_partial_restart_has_valley(state: &AppleSealState, next: &[TranscriptSegment]) -> bool {
    if !apple_partials_are_disjoint(&state.open_partial_segments, next) {
        return false;
    }
    let (Some(fusion), Some(calibration)) = (&state.fusion, state.energy_calibration.as_ref())
    else {
        return false;
    };
    let previous = apple_segments_on_pcm_clock(state, &state.open_partial_segments);
    let next = apple_segments_on_pcm_clock(state, next);
    match (
        previous.iter().map(|word| word.range.sample_end).max(),
        next.first(),
    ) {
        (Some(end), Some(start)) => fusion.has_phrase_valley(
            end,
            start.range.sample_start,
            calibration.min_valley_samples,
        ),
        _ => false,
    }
}

fn retain_open_apple_phrase(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    audio_secs: f32,
) {
    if state.open_partial.is_empty() {
        return;
    }
    let text = std::mem::take(&mut state.open_partial);
    let segments = std::mem::take(&mut state.open_partial_segments);
    let rev = state.preview_rev;
    let (_, events) = close_apple_phrase(
        state,
        ev_tx,
        state.phrase_id,
        &text,
        segments,
        audio_secs,
        true,
    );
    let (final_disposition, refused_evidence) = phrase_final_disposition(&events, &text);
    let _ = ev_tx.send(EngineEvent::PreviewDisposition {
        superseded_through_rev: rev,
        final_disposition,
        refused_evidence,
    });
}

/// Qualify the existing physical owner on a short measured valley aligned
/// with the current provider's timed phrase end. Reuses ordinary seal/admission.
fn publish_apple_phrase_at_valley(
    state: &mut AppleSealState,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    audio_secs: f32,
    consultation: Option<&mut LiveConsultationCapture>,
) {
    if !state.seal_ledger_enabled
        || !state.apple_live_enabled
        || !state.fusion_seal_armed
        || state.open_partial.is_empty()
        || state.open_partial_segments.is_empty()
        || state.open_partial_segments.iter().any(|word| {
            !word.start_ts.is_finite()
                || !word.end_ts.is_finite()
                || word.start_ts < 0.0
                || word.end_ts <= word.start_ts
        })
    {
        return;
    }
    let Some(calibration) = state.energy_calibration.as_ref() else {
        return;
    };
    let Some(end) = apple_segments_on_pcm_clock(state, &state.open_partial_segments)
        .iter()
        .map(|word| word.range.sample_end)
        .max()
    else {
        return;
    };
    let Some(fusion) = state.fusion.as_mut() else {
        return;
    };
    let ingest = fusion.confirm_phrase_valley(end, calibration.min_valley_samples);
    if ingest.closed.is_empty() {
        return;
    }
    if let Some(owner) = consultation
        && owner.observe(&ingest, fusion.observed_samples()).is_err()
    {
        owner.report_refusal(ev_tx);
    }
    for evidence in ingest.sideband {
        let _ = ev_tx.send(EngineEvent::SidebandEvidence { evidence });
    }
    info!(
        phrase_end_sample = end,
        decision_sample = fusion.observed_samples(),
        "apple_lifecycle: progressive phrase closed on measured Silero valley"
    );
    retain_open_apple_phrase(state, ev_tx, audio_secs);
}

/// Read the phrase's actual Apple receipts; a successful seal function call
/// alone is not evidence that its words were admitted or kept visible.
fn phrase_final_disposition(
    events: &[EngineEvent],
    final_text: &str,
) -> (
    crate::pipeline::contracts::PreviewFinalDisposition,
    Vec<crate::pipeline::contracts::RefusedPreviewEvidence>,
) {
    use crate::pipeline::contracts::{PreviewFinalDisposition, RefusedPreviewEvidence};
    let mut kept_unanchored = false;
    let mut refusal = None;
    let mut refused_evidence = Vec::new();
    for event in events {
        match event {
            EngineEvent::LedgerMutation {
                observation,
                label,
                receipt,
            } if observation.producer == LedgerObservationProducer::Apple => match receipt {
                MutationReceipt::Insert { .. }
                | MutationReceipt::Correct { .. }
                | MutationReceipt::Preserve { .. } => {}
                MutationReceipt::KeepVisibleUnanchored { .. } => kept_unanchored = true,
                MutationReceipt::Refuse { occurrence, reason } => {
                    let reason = reason.as_str().to_string();
                    refusal = Some(reason.clone());
                    refused_evidence.push(RefusedPreviewEvidence {
                        range: Some(occurrence.clone()),
                        text: label.clone(),
                        reason,
                    });
                }
            },
            EngineEvent::Warning { code, .. } if code == "apple_final_without_pcm_timing" => {
                refusal = Some(code.clone());
            }
            _ => {}
        }
    }
    // Refused slices carry only their own label. Replaying the whole final
    // here would duplicate any sibling slice already visible in the canvas.
    if let Some(reason) = refusal.as_ref().filter(|_| refused_evidence.is_empty()) {
        let reason = reason.clone();
        refused_evidence.push(RefusedPreviewEvidence {
            range: None,
            text: final_text.to_string(),
            reason,
        });
    }
    let disposition = if let Some(reason) = refusal {
        PreviewFinalDisposition::Refused { reason }
    } else if kept_unanchored {
        PreviewFinalDisposition::KeptUnanchored
    } else {
        PreviewFinalDisposition::Admitted
    };
    (disposition, refused_evidence)
}

/// Map one poll's worth of bridge events onto `EngineEvent`s, sealing where the
/// stream says a phrase closed.
///
/// The mapping is where the RAW-preview / corrected-seal split is enforced:
/// `Partial` forwards verbatim, `PhraseFinal` goes through
/// [`seal_utterance_final`] (lexicon + cleanup). `audio_secs` is the session
/// clock and only acts as a fallback `end_ts` when the engine hands over no
/// segments. `Summary` reaches [`seal_utterance_final`] only at `utterance_id == 0`
/// or during the stop's trailing finish after the armed early seal. Hands-free
/// epoch close does not open that window; arming alone never admits a summary.
/// Admitted summaries use the existing physical admission and replay receipts.
///
/// Partial revisions replace the open observation. Only a measured Silero
/// valley permits retention as a closed phrase, independently of its wording.
fn emit_stream_events(
    events: Vec<LiveStreamEvent>,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    state: &mut AppleSealState,
    audio_secs: f32,
) {
    if !state.apple_live_enabled {
        return;
    }
    if !state.seal_ledger_enabled {
        emit_direct_apple_events(events, ev_tx, state);
        return;
    }
    for event in events {
        match event {
            LiveStreamEvent::Ready => {
                info!(
                    audio_secs,
                    sealed = state.sealed_count,
                    "apple_lifecycle: recognizer ready / stream start"
                );
            }
            LiveStreamEvent::End => {
                info!(
                    audio_secs,
                    sealed = state.sealed_count,
                    open_partial_chars = state.open_partial.len(),
                    filtered_empty_drops = state.filtered_empty_drops,
                    "apple_lifecycle: recognizer end"
                );
            }
            LiveStreamEvent::Partial { text, segments } => {
                state.finishing_phrase_id = None;
                state.observe_apple_progress(&text, &segments);
                // Disjoint recognizer timing nominates a restart; measured
                // Silero silence on this capture epoch is the only authority.
                if apple_partial_restart_has_valley(state, &segments) {
                    retain_open_apple_phrase(state, ev_tx, audio_secs);
                }
                // Preserve the recognizer pin as a diagnostic receipt. The
                // mirror paints this phrase at the tail regardless of its pin.
                let on_pcm = apple_segments_on_pcm_clock(state, &segments);
                let pin = match (
                    on_pcm.iter().map(|word| word.range.sample_start).min(),
                    on_pcm.iter().map(|word| word.range.sample_end).max(),
                ) {
                    (Some(sample_start), Some(sample_end)) => {
                        PreviewPin::from_segments(TailSampleRange {
                            session: state.session_id.clone(),
                            capture_epoch: state.capture_epoch,
                            sample_start,
                            sample_end,
                        })
                    }
                    _ => {
                        let captured_end = state.audio.session_sample_end();
                        let open_from = state.last_apple_segment_end;
                        let sample_start =
                            seconds_to_captured_sample(open_from, state.sample_rate, captured_end);
                        PreviewPin::open_occurrence(TailSampleRange {
                            session: state.session_id.clone(),
                            capture_epoch: state.capture_epoch,
                            sample_start,
                            sample_end: seconds_to_captured_sample(
                                audio_secs,
                                state.sample_rate,
                                captured_end,
                            )
                            .max(sample_start),
                        })
                    }
                };
                // Previews stay RAW: they are in-flight presentation, not
                // canvas, and correcting them would make the lexicon rewrite
                // flicker letter by letter while the phrase is still forming.
                // An empty recognition withdrawal is not evidence that the
                // preceding physical phrase disappeared. Keep it until a
                // measured close or a subsequent nonempty observation.
                if !text.trim().is_empty() {
                    state.open_partial = text.clone();
                    state.open_partial_segments = segments;
                }
                state.open_partial_pin = pin.range.sample_start;
                state.preview_rev = state.preview_rev.saturating_add(1);
                let _ = ev_tx.send(EngineEvent::Preview {
                    rev: state.preview_rev,
                    text,
                    pin,
                });
            }
            LiveStreamEvent::PhraseFinal { text, segments } => {
                state.observe_apple_progress(&text, &segments);
                // The phrase is closed either way — the open partial is stale.
                info!(
                    audio_secs,
                    sealed_before = state.sealed_count,
                    text_chars = text.len(),
                    "apple_lifecycle: phrase final received"
                );
                let superseded_through_rev = if state.open_partial.is_empty() {
                    0
                } else {
                    state.preview_rev
                };
                state.open_partial.clear();
                state.open_partial_segments.clear();
                let phrase_id = state.finishing_phrase_id.take().unwrap_or(state.phrase_id);
                let (committed, phrase_events) = close_apple_phrase(
                    state,
                    ev_tx,
                    phrase_id,
                    &text,
                    segments,
                    audio_secs,
                    superseded_through_rev != 0,
                );
                let (final_disposition, refused_evidence) =
                    phrase_final_disposition(&phrase_events, &text);
                let has_receipt = !phrase_events.is_empty() || superseded_through_rev != 0;
                if has_receipt {
                    let _ = ev_tx.send(EngineEvent::PreviewDisposition {
                        superseded_through_rev,
                        final_disposition,
                        refused_evidence,
                    });
                }
                info!(
                    audio_secs,
                    committed,
                    sealed_after = state.sealed_count,
                    "apple_lifecycle: phrase final adjudicated"
                );
            }
            LiveStreamEvent::Error { message } => {
                warn!("Apple live stream error event: {message}");
                let _ = ev_tx.send(EngineEvent::NoSpeech {
                    reason: format!("apple_live_stream: {message}"),
                });
            }
            LiveStreamEvent::Summary {
                text,
                segments,
                ok,
                error,
            } => {
                if !ok {
                    let msg = error.unwrap_or_else(|| "stream summary not ok".into());
                    warn!("Apple live stream summary error: {msg}");
                    let _ = ev_tx.send(EngineEvent::NoSpeech {
                        reason: format!("apple_live_stream_summary: {msg}"),
                    });
                    continue;
                }
                // Only the stop's trailing finish may add a summary after an
                // earlier seal. Epoch summaries retain the original gate.
                if state.stop_trailing_finish || state.utterance_id == 0 {
                    let closes_open = !state.open_partial.is_empty();
                    let phrase_id = if !closes_open {
                        state.last_closed_phrase_id.unwrap_or(state.phrase_id)
                    } else {
                        state.phrase_id
                    };
                    state.open_partial.clear();
                    state.open_partial_segments.clear();
                    close_apple_phrase(
                        state,
                        ev_tx,
                        phrase_id,
                        &text,
                        segments,
                        audio_secs,
                        closes_open,
                    );
                } else if !state.open_partial.trim().is_empty() {
                    // Preserve the existing summary admission gate. Its open
                    // phrase still closes explicitly, without inventing a seal.
                    let phrase_id = state.phrase_id;
                    let count = state.open_partial.split_whitespace().count();
                    state.closed_phrases.insert(
                        phrase_id,
                        ClosedApplePhrase {
                            arrival_index: state.closed_phrases.len(),
                            outcomes: BTreeMap::from([(ApplePhraseOutcome::NoChange, count)]),
                            was_untimed: false,
                        },
                    );
                    state.last_closed_phrase_id = Some(phrase_id);
                    state.phrase_id = state.phrase_id.saturating_add(1);
                    state.open_partial.clear();
                    state.open_partial_segments.clear();
                }
            }
        }
    }
    state.publish_unadmitted_words(ev_tx);
}

/// Raw Apple output is a producer only when the explicit Layer 1 choice left
/// Apple primary. Otherwise it is preview-only until that primary publishes;
/// it can never replace a Whisper translation or a consent-owned Cloud result.
fn emit_direct_apple_events(
    events: impl IntoIterator<Item = LiveStreamEvent>,
    ev_tx: &mpsc::UnboundedSender<EngineEvent>,
    state: &mut AppleSealState,
) {
    for event in events {
        if state.direct_end.is_none() {
            break;
        }
        match event {
            LiveStreamEvent::Ready => state.direct_apple_epoch_has_final = false,
            LiveStreamEvent::End => {}
            LiveStreamEvent::Partial { text, segments } => {
                if state.direct_producer != LedgerObservationProducer::Apple
                    && state.direct_sample_end > 0
                {
                    continue;
                }
                state.preview_rev = state.preview_rev.saturating_add(1);
                if state.direct_producer == LedgerObservationProducer::Apple {
                    // Retain one raw fallback for a transport that ends without
                    // a phrase final. No spelling, lexicon or label admission.
                    state.open_partial.clone_from(&text);
                    state.open_partial_segments = segments;
                    let observation = LedgerObservationIdentity::new(
                        LedgerObservationProducer::Apple,
                        state.phrase_id,
                        state.preview_rev,
                        OccurrenceIdentity::new(
                            &state.session_id,
                            state.capture_epoch,
                            state.direct_sample_end,
                            state.audio.session_sample_end(),
                        ),
                    );
                    state.publish_direct_transcript(ev_tx, observation, text, false);
                } else if state.direct_sample_end == 0 {
                    let _ = ev_tx.send(EngineEvent::Preview {
                        rev: state.preview_rev,
                        text,
                        pin: PreviewPin::open_occurrence(TailSampleRange {
                            session: state.session_id.clone(),
                            capture_epoch: state.capture_epoch,
                            sample_start: 0,
                            sample_end: state.audio.session_sample_end(),
                        }),
                    });
                }
            }
            LiveStreamEvent::Summary { ok: true, .. } if state.direct_apple_epoch_has_final => {
                // This is the whole recognizer epoch, not another physical
                // phrase. Any outstanding partial closes at the finish seam.
            }
            LiveStreamEvent::PhraseFinal { text, segments }
            | LiveStreamEvent::Summary {
                text,
                segments,
                ok: true,
                ..
            } => {
                state.direct_apple_epoch_has_final = true;
                state.open_partial.clear();
                state.open_partial_segments.clear();
                if state.direct_producer != LedgerObservationProducer::Apple {
                    continue;
                }
                let captured_end = state.audio.session_sample_end();
                let sample_end = segments
                    .iter()
                    .filter(|segment| {
                        segment.end_ts.is_finite()
                            && segment.start_ts.is_finite()
                            && segment.start_ts >= 0.0
                            && segment.end_ts > segment.start_ts
                    })
                    .map(|segment| {
                        seconds_to_captured_sample(segment.end_ts, state.sample_rate, captured_end)
                    })
                    .max()
                    .unwrap_or(captured_end);
                state.preview_rev = state.preview_rev.saturating_add(1);
                let observation = LedgerObservationIdentity::new(
                    LedgerObservationProducer::Apple,
                    state.phrase_id,
                    state.preview_rev,
                    OccurrenceIdentity::new(
                        &state.session_id,
                        state.capture_epoch,
                        state.direct_sample_end,
                        sample_end,
                    ),
                );
                if state.publish_direct_transcript(ev_tx, observation, text, true) {
                    state.phrase_id = state.phrase_id.saturating_add(1);
                }
            }
            LiveStreamEvent::Summary {
                ok: false, error, ..
            } => {
                let message = error.unwrap_or_else(|| "Apple returned no final summary.".into());
                if state.direct_producer == LedgerObservationProducer::Apple {
                    let _ = ev_tx.send(EngineEvent::NoSpeech { reason: message });
                    drop(state.direct_end.take());
                } else {
                    let _ = ev_tx.send(EngineEvent::Warning {
                        code: "direct_apple_summary_failed".into(),
                        message,
                    });
                }
            }
            LiveStreamEvent::Error { message } => {
                if state.direct_producer == LedgerObservationProducer::Apple {
                    let _ = ev_tx.send(EngineEvent::NoSpeech { reason: message });
                    drop(state.direct_end.take());
                } else {
                    let _ = ev_tx.send(EngineEvent::Warning {
                        code: "direct_apple_stream_failed".into(),
                        message,
                    });
                }
            }
        }
    }
}


#[cfg(test)]
#[path = "apple_live_session_tests/c13a_lifecycle_tests.rs"]
mod c13a_lifecycle_tests;

#[cfg(test)]
#[path = "apple_live_session_tests/layer1_frontier_contracts.rs"]
mod layer1_frontier_contracts;

#[cfg(test)]
#[path = "seal_coverage_tests.rs"]
mod seal_coverage_tests;

#[cfg(test)]
#[path = "live_speech_edge_tests.rs"]
mod live_speech_edge_tests;

#[cfg(test)]
#[path = "apple_live_session_tests/storm_tests.rs"]
mod storm_tests;

#[cfg(test)]
#[path = "apple_live_session_tests/rc_w1_live_ledger_tests.rs"]
mod rc_w1_live_ledger_tests;

#[cfg(test)]
#[path = "apple_live_session_tests/rc_w2_acoustic_tests.rs"]
mod rc_w2_acoustic_tests;

#[cfg(test)]
#[path = "apple_live_session_tests/rc_w2_test_rehab.rs"]
mod rc_w2_test_rehab;

#[cfg(test)]
#[path = "apple_live_session_tests/composer_turn_formatter_arming_tests.rs"]
mod composer_turn_formatter_arming_tests;

#[cfg(test)]
#[path = "apple_live_session_tests/live_refinement_admission_tests.rs"]
mod live_refinement_admission_tests;

#[cfg(test)]
#[path = "apple_live_session_tests/relay_l1_overlap_admission_tests.rs"]
mod relay_l1_overlap_admission_tests;

#[cfg(test)]
#[path = "apple_live_session_tests/tc2_window_contract_tests.rs"]
mod tc2_window_contract_tests;

#[cfg(test)]
#[path = "apple_live_session_tests/channel_epoch_gate.rs"]
mod channel_epoch_gate;

#[cfg(test)]
#[path = "apple_live_session_tests/word_no_speech_tests.rs"]
mod word_no_speech_tests;

#[cfg(feature = "test-isolation")]
#[path = "apple_live_session_tests/forensic_word_conservation_fixture.rs"]
mod forensic_word_conservation_fixture;

#[cfg(feature = "test-isolation")]
pub fn forensic_word_conservation_trace(
    fragmented_ingress: bool,
    contracted: bool,
) -> Vec<(
    crate::pipeline::acoustic_ledger::AcousticLedger,
    Vec<EngineEvent>,
)> {
    forensic_word_conservation_fixture::capture_trace(fragmented_ingress, contracted)
}

#[cfg(test)]
#[path = "capture_decode_budget_contract_tests.rs"]
mod capture_decode_budget_contract_tests;

#[cfg(test)]
#[path = "corroborated_lexicon_contract_tests.rs"]
mod corroborated_lexicon_contract_tests;

#[cfg(test)]
#[path = "transcription_controls_tests.rs"]
mod transcription_controls_tests;

#[cfg(test)]
#[path = "calibrated_energy_qualification_tests.rs"]
mod calibrated_energy_qualification_tests;

#[cfg(test)]
#[path = "native_forward_publication_tests.rs"]
mod native_forward_publication_tests;
