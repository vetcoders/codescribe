//! Supervisor findings — the engine-owned catalog of transcript-quality issues.
//!
//! Voice Lab's three-judge used to score Daily against candle HQ as if HQ were
//! the document. HQ and cloud are [`crate::quality::engine_contract::ReportSurfaceRole::HumanTriggeredProposal`].
//! This module names every quality issue the engine already knows, and classifies
//! a take into targeted, falsifiable findings a supervisor can act on.
//!
//! Rust is the lock. Voice Lab `judge.py` mirrors the take-evidence subset.

use serde::{Deserialize, Serialize};

use crate::quality::engine_contract::{ReportSurfaceRole, surface_role};
use crate::quality::teacher::{AlignOp, align_words, tokenize};

/// Schema id carried by every supervisor report / Voice Lab `supervisor` object.
pub const SUPERVISOR_FINDINGS_SCHEMA: &str = "codescribe-supervisor-findings/v1";

/// Product domain token the file/live loopback lanes must send.
pub const PROGRAMMING_VOCABULARY: &str = "programming";

/// Explicit bench opt-out. Omitting the field is not this.
pub const VOCABULARY_OFF: &str = "off";

/// Cap on per-lane attention findings so a long take stays readable.
const MAX_ATTENTION_FINDINGS: usize = 8;

/// Silence-corpus residue Whisper emits on empty audio. Must stay in the
/// same spirit as the decoder's hallucination diagnostics.
const SILENCE_CORPUS_RESIDUE: &[&str] = &[
    "thank you",
    "thanks for watching",
    "thanks for listening",
    "dziękuję za uwagę",
    "do zobaczenia",
    "subscribe",
    "like and subscribe",
    "napisy stworzone przez społeczność",
];

/// Families already named across the engine. Not a second doctrine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityIssueFamily {
    EngineContract,
    Clock,
    OverlayHighlight,
    TeacherAttention,
    Confidence,
    TranscriptState,
    DeliveryGate,
    WhisperFilter,
    JudgeHygiene,
}

/// Where a supervisor should cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingTarget {
    EngineCode,
    LabJudge,
    LexiconTune,
    OperatorReview,
}

/// Severity the supervisor ranks first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    P0,
    P1,
    P2,
    Note,
}

/// How strongly the evidence supports the claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceGrade {
    Strong,
    Medium,
    Weak,
    None,
}

/// Every transcript-quality issue category the engine already names.
///
/// Adding a variant without a catalog spec is a compile error (`spec` match).
/// Adding a spec without listing it in [`QualityIssueKind::ALL`] fails the
/// catalog-coverage test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityIssueKind {
    // ── Engine contract forbidden ops ────────────────────────────────────
    RewriteFromZero,
    ReorderSpans,
    HallucinateIntoSilence,
    FullFileInAutomaticPipeline,
    AutoReplaceAfterTranscriptSealed,
    TreatCommittedAsDocument,
    TreatWholeTextMutableUntilSessionSeal,
    // ── Clock / Seal Atlas ───────────────────────────────────────────────
    ClockLie,
    UtteranceGrainSilenceTail,
    LetterTimingAsMeasurement,
    ReconstructedTimeline,
    // ── Overlay highlights ───────────────────────────────────────────────
    LexiconCorrected,
    SpeechGap,
    // ── Teacher attention ────────────────────────────────────────────────
    LiveOnly,
    WhisperExcess,
    Disagreement,
    WhisperErrorAtLiveWeakness,
    LiveMissWhisperOk,
    // ── Confidence flags ─────────────────────────────────────────────────
    VeryLowSpeech,
    PossibleHallucinationLogprob,
    LocalFinalPassUnavailable,
    CloudFallbackUsed,
    StreamingPreviewUsedAsVerdict,
    UnverifiedStream,
    CloudPrimaryMissing,
    AiNoopDetected,
    FinalPassLengthRegression,
    HighCompression,
    // ── Transcript state ─────────────────────────────────────────────────
    NoSpeechDetected,
    EmptyTranscript,
    // ── Delivery gate ────────────────────────────────────────────────────
    RawFinalRewrite,
    LossyStreamDrops,
    HeavyCorrectionPressure,
    SemanticMeaningChange,
    // ── Whisper filter ───────────────────────────────────────────────────
    SilenceCorpusResidue,
    WordRateAnomaly,
    VadDegraded,
    ShortUtteranceDrop,
    // ── Judge hygiene (the lab lying) ────────────────────────────────────
    HqTreatedAsDocument,
    CloudTreatedAsDocument,
    WerPromotedToDocumentScore,
    OmittedProgrammingVocabulary,
    LastSessionPairedWithLiveOverlay,
    LeftoverWebsocketPolarity,
    ProposalAgreementMisreadAsAccuracy,
}

/// Static catalog row. Wire id is [`QualityIssueKind`] serde / [`QualityIssueKind::as_str`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct QualityIssueSpec {
    pub kind: QualityIssueKind,
    pub family: QualityIssueFamily,
    pub default_severity: FindingSeverity,
    pub default_target: FindingTarget,
    pub what: &'static str,
    pub falsifier: &'static str,
    pub action: &'static str,
}

/// One token locus a finding points at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingSpan {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub daily_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposal_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposal_lane: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub word_index: Option<usize>,
}

/// One targeted finding. Claim + falsifier + action are required.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisorFinding {
    pub kind: QualityIssueKind,
    pub family: QualityIssueFamily,
    pub severity: FindingSeverity,
    pub evidence_grade: EvidenceGrade,
    pub target: FindingTarget,
    pub claim: String,
    pub falsifier: String,
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<FindingSpan>,
}

/// Surface roles for the three-judge lanes. WER does not change these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisorRoles {
    pub daily: ReportSurfaceRole,
    pub candle: ReportSurfaceRole,
    pub cloud: ReportSurfaceRole,
}

/// Supervisor payload Voice Lab embeds next to three-judge WER (the footnote).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisorReport {
    pub schema: String,
    pub roles: SupervisorRoles,
    pub findings: Vec<SupervisorFinding>,
    pub catalog_ids: Vec<String>,
    pub wer_is_footnote: bool,
}

/// Evidence a take (or a lying judge) can actually show.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TakeQualityEvidence {
    pub daily_text: String,
    pub hq_text: String,
    pub cloud_text: String,
    pub snapshot_live: bool,
    pub audio_from_last_session: bool,
    pub vocabulary: Option<String>,
    pub cloud_ran: bool,
    /// True when the judge scores Daily as if HQ were the document.
    pub treats_hq_as_document: bool,
    /// True when first_divergence still labels HQ as `websocket` / Daily as `codescribe`.
    pub leftover_websocket_polarity: bool,
    pub clock_lie_count: usize,
    pub speech_gap_count: usize,
    pub confidence_flags: Vec<String>,
}

impl QualityIssueKind {
    /// Exhaustive catalog order. Coverage test walks this slice.
    pub const ALL: &'static [Self] = &[
        Self::RewriteFromZero,
        Self::ReorderSpans,
        Self::HallucinateIntoSilence,
        Self::FullFileInAutomaticPipeline,
        Self::AutoReplaceAfterTranscriptSealed,
        Self::TreatCommittedAsDocument,
        Self::TreatWholeTextMutableUntilSessionSeal,
        Self::ClockLie,
        Self::UtteranceGrainSilenceTail,
        Self::LetterTimingAsMeasurement,
        Self::ReconstructedTimeline,
        Self::LexiconCorrected,
        Self::SpeechGap,
        Self::LiveOnly,
        Self::WhisperExcess,
        Self::Disagreement,
        Self::WhisperErrorAtLiveWeakness,
        Self::LiveMissWhisperOk,
        Self::VeryLowSpeech,
        Self::PossibleHallucinationLogprob,
        Self::LocalFinalPassUnavailable,
        Self::CloudFallbackUsed,
        Self::StreamingPreviewUsedAsVerdict,
        Self::UnverifiedStream,
        Self::CloudPrimaryMissing,
        Self::AiNoopDetected,
        Self::FinalPassLengthRegression,
        Self::HighCompression,
        Self::NoSpeechDetected,
        Self::EmptyTranscript,
        Self::RawFinalRewrite,
        Self::LossyStreamDrops,
        Self::HeavyCorrectionPressure,
        Self::SemanticMeaningChange,
        Self::SilenceCorpusResidue,
        Self::WordRateAnomaly,
        Self::VadDegraded,
        Self::ShortUtteranceDrop,
        Self::HqTreatedAsDocument,
        Self::CloudTreatedAsDocument,
        Self::WerPromotedToDocumentScore,
        Self::OmittedProgrammingVocabulary,
        Self::LastSessionPairedWithLiveOverlay,
        Self::LeftoverWebsocketPolarity,
        Self::ProposalAgreementMisreadAsAccuracy,
    ];

    /// Wire token. Matches serde `snake_case`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RewriteFromZero => "rewrite_from_zero",
            Self::ReorderSpans => "reorder_spans",
            Self::HallucinateIntoSilence => "hallucinate_into_silence",
            Self::FullFileInAutomaticPipeline => "full_file_in_automatic_pipeline",
            Self::AutoReplaceAfterTranscriptSealed => "auto_replace_after_transcript_sealed",
            Self::TreatCommittedAsDocument => "treat_committed_as_document",
            Self::TreatWholeTextMutableUntilSessionSeal => {
                "treat_whole_text_mutable_until_session_seal"
            }
            Self::ClockLie => "clock_lie",
            Self::UtteranceGrainSilenceTail => "utterance_grain_silence_tail",
            Self::LetterTimingAsMeasurement => "letter_timing_as_measurement",
            Self::ReconstructedTimeline => "reconstructed_timeline",
            Self::LexiconCorrected => "lexicon_corrected",
            Self::SpeechGap => "speech_gap",
            Self::LiveOnly => "live_only",
            Self::WhisperExcess => "whisper_excess",
            Self::Disagreement => "disagreement",
            Self::WhisperErrorAtLiveWeakness => "whisper_error_at_live_weakness",
            Self::LiveMissWhisperOk => "live_miss_whisper_ok",
            Self::VeryLowSpeech => "very_low_speech",
            Self::PossibleHallucinationLogprob => "possible_hallucination_logprob",
            Self::LocalFinalPassUnavailable => "local_final_pass_unavailable",
            Self::CloudFallbackUsed => "cloud_fallback_used",
            Self::StreamingPreviewUsedAsVerdict => "streaming_preview_used_as_verdict",
            Self::UnverifiedStream => "unverified_stream",
            Self::CloudPrimaryMissing => "cloud_primary_missing",
            Self::AiNoopDetected => "ai_noop_detected",
            Self::FinalPassLengthRegression => "final_pass_length_regression",
            Self::HighCompression => "high_compression",
            Self::NoSpeechDetected => "no_speech_detected",
            Self::EmptyTranscript => "empty_transcript",
            Self::RawFinalRewrite => "raw_final_rewrite",
            Self::LossyStreamDrops => "lossy_stream_drops",
            Self::HeavyCorrectionPressure => "heavy_correction_pressure",
            Self::SemanticMeaningChange => "semantic_meaning_change",
            Self::SilenceCorpusResidue => "silence_corpus_residue",
            Self::WordRateAnomaly => "word_rate_anomaly",
            Self::VadDegraded => "vad_degraded",
            Self::ShortUtteranceDrop => "short_utterance_drop",
            Self::HqTreatedAsDocument => "hq_treated_as_document",
            Self::CloudTreatedAsDocument => "cloud_treated_as_document",
            Self::WerPromotedToDocumentScore => "wer_promoted_to_document_score",
            Self::OmittedProgrammingVocabulary => "omitted_programming_vocabulary",
            Self::LastSessionPairedWithLiveOverlay => "last_session_paired_with_live_overlay",
            Self::LeftoverWebsocketPolarity => "leftover_websocket_polarity",
            Self::ProposalAgreementMisreadAsAccuracy => "proposal_agreement_misread_as_accuracy",
        }
    }

    /// Catalog row for this kind.
    pub const fn spec(self) -> QualityIssueSpec {
        match self {
            Self::RewriteFromZero => spec(
                self,
                QualityIssueFamily::EngineContract,
                FindingSeverity::P0,
                FindingTarget::EngineCode,
                "A layer rebuilt the document from tokens instead of appending on pcm_time.",
                "Show the span ledger still ordered on the original PCM ranges.",
                "Restore append-only ReplaceRange on the sealed span key.",
            ),
            Self::ReorderSpans => spec(
                self,
                QualityIssueFamily::EngineContract,
                FindingSeverity::P0,
                FindingTarget::EngineCode,
                "Utterances were reordered off the PCM axis.",
                "Replay the take: sealed spans stay in capture order.",
                "Stop any sort/merge that is not keyed by sample_start.",
            ),
            Self::HallucinateIntoSilence => spec(
                self,
                QualityIssueFamily::EngineContract,
                FindingSeverity::P0,
                FindingTarget::EngineCode,
                "A later layer invented speech inside Silero silence.",
                "Silero p(speech) on that PCM range is below onset, and Apple sealed empty.",
                "Ban Whisper/cloud from writing into a silence-classified hole.",
            ),
            Self::FullFileInAutomaticPipeline => spec(
                self,
                QualityIssueFamily::EngineContract,
                FindingSeverity::P0,
                FindingTarget::EngineCode,
                "A full-file pass ran as if it were the live engine.",
                "No automatic stop-path invoked codescribe transcribe / :8444.",
                "Keep full-file as button_only_proposal.",
            ),
            Self::AutoReplaceAfterTranscriptSealed => spec(
                self,
                QualityIssueFamily::EngineContract,
                FindingSeverity::P0,
                FindingTarget::EngineCode,
                "Automation replaced the session document after transcript_sealed.",
                "Post-seal HQ/cloud stayed a proposal the human must accept.",
                "Fence auto-apply on the seal event.",
            ),
            Self::TreatCommittedAsDocument => spec(
                self,
                QualityIssueFamily::EngineContract,
                FindingSeverity::P1,
                FindingTarget::EngineCode,
                "A layer-final commit was treated as the session document.",
                "utterance_final stayed a per-layer ban, not transcript_sealed.",
                "Do not paste or score committed as delivered.",
            ),
            Self::TreatWholeTextMutableUntilSessionSeal => spec(
                self,
                QualityIssueFamily::EngineContract,
                FindingSeverity::P0,
                FindingTarget::EngineCode,
                "Closed spans were mutated as if the whole buffer were still open.",
                "Sealed [sample_start, sample_end) stayed append-only.",
                "Restrict mutation to the open tail.",
            ),
            Self::ClockLie => spec(
                self,
                QualityIssueFamily::Clock,
                FindingSeverity::P1,
                FindingTarget::EngineCode,
                "A span claims more characters than speech can produce in that PCM duration.",
                "chars/sec over the span range is ≤ CLOCK_LIE_CHARS_PER_SEC (30).",
                "Treat the range as an Apple commit window, not the speech outline.",
            ),
            Self::UtteranceGrainSilenceTail => spec(
                self,
                QualityIssueFamily::Clock,
                FindingSeverity::P2,
                FindingTarget::OperatorReview,
                "Utterance-grain span includes the silence tail between Apple commits.",
                "word-grain pins exist, or Silero edges bound the speech.",
                "Do not mint identity from the commit-to-commit window.",
            ),
            Self::LetterTimingAsMeasurement => spec(
                self,
                QualityIssueFamily::Clock,
                FindingSeverity::P2,
                FindingTarget::LabJudge,
                "Grapheme ticks were presented as measured times.",
                "HTML/report labels letter ticks as interpolation_not_measurement.",
                "Strip any forced-aligner claim we do not have.",
            ),
            Self::ReconstructedTimeline => spec(
                self,
                QualityIssueFamily::Clock,
                FindingSeverity::P1,
                FindingTarget::LabJudge,
                "A quality surface rebuilt time from the final string instead of PCM.",
                "Words come from SealedSpan.words / the live dump on pcm_time.",
                "Refuse reports that reconstruct a timeline from tokens.",
            ),
            Self::LexiconCorrected => spec(
                self,
                QualityIssueFamily::OverlayHighlight,
                FindingSeverity::Note,
                FindingTarget::LexiconTune,
                "A lexicon rewrite already landed on committed text.",
                "The before/after pair is absent from lexicon.custom.jsonl.",
                "Keep the rule if the next take still needs it; drop if it over-fires.",
            ),
            Self::SpeechGap => spec(
                self,
                QualityIssueFamily::OverlayHighlight,
                FindingSeverity::P2,
                FindingTarget::EngineCode,
                "Silero heard speech and no engine word landed in the span.",
                "A word sample range overlaps the speech range, or Silero was wrong.",
                "Fill the pustka with Layer 1 ReplaceRange; do not invent into silence.",
            ),
            Self::LiveOnly => spec(
                self,
                QualityIssueFamily::TeacherAttention,
                FindingSeverity::P2,
                FindingTarget::OperatorReview,
                "Daily/live kept a token the proposal dropped.",
                "Human reference agrees the token was not said, or HQ/cloud both drop it for a reason.",
                "Do not delete the live token automatically. Review Apple residue vs Whisper drop.",
            ),
            Self::WhisperExcess => spec(
                self,
                QualityIssueFamily::TeacherAttention,
                FindingSeverity::P1,
                FindingTarget::EngineCode,
                "A proposal inserted tokens absent from the daily document.",
                "Human said those tokens, or Silero shows a hole Apple left.",
                "Allow hole-fill only inside unsealed allowed spans. Tune Layer 1, not Daily WER.",
            ),
            Self::Disagreement => spec(
                self,
                QualityIssueFamily::TeacherAttention,
                FindingSeverity::P2,
                FindingTarget::LexiconTune,
                "Daily and a proposal disagree on a token. This is not accuracy.",
                "Human reference picks one side, or a lexicon rule already owns the pair.",
                "If it is jargon (Rust/raz), teach the custom lexicon. Do not crown HQ.",
            ),
            Self::WhisperErrorAtLiveWeakness => spec(
                self,
                QualityIssueFamily::TeacherAttention,
                FindingSeverity::P1,
                FindingTarget::EngineCode,
                "Proposal disagrees with a human at a locus live also missed.",
                "Human text is absent, or live actually carried the human form.",
                "This is the Teacher thesis site — gap-fill, not a WER hero score.",
            ),
            Self::LiveMissWhisperOk => spec(
                self,
                QualityIssueFamily::TeacherAttention,
                FindingSeverity::P2,
                FindingTarget::EngineCode,
                "Live missed a human token the proposal carried.",
                "Human text is absent, or live actually had the form.",
                "Classic Apple under-gen. Layer 1 may fill; do not replace the floor.",
            ),
            Self::VeryLowSpeech => spec(
                self,
                QualityIssueFamily::Confidence,
                FindingSeverity::P1,
                FindingTarget::OperatorReview,
                "VAD speech share is at/below the very-low-speech floor.",
                "speech_pct is above the engine threshold on the same take.",
                "Do not trust a long transcript on near-silence.",
            ),
            Self::PossibleHallucinationLogprob => spec(
                self,
                QualityIssueFamily::Confidence,
                FindingSeverity::P1,
                FindingTarget::EngineCode,
                "avg_logprob crossed the hallucination ceiling.",
                "avg_logprob is above -1.0, or the text is short-whitelist speech.",
                "Inspect the span before teaching lexicon; this score is diagnostic only.",
            ),
            Self::LocalFinalPassUnavailable => spec(
                self,
                QualityIssueFamily::Confidence,
                FindingSeverity::P2,
                FindingTarget::OperatorReview,
                "Hold path asked for a local file pass and did not get a verdict.",
                "codescribe transcribe produced a verdict for the same WAV.",
                "Do not pretend HQ ran.",
            ),
            Self::CloudFallbackUsed => spec(
                self,
                QualityIssueFamily::Confidence,
                FindingSeverity::P2,
                FindingTarget::OperatorReview,
                "Cloud was committed after the local path failed.",
                "Local produced a usable verdict on the same take.",
                "Label the lane degraded. Cloud is not a silent upgrade.",
            ),
            Self::StreamingPreviewUsedAsVerdict => spec(
                self,
                QualityIssueFamily::Confidence,
                FindingSeverity::P1,
                FindingTarget::EngineCode,
                "Streaming preview was frozen as the verdict.",
                "A final-pass disposition exists.",
                "Do not score preview as transcript_sealed.",
            ),
            Self::UnverifiedStream => spec(
                self,
                QualityIssueFamily::Confidence,
                FindingSeverity::P2,
                FindingTarget::OperatorReview,
                "Stream text was exposed before final-pass adjudication.",
                "An explicit final-pass ran.",
                "Keep the preview grey until a bar is crossed.",
            ),
            Self::CloudPrimaryMissing => spec(
                self,
                QualityIssueFamily::Confidence,
                FindingSeverity::P1,
                FindingTarget::OperatorReview,
                "Cloud was the primary source and returned empty/error.",
                "The cloud call returned non-empty text.",
                "Refuse to treat a blank cloud lane as a document.",
            ),
            Self::AiNoopDetected => spec(
                self,
                QualityIssueFamily::Confidence,
                FindingSeverity::Note,
                FindingTarget::OperatorReview,
                "Format ran and emitted the raw input.",
                "The formatted text actually differs in content.",
                "Do not display Format as Applied.",
            ),
            Self::FinalPassLengthRegression => spec(
                self,
                QualityIssueFamily::Confidence,
                FindingSeverity::P0,
                FindingTarget::EngineCode,
                "File final collapsed versus the live streaming floor.",
                "final kept ≥40% of stream chars, or stream was below the min floor.",
                "Keep the stream. Never auto-replace with the collapse.",
            ),
            Self::HighCompression => spec(
                self,
                QualityIssueFamily::Confidence,
                FindingSeverity::P2,
                FindingTarget::EngineCode,
                "Whisper compression_ratio crossed the diagnostic threshold.",
                "compression_ratio is below the engine threshold.",
                "Pair with logprob. Do not teach lexicon from a compressed dump.",
            ),
            Self::NoSpeechDetected => spec(
                self,
                QualityIssueFamily::TranscriptState,
                FindingSeverity::Note,
                FindingTarget::OperatorReview,
                "VAD found no speech. This is not an empty-transcript failure.",
                "Silero frames show speech, or the operator spoke.",
                "Do not score WER against a no-speech take.",
            ),
            Self::EmptyTranscript => spec(
                self,
                QualityIssueFamily::TranscriptState,
                FindingSeverity::P1,
                FindingTarget::EngineCode,
                "Daily document is empty with no no-speech reason on record.",
                "A no_speech_reason exists, or Daily has text.",
                "Investigate attribution. Do not fill with HQ automatically.",
            ),
            Self::RawFinalRewrite => spec(
                self,
                QualityIssueFamily::DeliveryGate,
                FindingSeverity::P1,
                FindingTarget::EngineCode,
                "Raw→final character rewrite crossed the delivery gate.",
                "shape_only punctuation, or diff_ratio below QUALITY_GATE_DIFF_RATIO.",
                "Inspect Format/lexicon. Do not ship a silent rewrite.",
            ),
            Self::LossyStreamDrops => spec(
                self,
                QualityIssueFamily::DeliveryGate,
                FindingSeverity::P1,
                FindingTarget::EngineCode,
                "Stream drop_ratio crossed the lossy threshold.",
                "dropped_chunks/input_chunks is below QUALITY_GATE_DROP_RATIO.",
                "Fix the stream. Do not blame STT wording.",
            ),
            Self::HeavyCorrectionPressure => spec(
                self,
                QualityIssueFamily::DeliveryGate,
                FindingSeverity::P2,
                FindingTarget::OperatorReview,
                "Backspace/correction ratio crossed the delivery gate.",
                "correction_ratio is below QUALITY_GATE_CORRECTION_RATIO.",
                "This is operator pressure, not a WER score.",
            ),
            Self::SemanticMeaningChange => spec(
                self,
                QualityIssueFamily::DeliveryGate,
                FindingSeverity::P1,
                FindingTarget::EngineCode,
                "MiniLM cosine says Format changed meaning while length stayed similar.",
                "semantic_cosine is None (too short) or above the calibrated floor.",
                "Quarantine Format. Daily/raw stays the document.",
            ),
            Self::SilenceCorpusResidue => spec(
                self,
                QualityIssueFamily::WhisperFilter,
                FindingSeverity::P1,
                FindingTarget::EngineCode,
                "A proposal matches Whisper silence-corpus residue Daily does not have.",
                "Daily also contains the phrase, or Silero shows real speech there.",
                "Drop the residue. Do not teach it into the lexicon.",
            ),
            Self::WordRateAnomaly => spec(
                self,
                QualityIssueFamily::WhisperFilter,
                FindingSeverity::P2,
                FindingTarget::EngineCode,
                "Words/sec exceeded MAX_WORDS_PER_SEC — clock or dump anomaly.",
                "Rate is ≤ 5 w/s over a sample with ≥6 words.",
                "Same family as clock-lie. Do not treat as fluent speech.",
            ),
            Self::VadDegraded => spec(
                self,
                QualityIssueFamily::WhisperFilter,
                FindingSeverity::P2,
                FindingTarget::EngineCode,
                "VAD predict_errors / unavailable_frames fired on the batch.",
                "vad_degraded warning is absent on a clean replay.",
                "Do not trust speech_gap / silence calls on a degraded VAD batch.",
            ),
            Self::ShortUtteranceDrop => spec(
                self,
                QualityIssueFamily::WhisperFilter,
                FindingSeverity::Note,
                FindingTarget::OperatorReview,
                "A sub-0.5s low-confidence clip was dropped as a click/breath.",
                "Duration ≥ 0.5s or Silero speech_prob ≥ 0.55.",
                "Whitelist short Polish speech (tak/nie/no) must never hit this.",
            ),
            Self::HqTreatedAsDocument => spec(
                self,
                QualityIssueFamily::JudgeHygiene,
                FindingSeverity::P0,
                FindingTarget::LabJudge,
                "The judge treated candle HQ as the document / WER reference.",
                "roles.candle is human_triggered_proposal and WER is a footnote.",
                "Stop scoring Daily as if it must chase HQ. HQ is a button-only proposal.",
            ),
            Self::CloudTreatedAsDocument => spec(
                self,
                QualityIssueFamily::JudgeHygiene,
                FindingSeverity::P0,
                FindingTarget::LabJudge,
                "The judge treated cloud :8444 as the document.",
                "roles.cloud is human_triggered_proposal.",
                "Cloud file is a proposal. Daily remains the session document.",
            ),
            Self::WerPromotedToDocumentScore => spec(
                self,
                QualityIssueFamily::JudgeHygiene,
                FindingSeverity::P0,
                FindingTarget::LabJudge,
                "WER was presented as the quality score of the live engine.",
                "WER sits behind wer_is_footnote and a Seal Atlas / findings payload.",
                "Demote the hero WER. Findings first.",
            ),
            Self::OmittedProgrammingVocabulary => spec(
                self,
                QualityIssueFamily::JudgeHygiene,
                FindingSeverity::P1,
                FindingTarget::LabJudge,
                "Cloud :8444 ran without vocabulary=programming (or explicit off).",
                "Multipart/live config carries vocabulary=programming, or off for an unbiased bench.",
                "Send the product domain token. Omitting it is not a silent default.",
            ),
            Self::LastSessionPairedWithLiveOverlay => spec(
                self,
                QualityIssueFamily::JudgeHygiene,
                FindingSeverity::P0,
                FindingTarget::LabJudge,
                "Live Daily was scored against last_session.wav from the previous take.",
                "Live snapshots use this take's wav_path/audio_path, never last_session.wav.",
                "Refuse the compare. Fake 90%+ WER is not a finding against Daily.",
            ),
            Self::LeftoverWebsocketPolarity => spec(
                self,
                QualityIssueFamily::JudgeHygiene,
                FindingSeverity::P1,
                FindingTarget::LabJudge,
                "first_divergence still labels sides websocket/codescribe regardless of lane.",
                "Divergence keys are reference/hypothesis only.",
                "Delete the leftover polarity. It inverts HQ vs Daily.",
            ),
            Self::ProposalAgreementMisreadAsAccuracy => spec(
                self,
                QualityIssueFamily::JudgeHygiene,
                FindingSeverity::P1,
                FindingTarget::LabJudge,
                "Agreement with a proposal was reported as accuracy against what was said.",
                "No human/corpus reference was claimed, and roles stay proposal.",
                "Call it proposal_agreement. Accuracy requires a human transcript.",
            ),
        }
    }
}

const fn spec(
    kind: QualityIssueKind,
    family: QualityIssueFamily,
    default_severity: FindingSeverity,
    default_target: FindingTarget,
    what: &'static str,
    falsifier: &'static str,
    action: &'static str,
) -> QualityIssueSpec {
    QualityIssueSpec {
        kind,
        family,
        default_severity,
        default_target,
        what,
        falsifier,
        action,
    }
}

/// Every catalog row, in [`QualityIssueKind::ALL`] order.
pub fn quality_issue_catalog() -> Vec<QualityIssueSpec> {
    QualityIssueKind::ALL
        .iter()
        .copied()
        .map(QualityIssueKind::spec)
        .collect()
}

/// Wire ids for the Voice Lab lockstep list.
pub fn quality_issue_kind_ids() -> Vec<&'static str> {
    QualityIssueKind::ALL
        .iter()
        .copied()
        .map(QualityIssueKind::as_str)
        .collect()
}

/// Classify one take. Missing evidence does not invent a hit.
pub fn classify_take_findings(evidence: &TakeQualityEvidence) -> SupervisorReport {
    let mut findings = Vec::new();
    findings.extend(hygiene_findings(evidence));
    findings.extend(count_findings(evidence));
    findings.extend(flag_findings(evidence));
    findings.extend(empty_daily_findings(evidence));
    findings.extend(residue_findings(evidence));
    findings.extend(attention_findings(
        &evidence.daily_text,
        &evidence.hq_text,
        "hq",
    ));
    findings.extend(attention_findings(
        &evidence.daily_text,
        &evidence.cloud_text,
        "cloud",
    ));

    SupervisorReport {
        schema: SUPERVISOR_FINDINGS_SCHEMA.to_string(),
        roles: SupervisorRoles {
            daily: ReportSurfaceRole::SessionDocument,
            candle: ReportSurfaceRole::HumanTriggeredProposal,
            cloud: ReportSurfaceRole::HumanTriggeredProposal,
        },
        findings,
        catalog_ids: quality_issue_kind_ids()
            .into_iter()
            .map(str::to_string)
            .collect(),
        wer_is_footnote: true,
    }
}

fn hygiene_findings(evidence: &TakeQualityEvidence) -> Vec<SupervisorFinding> {
    let mut out = Vec::new();
    if evidence.treats_hq_as_document {
        out.push(finding(
            QualityIssueKind::HqTreatedAsDocument,
            EvidenceGrade::Strong,
            "Judge scored Daily against candle HQ as if HQ were the document.".into(),
        ));
        out.push(finding(
            QualityIssueKind::WerPromotedToDocumentScore,
            EvidenceGrade::Strong,
            "Daily-vs-HQ WER was used as the live-engine quality score.".into(),
        ));
        out.push(finding(
            QualityIssueKind::ProposalAgreementMisreadAsAccuracy,
            EvidenceGrade::Strong,
            "Proposal agreement was reported as accuracy against what was said.".into(),
        ));
    }
    if evidence.leftover_websocket_polarity {
        out.push(finding(
            QualityIssueKind::LeftoverWebsocketPolarity,
            EvidenceGrade::Strong,
            "Divergence still carries websocket/codescribe keys that invert lane names.".into(),
        ));
    }
    if evidence.snapshot_live && evidence.audio_from_last_session {
        out.push(finding(
            QualityIssueKind::LastSessionPairedWithLiveOverlay,
            EvidenceGrade::Strong,
            "Live overlay text was paired with last_session.wav from another take.".into(),
        ));
    }
    if evidence.cloud_ran && !vocabulary_is_honest(evidence.vocabulary.as_deref()) {
        let got = evidence.vocabulary.as_deref().unwrap_or("<omitted>");
        out.push(finding(
            QualityIssueKind::OmittedProgrammingVocabulary,
            EvidenceGrade::Strong,
            format!(
                "Cloud :8444 ran with vocabulary={got}; product default is {PROGRAMMING_VOCABULARY} (or explicit {VOCABULARY_OFF})."
            ),
        ));
    }
    out
}

fn vocabulary_is_honest(value: Option<&str>) -> bool {
    matches!(
        value.map(str::trim),
        Some(PROGRAMMING_VOCABULARY) | Some(VOCABULARY_OFF)
    )
}

fn count_findings(evidence: &TakeQualityEvidence) -> Vec<SupervisorFinding> {
    let mut out = Vec::new();
    if evidence.clock_lie_count > 0 {
        out.push(finding(
            QualityIssueKind::ClockLie,
            EvidenceGrade::Strong,
            format!(
                "{} sealed span(s) exceed CLOCK_LIE_CHARS_PER_SEC.",
                evidence.clock_lie_count
            ),
        ));
    }
    if evidence.speech_gap_count > 0 {
        out.push(finding(
            QualityIssueKind::SpeechGap,
            EvidenceGrade::Strong,
            format!(
                "{} Silero speech span(s) have no overlapping engine word.",
                evidence.speech_gap_count
            ),
        ));
    }
    out
}

fn flag_findings(evidence: &TakeQualityEvidence) -> Vec<SupervisorFinding> {
    evidence
        .confidence_flags
        .iter()
        .filter_map(|flag| kind_for_flag(flag).map(|kind| (flag.as_str(), kind)))
        .map(|(flag, kind)| {
            finding(
                kind,
                EvidenceGrade::Strong,
                format!("Take carries confidence flag `{flag}`."),
            )
        })
        .collect()
}

fn kind_for_flag(flag: &str) -> Option<QualityIssueKind> {
    let token = flag.trim();
    match token {
        "very_low_speech" => Some(QualityIssueKind::VeryLowSpeech),
        "possible_hallucination_logprob" => Some(QualityIssueKind::PossibleHallucinationLogprob),
        "local_final_pass_unavailable" => Some(QualityIssueKind::LocalFinalPassUnavailable),
        "cloud_fallback_used" => Some(QualityIssueKind::CloudFallbackUsed),
        "streaming_preview_used_as_verdict" => {
            Some(QualityIssueKind::StreamingPreviewUsedAsVerdict)
        }
        "unverified_stream" => Some(QualityIssueKind::UnverifiedStream),
        "cloud_primary_missing" => Some(QualityIssueKind::CloudPrimaryMissing),
        "ai_noop_detected" => Some(QualityIssueKind::AiNoopDetected),
        "final_pass_length_regression" => Some(QualityIssueKind::FinalPassLengthRegression),
        "high_compression" | "low_logprob" => {
            if token == "high_compression" {
                Some(QualityIssueKind::HighCompression)
            } else {
                Some(QualityIssueKind::PossibleHallucinationLogprob)
            }
        }
        _ => None,
    }
}

fn empty_daily_findings(evidence: &TakeQualityEvidence) -> Vec<SupervisorFinding> {
    if !evidence.daily_text.trim().is_empty() {
        return Vec::new();
    }
    if evidence
        .confidence_flags
        .iter()
        .any(|flag| flag == "no_speech_detected")
    {
        return vec![finding(
            QualityIssueKind::NoSpeechDetected,
            EvidenceGrade::Medium,
            "Daily is empty and the take is flagged no_speech_detected.".into(),
        )];
    }
    vec![finding(
        QualityIssueKind::EmptyTranscript,
        EvidenceGrade::Medium,
        "Daily session document is empty with no no-speech reason.".into(),
    )]
}

fn residue_findings(evidence: &TakeQualityEvidence) -> Vec<SupervisorFinding> {
    let daily = evidence.daily_text.to_ascii_lowercase();
    let mut out = Vec::new();
    for (lane, text) in [
        ("hq", evidence.hq_text.as_str()),
        ("cloud", evidence.cloud_text.as_str()),
    ] {
        let lowered = text.to_ascii_lowercase();
        if let Some(phrase) = SILENCE_CORPUS_RESIDUE
            .iter()
            .copied()
            .find(|phrase| lowered.contains(phrase) && !daily.contains(phrase))
        {
            let kind = if evidence.daily_text.trim().is_empty() {
                QualityIssueKind::HallucinateIntoSilence
            } else {
                QualityIssueKind::SilenceCorpusResidue
            };
            let mut row = finding(
                kind,
                EvidenceGrade::Medium,
                format!(
                    "`{lane}` proposal contains silence-corpus residue `{phrase}` absent from Daily."
                ),
            );
            row.span = Some(FindingSpan {
                daily_token: None,
                proposal_token: Some(phrase.to_string()),
                proposal_lane: Some(lane.to_string()),
                word_index: None,
            });
            out.push(row);
        }
    }
    out
}

fn attention_findings(daily: &str, proposal: &str, lane: &str) -> Vec<SupervisorFinding> {
    if daily.trim().is_empty() || proposal.trim().is_empty() {
        return Vec::new();
    }
    let live = tokenize(daily);
    let other = tokenize(proposal);
    let ops = align_words(&live, &other);
    let mut out = Vec::new();
    let mut kept = 0usize;
    let mut truncated = 0usize;
    for op in ops {
        let row = match op {
            AlignOp::Equal { .. } => None,
            AlignOp::DeleteA { a } => Some(attention_row(
                QualityIssueKind::LiveOnly,
                lane,
                Some(live[a].surface.clone()),
                None,
                Some(a + 1),
                format!(
                    "Daily kept «{}»; `{lane}` proposal has no counterpart.",
                    live[a].surface
                ),
            )),
            AlignOp::InsertB { b } => Some(attention_row(
                QualityIssueKind::WhisperExcess,
                lane,
                None,
                Some(other[b].surface.clone()),
                Some(b + 1),
                format!(
                    "`{lane}` proposal inserted «{}» absent from the Daily document.",
                    other[b].surface
                ),
            )),
            AlignOp::Substitute { a, b } => Some(attention_row(
                QualityIssueKind::Disagreement,
                lane,
                Some(live[a].surface.clone()),
                Some(other[b].surface.clone()),
                Some(a + 1),
                format!(
                    "Daily «{}» vs `{lane}` proposal «{}» — agreement, not accuracy.",
                    live[a].surface, other[b].surface
                ),
            )),
        };
        if let Some(row) = row {
            if kept < MAX_ATTENTION_FINDINGS {
                out.push(row);
                kept += 1;
            } else {
                truncated += 1;
            }
        }
    }
    if truncated > 0 {
        out.push(finding(
            QualityIssueKind::Disagreement,
            EvidenceGrade::Medium,
            format!(
                "{truncated} further `{lane}` attention loci truncated at {MAX_ATTENTION_FINDINGS}."
            ),
        ));
    }
    out
}

fn attention_row(
    kind: QualityIssueKind,
    lane: &str,
    daily_token: Option<String>,
    proposal_token: Option<String>,
    word_index: Option<usize>,
    claim: String,
) -> SupervisorFinding {
    let mut row = finding(kind, EvidenceGrade::Medium, claim);
    row.span = Some(FindingSpan {
        daily_token,
        proposal_token,
        proposal_lane: Some(lane.to_string()),
        word_index,
    });
    row
}

fn finding(
    kind: QualityIssueKind,
    evidence_grade: EvidenceGrade,
    claim: String,
) -> SupervisorFinding {
    let spec = kind.spec();
    SupervisorFinding {
        kind,
        family: spec.family,
        severity: spec.default_severity,
        evidence_grade,
        target: spec.default_target,
        claim,
        falsifier: spec.falsifier.to_string(),
        action: spec.action.to_string(),
        span: None,
    }
}

/// Column roles the judge must not invert. Re-export of the contract helper
/// so Voice Lab lockstep tests and this classifier share one function.
pub fn lane_surface_role(column: &str) -> Option<ReportSurfaceRole> {
    surface_role(column)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quality::engine_contract::{ENGINE_CONTRACT_DOC, is_clock_lie};
    use std::collections::HashSet;

    #[test]
    fn catalog_covers_every_kind_once() {
        let ids = quality_issue_kind_ids();
        let unique: HashSet<_> = ids.iter().copied().collect();
        assert_eq!(ids.len(), QualityIssueKind::ALL.len());
        assert_eq!(unique.len(), QualityIssueKind::ALL.len());
        for kind in QualityIssueKind::ALL {
            let spec = kind.spec();
            assert_eq!(spec.kind, *kind);
            assert_eq!(kind.as_str(), spec.kind.as_str());
            assert!(!spec.what.is_empty(), "{kind:?} missing what");
            assert!(!spec.falsifier.is_empty(), "{kind:?} missing falsifier");
            assert!(!spec.action.is_empty(), "{kind:?} missing action");
        }
    }

    #[test]
    fn hq_and_cloud_columns_stay_proposals() {
        assert_eq!(
            lane_surface_role("hq"),
            Some(ReportSurfaceRole::HumanTriggeredProposal)
        );
        assert_eq!(
            lane_surface_role("cloud"),
            Some(ReportSurfaceRole::HumanTriggeredProposal)
        );
        assert_eq!(
            lane_surface_role("delivered"),
            Some(ReportSurfaceRole::SessionDocument)
        );
        let report = classify_take_findings(&TakeQualityEvidence::default());
        assert_eq!(
            report.roles.candle,
            ReportSurfaceRole::HumanTriggeredProposal
        );
        assert_eq!(
            report.roles.cloud,
            ReportSurfaceRole::HumanTriggeredProposal
        );
        assert_eq!(report.roles.daily, ReportSurfaceRole::SessionDocument);
        assert!(report.wer_is_footnote);
        assert_eq!(report.schema, SUPERVISOR_FINDINGS_SCHEMA);
    }

    #[test]
    fn lying_judge_evidence_emits_hygiene_findings() {
        let report = classify_take_findings(&TakeQualityEvidence {
            daily_text: "podpinamy websocket na żywo".into(),
            hq_text: "podpinamy sok na żywo".into(),
            snapshot_live: true,
            audio_from_last_session: true,
            cloud_ran: true,
            vocabulary: None,
            treats_hq_as_document: true,
            leftover_websocket_polarity: true,
            ..TakeQualityEvidence::default()
        });
        let kinds: HashSet<_> = report.findings.iter().map(|row| row.kind).collect();
        for required in [
            QualityIssueKind::HqTreatedAsDocument,
            QualityIssueKind::WerPromotedToDocumentScore,
            QualityIssueKind::ProposalAgreementMisreadAsAccuracy,
            QualityIssueKind::LeftoverWebsocketPolarity,
            QualityIssueKind::LastSessionPairedWithLiveOverlay,
            QualityIssueKind::OmittedProgrammingVocabulary,
            QualityIssueKind::Disagreement,
        ] {
            assert!(
                kinds.contains(&required),
                "missing {required:?} in {kinds:?}"
            );
        }
        let disagreement = report
            .findings
            .iter()
            .find(|row| row.kind == QualityIssueKind::Disagreement)
            .expect("disagreement");
        assert!(
            disagreement.claim.contains("agreement, not accuracy"),
            "{}",
            disagreement.claim
        );
        assert_eq!(disagreement.target, FindingTarget::LexiconTune);
    }

    #[test]
    fn honest_take_does_not_promote_wer() {
        let report = classify_take_findings(&TakeQualityEvidence {
            daily_text: "podpinamy websocket na żywo".into(),
            hq_text: "podpinamy websocket na żywo".into(),
            cloud_text: "podpinamy websocket na żywo".into(),
            cloud_ran: true,
            vocabulary: Some(PROGRAMMING_VOCABULARY.into()),
            treats_hq_as_document: false,
            leftover_websocket_polarity: false,
            snapshot_live: false,
            audio_from_last_session: true,
            ..TakeQualityEvidence::default()
        });
        let kinds: HashSet<_> = report.findings.iter().map(|row| row.kind).collect();
        assert!(!kinds.contains(&QualityIssueKind::HqTreatedAsDocument));
        assert!(!kinds.contains(&QualityIssueKind::WerPromotedToDocumentScore));
        assert!(!kinds.contains(&QualityIssueKind::OmittedProgrammingVocabulary));
        assert!(!kinds.contains(&QualityIssueKind::LastSessionPairedWithLiveOverlay));
        assert!(!kinds.contains(&QualityIssueKind::Disagreement));
    }

    #[test]
    fn vocabulary_off_is_explicit_unbiased_bench() {
        let report = classify_take_findings(&TakeQualityEvidence {
            cloud_ran: true,
            vocabulary: Some(VOCABULARY_OFF.into()),
            daily_text: "ok".into(),
            hq_text: "ok".into(),
            ..TakeQualityEvidence::default()
        });
        assert!(
            report
                .findings
                .iter()
                .all(|row| row.kind != QualityIssueKind::OmittedProgrammingVocabulary)
        );
    }

    #[test]
    fn empty_daily_plus_thanks_for_watching_is_silence_hallucination() {
        let report = classify_take_findings(&TakeQualityEvidence {
            daily_text: String::new(),
            hq_text: "Thanks for watching".into(),
            ..TakeQualityEvidence::default()
        });
        assert!(
            report
                .findings
                .iter()
                .any(|row| row.kind == QualityIssueKind::HallucinateIntoSilence)
        );
        assert!(
            report
                .findings
                .iter()
                .any(|row| row.kind == QualityIssueKind::EmptyTranscript)
        );
    }

    #[test]
    fn clock_lie_helper_is_the_same_engine_function() {
        assert!(is_clock_lie(41, 0.10));
        let report = classify_take_findings(&TakeQualityEvidence {
            daily_text: "ten".into(),
            clock_lie_count: 1,
            ..TakeQualityEvidence::default()
        });
        assert!(
            report
                .findings
                .iter()
                .any(|row| row.kind == QualityIssueKind::ClockLie)
        );
    }

    #[test]
    fn confidence_flag_maps_to_typed_kind() {
        let report = classify_take_findings(&TakeQualityEvidence {
            daily_text: "ok".into(),
            confidence_flags: vec!["possible_hallucination_logprob".into()],
            ..TakeQualityEvidence::default()
        });
        let kinds: HashSet<_> = report.findings.iter().map(|row| row.kind).collect();
        assert!(kinds.contains(&QualityIssueKind::PossibleHallucinationLogprob));
    }

    #[test]
    fn contract_doc_names_the_supervisor_lock() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let body = std::fs::read_to_string(root.join(ENGINE_CONTRACT_DOC))
            .unwrap_or_else(|err| panic!("{} must exist: {err}", ENGINE_CONTRACT_DOC));
        for needle in [
            SUPERVISOR_FINDINGS_SCHEMA,
            "hq_treated_as_document",
            "omitted_programming_vocabulary",
            "last_session_paired_with_live_overlay",
        ] {
            assert!(
                body.contains(needle),
                "{ENGINE_CONTRACT_DOC} missing {needle:?}"
            );
        }
    }
}
