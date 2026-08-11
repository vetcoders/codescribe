//! Production-owned replay seam for private overlay quality evaluation.
//!
//! Audio ingress is the only substituted boundary: decoded fixture PCM is fed
//! in 100 ms chunks instead of arriving from CoreAudio. Everything downstream
//! is shared with the overlay: recording-start Layer 1 policy, `SessionConfig`,
//! `transcription_session`, stop truth adjudication, and the unconditional
//! lexicon/text layer immediately before delivery.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Result, anyhow};
use codescribe_core::asr_session::GatewaySessionAvailability;
use codescribe_core::audio::streaming_recorder::replay_production_session;
use codescribe_core::config::UserSettings;
use codescribe_core::pipeline::contracts::EngineEvent;
use codescribe_core::pipeline::contracts::TranscriptionVerdict;
use codescribe_core::pipeline::stream_postprocess::StreamPostProcessStats;
use codescribe_core::pipeline::streaming::APPLE_FINAL_OVERLAP_WARNING_CODE;

use super::helpers::SessionTelemetrySnapshot;
use super::truth::{adjudicate_recording_truth, postprocess_transcript_for_delivery};
use crate::presentation::emitter::reduce_transcript_events;

/// Which production stop lane a corpus replay should exercise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductionReplayLane {
    /// Shipped no-final-pass degradation: live canvas followed by lexicon.
    AppleLexicon,
    /// Explicit production local full-file pass, adjudicated against live.
    LocalFinalPass,
}

impl ProductionReplayLane {
    /// Stable content-free token for reports and filenames.
    pub const fn as_token(self) -> &'static str {
        match self {
            Self::AppleLexicon => "apple_lexicon",
            Self::LocalFinalPass => "local_final_pass",
        }
    }
}

/// In-memory result used to calculate content-redacting quality metrics.
///
/// Transcript bodies intentionally have no serialization implementation. The
/// corpus runner must reduce them to counts/scores before writing artifacts.
#[derive(Debug)]
pub struct ProductionOverlayReplay {
    pub lane: ProductionReplayLane,
    pub events: Vec<EngineEvent>,
    pub live_text: String,
    pub adjudicated_text: String,
    pub delivered_text: String,
    pub layer1_armed: bool,
    pub transcript_source: Option<String>,
    pub engine_label: Option<String>,
    pub postprocess_stats: StreamPostProcessStats,
    pub boundary_evidence: ReplayBoundaryEvidence,
}

/// Content-free final-boundary evidence emitted for every replay recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayBoundaryEvidence {
    pub final_count: usize,
    pub unique_final_id_count: usize,
    pub repeated_final_id_count: usize,
    pub overlapping_final_window_count: usize,
}

fn boundary_evidence(events: &[EngineEvent]) -> ReplayBoundaryEvidence {
    let mut ids = HashSet::new();
    let mut windows = Vec::<(f32, f32)>::new();
    let mut final_count = 0usize;
    let mut repeated_final_id_count = 0usize;
    let mut overlapping_final_window_count = 0usize;

    for event in events {
        if matches!(event, EngineEvent::Warning { code, .. } if code == APPLE_FINAL_OVERLAP_WARNING_CODE)
        {
            overlapping_final_window_count += 1;
            continue;
        }
        let EngineEvent::UtteranceFinal {
            utterance_id,
            start_ts,
            end_ts,
            ..
        } = event
        else {
            continue;
        };
        final_count += 1;
        if !ids.insert(*utterance_id) {
            repeated_final_id_count += 1;
        }
        if start_ts.is_finite() && end_ts.is_finite() && end_ts > start_ts {
            if windows
                .iter()
                .any(|(prior_start, prior_end)| start_ts < prior_end && end_ts > prior_start)
            {
                overlapping_final_window_count += 1;
            }
            windows.push((*start_ts, *end_ts));
        }
    }

    ReplayBoundaryEvidence {
        final_count,
        unique_final_id_count: ids.len(),
        repeated_final_id_count,
        overlapping_final_window_count,
    }
}

struct ReplayDelivery {
    adjudicated_text: String,
    delivered_text: String,
    transcript_source: Option<String>,
    engine_label: Option<String>,
    postprocess_stats: StreamPostProcessStats,
}

/// Shared replay stop boundary: production adjudication immediately followed
/// by the production delivery postprocessor. Keeping these calls together
/// makes a bypass detectable by one deterministic regression witness.
fn finish_replay_delivery(
    live_text: String,
    local_final_pass_attempted: bool,
    local_final_pass_verdict: Option<TranscriptionVerdict>,
    streaming_engine_label: &str,
) -> Result<ReplayDelivery> {
    let verdict = adjudicate_recording_truth(
        true,
        local_final_pass_attempted,
        local_final_pass_verdict,
        live_text,
        None,
        Some(streaming_engine_label),
        &SessionTelemetrySnapshot::default(),
    );
    let adjudicated_text = verdict
        .raw_text
        .clone()
        .ok_or_else(|| anyhow!("production adjudication produced no deliverable text"))?;
    let postprocessed = postprocess_transcript_for_delivery(&adjudicated_text);
    Ok(ReplayDelivery {
        adjudicated_text,
        delivered_text: postprocessed.text,
        transcript_source: verdict
            .transcript_source
            .map(|source| source.label().to_string()),
        engine_label: verdict.engine_label,
        postprocess_stats: postprocessed.stats,
    })
}

/// Replay one WAV through the production overlay engine cone.
pub async fn replay_overlay_recording(
    wav: &Path,
    language: Option<String>,
    settings: &UserSettings,
    gateway: GatewaySessionAvailability,
    lane: ProductionReplayLane,
) -> Result<ProductionOverlayReplay> {
    let (samples, sample_rate) = codescribe_core::audio::load_audio_file(wav)
        .map_err(|_| anyhow!("load replay audio failed"))?;
    if samples.is_empty() {
        return Err(anyhow!("replay WAV contains no samples"));
    }

    let session =
        replay_production_session(&samples, sample_rate, language.clone(), settings, gateway)
            .await?;
    let reducer = reduce_transcript_events(&session.events);
    let full = reducer.rendered_text();
    let floor = reducer.streaming_floor();
    let live_text = if floor.trim().is_empty() { full } else { floor };

    let (attempted, final_verdict) = match lane {
        ProductionReplayLane::AppleLexicon => (false, None),
        ProductionReplayLane::LocalFinalPass => (
            true,
            Some(
                codescribe_core::stt::transcribe_file_verdict(wav, language.as_deref())
                    .map_err(|_| anyhow!("production local final pass failed"))?,
            ),
        ),
    };
    let delivery = finish_replay_delivery(
        live_text.clone(),
        attempted,
        final_verdict,
        &session.streaming_engine_label,
    )?;

    let boundary_evidence = boundary_evidence(&session.events);
    Ok(ProductionOverlayReplay {
        lane,
        events: session.events,
        live_text,
        adjudicated_text: delivery.adjudicated_text,
        delivered_text: delivery.delivered_text,
        layer1_armed: session.layer1_armed,
        transcript_source: delivery.transcript_source,
        engine_label: delivery.engine_label,
        postprocess_stats: delivery.postprocess_stats,
        boundary_evidence,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use codescribe_core::pipeline::contracts::TranscriptSegment;

    fn final_event(id: u64, text: &str, start_ts: f32, end_ts: f32) -> EngineEvent {
        EngineEvent::UtteranceFinal {
            utterance_id: id,
            text: text.to_string(),
            raw_text: text.to_string(),
            start_ts,
            end_ts,
            segments: vec![TranscriptSegment {
                text: text.to_string(),
                start_ts,
                end_ts,
            }],
            vad_speech_pct: None,
            avg_logprob: None,
            compression_ratio: None,
            quality_gate_dropped: false,
            confidence_flags: Vec::new(),
        }
    }

    #[test]
    fn replay_stop_boundary_cannot_bypass_adjudication_or_lexicon() {
        let delivery = finish_replay_delivery(
            "Uzywam doker do kontenerow.".to_string(),
            false,
            None,
            "live_apple",
        )
        .expect("live floor should remain deliverable");
        assert_eq!(
            delivery.transcript_source.as_deref(),
            Some("Streaming fallback"),
            "the replay must cross production truth adjudication"
        );
        assert!(
            delivery.delivered_text.contains("Docker"),
            "the replay must cross the unconditional production lexicon layer"
        );
        assert!(delivery.postprocess_stats.lexicon_rewrites >= 1);
        assert_eq!(delivery.engine_label.as_deref(), Some("live_apple"));
    }

    /// The exact field consumed by the production replay JSON must name the
    /// live Apple canvas when Layer 1 and local final pass are both disarmed.
    #[test]
    fn apple_only_replay_json_surface_never_reports_streaming_whisper() {
        let delivery =
            finish_replay_delivery("pacjent stabilny".to_string(), false, None, "live_apple")
                .expect("Apple live floor should remain deliverable");

        let row = serde_json::json!({ "engine_label": delivery.engine_label });
        assert_eq!(row["engine_label"], "live_apple");
        assert_ne!(row["engine_label"], "streaming_whisper");
    }

    #[test]
    fn production_reducer_vectors_preserve_one_slot_and_legitimate_repetition() {
        let cumulative = vec![
            EngineEvent::Preview {
                rev: 1,
                text: "alpha".into(),
            },
            EngineEvent::Preview {
                rev: 2,
                text: "alpha beta".into(),
            },
            final_event(1, "alpha beta", 0.0, 1.0),
        ];
        let reduced = reduce_transcript_events(&cumulative);
        assert_eq!(reduced.streaming_floor(), "alpha beta");
        assert_eq!(reduced.committed_count(), 1);

        let revised = vec![
            final_event(7, "draft final", 0.0, 1.0),
            final_event(7, "revised final", 0.0, 1.0),
        ];
        let reduced = reduce_transcript_events(&revised);
        assert_eq!(reduced.streaming_floor(), "revised final");
        assert_eq!(reduced.committed_count(), 1);

        let repeated = vec![
            final_event(1, "tak tak", 0.0, 1.0),
            final_event(2, "tak tak", 1.0, 2.0),
        ];
        let reduced = reduce_transcript_events(&repeated);
        assert_eq!(reduced.streaming_floor(), "tak tak tak tak");
        assert_eq!(reduced.committed_count(), 2);
    }

    #[test]
    fn boundary_evidence_is_content_free_and_counts_id_and_window_conflicts() {
        let events = vec![
            final_event(1, "one", 0.0, 1.0),
            final_event(1, "revision", 0.0, 1.0),
            final_event(2, "two", 0.5, 2.0),
            final_event(3, "three", 2.0, 3.0),
        ];
        assert_eq!(
            boundary_evidence(&events),
            ReplayBoundaryEvidence {
                final_count: 4,
                unique_final_id_count: 3,
                repeated_final_id_count: 1,
                overlapping_final_window_count: 2,
            }
        );
    }

    #[tokio::test]
    async fn replay_load_failure_never_discloses_private_path() {
        let basename = format!("private-corpus-{}-must-not-leak.wav", std::process::id());
        let path = std::env::temp_dir().join(&basename);
        let error = replay_overlay_recording(
            &path,
            Some("pl".to_string()),
            &UserSettings::default(),
            GatewaySessionAvailability::Unavailable,
            ProductionReplayLane::AppleLexicon,
        )
        .await
        .expect_err("a missing replay input must fail");
        let rendered = format!("{error:#}");
        assert!(!rendered.contains(&basename));
        assert!(!rendered.contains(&path.display().to_string()));
    }
}
