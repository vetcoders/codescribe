//! Recording-truth adjudication for the stop path.
//!
//! Chooses the authoritative transcript (local final pass, stream floor, cloud)
//! and builds typed confidence flags + display status for sidecars / UI.

use tracing::{info, warn};

use codescribe_core::pipeline::contracts::{
    FinalPassDisposition, TranscriptionConfidenceFlag, TranscriptionVerdict,
};

use super::final_pass::engine_label_from_verdict;
use super::helpers::SessionTelemetrySnapshot;
use super::types::{RecordingFallbackClass, RecordingTranscriptSource};

/// Collapse a whitespace-only transcript to `None` — blank is not a transcript.
fn non_empty_transcript(text: Option<String>) -> Option<String> {
    text.filter(|text| !text.trim().is_empty())
}

/// The typed Layer 1 producer whose result is being reconciled with the live
/// Apple/stream floor. This is deliberately independent of transport vendor.
#[derive(Debug, Clone, Copy)]
enum Layer1AdjudicationSource {
    LocalWhisper,
    CloudPrimary,
    CloudFallback,
}

impl Layer1AdjudicationSource {
    const fn label(self) -> &'static str {
        match self {
            Self::LocalWhisper => "local_whisper",
            Self::CloudPrimary => "cloud_primary",
            Self::CloudFallback => "cloud_fallback",
        }
    }
}

const fn layer1_decision_reason(mode: codescribe_core::quality::Layer1MergeMode) -> &'static str {
    match mode {
        codescribe_core::quality::Layer1MergeMode::Empty => "empty",
        codescribe_core::quality::Layer1MergeMode::LiveOnly => "live_only",
        codescribe_core::quality::Layer1MergeMode::ProviderOnly => "provider_only",
        codescribe_core::quality::Layer1MergeMode::LiveFloorGapFill => "live_floor_gap_fill",
    }
}

const fn cloud_layer1_engine_label(
    mode: codescribe_core::quality::Layer1MergeMode,
) -> &'static str {
    match mode {
        codescribe_core::quality::Layer1MergeMode::ProviderOnly => "cloud_stt",
        _ => "merged_live_layer1:cloud_stt",
    }
}

/// Reconcile one Layer 1 result against the immutable live floor and emit only
/// content-free operational telemetry. Cloud sources never receive local
/// known-term evidence, so they can fill aligned gaps/tails but cannot win a
/// substitution over a committed live token.
fn merge_layer1_with_live_floor(
    live: Option<&str>,
    provider_text: &str,
    source: Layer1AdjudicationSource,
    known_terms: &[String],
) -> codescribe_core::quality::Layer1MergedDelivery {
    let live = live.unwrap_or_default();
    let merged = match source {
        Layer1AdjudicationSource::LocalWhisper => {
            codescribe_core::quality::merge_live_whisper_with_terms(
                live,
                provider_text,
                known_terms,
            )
            .into()
        }
        Layer1AdjudicationSource::CloudPrimary | Layer1AdjudicationSource::CloudFallback => {
            codescribe_core::quality::merge_live_layer1(live, provider_text)
        }
    };

    info!(
        source = source.label(),
        live_chars = live.chars().count(),
        provider_chars = provider_text.chars().count(),
        merged_chars = merged.text.chars().count(),
        decision_reason = layer1_decision_reason(merged.mode),
        equal = merged.equal_tokens,
        provider_fill = merged.provider_fill_tokens,
        live_subs = merged.live_kept_substitutes,
        provider_won_subs = merged.provider_won_substitutes,
        known_terms = known_terms.len(),
        "Layer 1 adjudication completed"
    );

    merged
}

/// The adjudicated outcome of one recording: what to deliver, and how much to
/// trust it.
///
/// Every field is provenance the sidecar (`truth.json`) and the UI read back, so
/// the app can be honest about which engine actually served and why.
#[derive(Debug, Clone, Default)]
pub(crate) struct RecordingTruthVerdict {
    /// The text to deliver, or `None` when nothing trustworthy survived.
    pub(crate) raw_text: Option<String>,
    /// Which lane produced [`Self::raw_text`].
    pub(crate) transcript_source: Option<RecordingTranscriptSource>,
    /// How degraded that lane is, when it is a fallback at all.
    pub(crate) fallback_class: Option<RecordingFallbackClass>,
    /// Why no speech was accepted; set means the verdict is deliberately empty.
    pub(crate) no_speech_reason: Option<String>,
    /// Share of the recording VAD classified as speech.
    pub(crate) speech_pct: Option<f32>,
    /// Engine average log-probability, the hallucination signal.
    pub(crate) avg_logprob: Option<f32>,
    /// Typed confidence flags (engine-owned + app-level provenance).
    /// Stored as the core enum rather than `Vec<String>` so downstream
    /// consumers do not need to re-parse tokens and new variants surface
    /// as compile errors instead of silent misses.
    pub(crate) confidence_flags: Vec<TranscriptionConfidenceFlag>,
    /// VAD speech sparkline preserved from the core `VadVerdict` so it
    /// can survive to `truth.json` on disk (previously dropped at the
    /// app boundary — tracked as Kłamstwo 7).
    pub(crate) sparkline: Option<String>,
    /// Disposition of the explicit file-level final pass, when one ran.
    /// None means no final pass was attempted for this verdict.
    pub(crate) final_pass_disposition: Option<FinalPassDisposition>,
    /// Actual serving engine label for sidecar/UI (`local_apple`, `local_whisper`, …).
    /// Preference-derived labels are forbidden when a verdict is present.
    pub(crate) engine_label: Option<String>,
    /// Why this recording is flagged for review, if it is.
    pub(crate) commit_trigger: Option<String>,
    /// Human-readable one-liner for the UI, derived from source and flags.
    pub(crate) display_status: String,
}

/// Append `flag` unless it is already present — the flag list is a set.
pub(crate) fn push_typed_flag(
    flags: &mut Vec<TranscriptionConfidenceFlag>,
    flag: TranscriptionConfidenceFlag,
) {
    if !flags.contains(&flag) {
        flags.push(flag);
    }
}

/// Record that AI formatting returned the input unchanged.
///
/// Skipped on the assistive lane, where an unchanged reply is a legitimate
/// answer rather than a no-op worth flagging.
pub(crate) fn apply_ai_noop_signal(
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

/// Why this recording should be surfaced for review, or `None` when it is clean.
///
/// No-speech wins outright; then the confidence flags in a fixed priority order;
/// then the fallback class. The ordering deliberately mirrors the original
/// string-based implementation so display behaviour survived the move to typed
/// flags unchanged.
pub(crate) fn truth_review_trigger(
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

/// One-line status for the UI, in the same precedence as the review trigger.
pub(crate) fn truth_display_status(
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

// allow(too_many_arguments): verdict aggregates independent recording-truth
// signals collected at one call site; a params struct would only restate the
// same names. Revisit if call sites multiply.
/// Assemble a [`RecordingTruthVerdict`], deriving the review trigger and the
/// display status from the signals passed in.
///
/// The single construction point for a verdict, so trigger and status can never
/// disagree with the flags they were computed from.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_truth_verdict(
    raw_text: Option<String>,
    transcript_source: Option<RecordingTranscriptSource>,
    fallback_class: Option<RecordingFallbackClass>,
    no_speech_reason: Option<String>,
    speech_pct: Option<f32>,
    avg_logprob: Option<f32>,
    confidence_flags: Vec<TranscriptionConfidenceFlag>,
    sparkline: Option<String>,
    final_pass_disposition: Option<FinalPassDisposition>,
    engine_label: Option<String>,
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
        engine_label,
        commit_trigger,
        display_status,
    }
}

/// Decide what a finished recording actually delivers, and label its provenance.
///
/// The live transcript is the floor of truth. A Layer 1 result is **merged**
/// into the live assembly (committed live kept, provider filling gaps/tail)
/// rather than replacing it — full-replace would delete correct live tokens
/// and is a doctrine violation. See [`codescribe_core::quality::merge_live_layer1`].
///
/// Resolution order:
/// 1. Local final pass — an explicit no-speech verdict is authoritative and
///    returns empty; otherwise the text merges with the live floor.
/// 2. A final pass that came back *shorter* than the live assembly is rejected
///    as a length regression, keeping the stream and flagging provenance.
/// 3. Session-level no-speech telemetry.
/// 4. Cloud verdict merged against the streaming floor, then the streaming
///    floor alone. Cloud fallback remains explicitly degraded.
/// 5. Nothing usable: an empty verdict carrying the reason.
pub(crate) fn adjudicate_recording_truth(
    use_local_stt: bool,
    local_final_pass_attempted: bool,
    local_final_pass_verdict: Option<TranscriptionVerdict>,
    streaming_text: String,
    cloud_verdict: Option<crate::client::CloudTranscriptionVerdict>,
    session_telemetry: &SessionTelemetrySnapshot,
) -> RecordingTruthVerdict {
    let streaming_text = non_empty_transcript(Some(streaming_text));
    let cloud_verdict = cloud_verdict.filter(|verdict| !verdict.text.trim().is_empty());
    // When file-final collapses vs live assembly (Apple SFSpeech short file
    // path), keep the stream as floor of truth and mark provenance.
    let mut final_pass_length_regression = false;

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
        let engine_label = Some(engine_label_from_verdict(
            &verdict.engine,
            final_pass_disposition,
        ));

        let raw_text = if no_speech_reason.is_some() {
            None
        } else {
            non_empty_transcript(Some(verdict.text))
        };

        // Explicit no-speech from final pass remains authoritative.
        if no_speech_reason.is_some() {
            return build_truth_verdict(
                None,
                Some(RecordingTranscriptSource::LocalFinalPass),
                fallback_class,
                no_speech_reason,
                speech_pct,
                avg_logprob,
                confidence_flags,
                sparkline,
                final_pass_disposition,
                engine_label,
            );
        }

        if let Some(final_text) = raw_text.as_ref() {
            let is_regression = streaming_text.as_ref().is_some_and(|stream| {
                codescribe_core::pipeline::contracts::final_pass_is_length_regression(
                    final_text, stream,
                )
            });
            if is_regression {
                final_pass_length_regression = true;
                warn!(
                    final_chars = final_text.chars().count(),
                    stream_chars = streaming_text
                        .as_ref()
                        .map(|s| s.chars().count())
                        .unwrap_or(0),
                    "final-pass length regression vs live assembly — keeping stream as floor of truth"
                );
                // Fall through to streaming / cloud branches.
            } else if let Some(stream) = streaming_text.as_ref() {
                // Product: never full-replace live with Whisper. Merge live floor
                // + Whisper gap-fill (85% Apple×Whisper thesis; Teacher AlignOp).
                // Known-terms catalog (Voice Lab canonical terms) lets Whisper win
                // a disagreement region on code-switched entities Apple garbled
                // ("delivered ≠ what I spoke", operator 2026-08-10). Load failure
                // degrades to the legacy live-floor behavior, never blocks the stop.
                let known_terms: Vec<String> =
                    codescribe_core::quality::overlay_quality::custom_lexicon_entries()
                        .map(|entries| {
                            let mut terms: Vec<String> =
                                entries.into_iter().map(|e| e.canonical).collect();
                            terms.sort();
                            terms.dedup();
                            terms
                        })
                        .unwrap_or_default();
                let merged = merge_layer1_with_live_floor(
                    Some(stream),
                    final_text,
                    Layer1AdjudicationSource::LocalWhisper,
                    &known_terms,
                );
                // Merged path still used a final pass; keep engine label from final
                // but text is composite live×whisper.
                let eng = engine_label
                    .clone()
                    .map(|e| format!("merged_live_whisper:{e}"))
                    .or_else(|| Some("merged_live_whisper".to_string()));
                return build_truth_verdict(
                    Some(merged.text),
                    Some(RecordingTranscriptSource::LocalFinalPass),
                    fallback_class,
                    None,
                    speech_pct,
                    avg_logprob,
                    confidence_flags,
                    sparkline,
                    final_pass_disposition,
                    eng,
                );
            } else {
                return build_truth_verdict(
                    Some(final_text.clone()),
                    Some(RecordingTranscriptSource::LocalFinalPass),
                    fallback_class,
                    None,
                    speech_pct,
                    avg_logprob,
                    confidence_flags,
                    sparkline,
                    final_pass_disposition,
                    engine_label,
                );
            }
        }
        // Empty final text without no_speech → fall through to stream floor.
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
            None,
        );
    }

    if use_local_stt {
        let mut confidence_flags: Vec<TranscriptionConfidenceFlag> = Vec::new();
        if final_pass_length_regression {
            push_typed_flag(
                &mut confidence_flags,
                TranscriptionConfidenceFlag::FinalPassLengthRegression,
            );
        } else if local_final_pass_attempted {
            push_typed_flag(
                &mut confidence_flags,
                TranscriptionConfidenceFlag::LocalFinalPassUnavailable,
            );
        }

        if let Some(cloud_verdict) = cloud_verdict {
            let merged = merge_layer1_with_live_floor(
                streaming_text.as_deref(),
                &cloud_verdict.text,
                Layer1AdjudicationSource::CloudFallback,
                &[],
            );
            let engine_label = cloud_layer1_engine_label(merged.mode);
            let mut fallback_flags = confidence_flags.clone();
            for flag in &cloud_verdict.confidence_flags {
                push_typed_flag(&mut fallback_flags, *flag);
            }
            push_typed_flag(
                &mut fallback_flags,
                TranscriptionConfidenceFlag::CloudFallbackUsed,
            );
            return build_truth_verdict(
                Some(merged.text),
                Some(RecordingTranscriptSource::CloudFallback),
                Some(RecordingFallbackClass::Degraded), // cloud fallback is no longer "Acceptable" (silent), it must be explicit
                None,
                None,
                None,
                fallback_flags,
                None,
                None,
                Some(engine_label.to_string()),
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
            // Regression keep-stream is still live assembly truth, not "degraded
            // because final missing" — label as streaming floor when we rejected
            // a collapsing final pass.
            let engine = if final_pass_length_regression {
                Some("streaming_live_floor".to_string())
            } else {
                Some("streaming_whisper".to_string())
            };
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
                engine,
            );
        }
    } else {
        if let Some(cloud_verdict) = cloud_verdict {
            let merged = merge_layer1_with_live_floor(
                streaming_text.as_deref(),
                &cloud_verdict.text,
                Layer1AdjudicationSource::CloudPrimary,
                &[],
            );
            let engine_label = cloud_layer1_engine_label(merged.mode);
            return build_truth_verdict(
                Some(merged.text),
                Some(RecordingTranscriptSource::CloudPrimary),
                None,
                None,
                None,
                None,
                cloud_verdict.confidence_flags,
                None,
                None,
                Some(engine_label.to_string()),
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
                Some("streaming_whisper".to_string()),
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
        None,
    )
}

/// Engine label for the sidecar and UI.
///
/// The engine reported by the verdict always wins; the per-source mapping is a
/// preference-derived guess and is used only when no actual label exists.
pub(crate) fn truth_engine_label(
    source: Option<RecordingTranscriptSource>,
    actual_engine_label: Option<&str>,
) -> Option<String> {
    if let Some(label) = actual_engine_label {
        return Some(label.to_string());
    }
    source.map(|source| match source {
        // Preference-derived labels only when no verdict engine is available.
        RecordingTranscriptSource::LocalFinalPass
        | RecordingTranscriptSource::ToggleSessionAdjudicated => "local_whisper".to_string(),
        RecordingTranscriptSource::CloudPrimary | RecordingTranscriptSource::CloudFallback => {
            "cloud_stt".to_string()
        }
        RecordingTranscriptSource::Streaming | RecordingTranscriptSource::StreamingFallback => {
            "streaming_whisper".to_string()
        }
    })
}
