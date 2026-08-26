//! Production-owned replay seam for private overlay quality evaluation.
//!
//! Audio ingress is the only substituted boundary: decoded fixture PCM is fed
//! in 100 ms chunks instead of arriving from CoreAudio. Everything downstream
//! is shared with the overlay: recording-start Layer 1 policy, `SessionConfig`,
//! `transcription_session`, the session-owned `AcousticLedger`, and the
//! `TranscriptReducer` projection delivered to the overlay.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Result, anyhow};
use codescribe_core::audio::streaming_recorder::replay_production_session;
use codescribe_core::config::UserSettings;
use codescribe_core::pipeline::acoustic_ledger::AcousticLedger;
use codescribe_core::pipeline::contracts::{
    EngineEvent, FinalPassDisposition, TranscriptionVerdict,
};
use codescribe_core::pipeline::streaming::APPLE_FINAL_OVERLAP_WARNING_CODE;

use crate::presentation::emitter::TranscriptReducer;

/// Which production stop lane a corpus replay should exercise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductionReplayLane {
    /// Production ledger projection without an additional full-file observation.
    AppleLexicon,
    /// Production ledger projection plus a diagnostic local full-file observation.
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
    pub final_pass_attempted: bool,
    pub final_pass_skipped: bool,
    pub final_pass_skip_reason: Option<String>,
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
    final_pass_attempted: bool,
    final_pass_skipped: bool,
    final_pass_skip_reason: Option<String>,
}

/// Project authenticated ledger events through the reducer of record.
fn project_ledger_truth(events: &[EngineEvent], ledger: &mut AcousticLedger) -> Result<String> {
    let mut reducer = TranscriptReducer::default();
    let mut rendered = None;
    for event in events {
        let revision = match event {
            EngineEvent::LedgerMutation {
                observation,
                receipt,
                ..
            } => reducer.apply_ledger_mutation(ledger, observation, receipt),
            EngineEvent::LedgerSeal { receipt } => reducer.apply_ledger_seal(receipt),
            EngineEvent::OccurrenceLabelProposal { proposal } => {
                // `apply_occurrence_label_proposal` returns `(formatter_returned, revision)`.
                // Offline replay projection tracks rendered document text updates via `revision`;
                // `formatter_returned` (indicating an open Formatter slot was returned to permit sealing)
                // is intentionally not used for real-time sealing in replay projection.
                let (formatter_returned, revision) =
                    reducer.apply_occurrence_label_proposal(ledger, proposal);
                let _ = formatter_returned;
                revision
            }
            _ => None,
        };
        if let Some(revision) = revision {
            rendered = Some(revision.rendered_text);
        }
    }
    rendered
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| anyhow!("production ledger projection produced no deliverable text"))
}

/// Delivery is a projection of the ledger, never a second text adjudication.
/// An optional full-file pass remains diagnostic evidence for corpus reports;
/// it cannot replace, append, or post-process the projected occurrences.
fn finish_replay_delivery(
    live_text: String,
    local_final_pass_verdict: Option<TranscriptionVerdict>,
    streaming_engine_label: &str,
) -> Result<ReplayDelivery> {
    if live_text.trim().is_empty() {
        return Err(anyhow!(
            "production ledger projection produced no deliverable text"
        ));
    }
    let final_pass_attempted = local_final_pass_verdict.is_some();
    let (final_pass_skipped, final_pass_skip_reason) = local_final_pass_verdict
        .as_ref()
        .and_then(|verdict| verdict.final_pass.as_ref())
        .map(|pass| {
            (
                matches!(
                    pass.disposition,
                    FinalPassDisposition::Skipped
                        | FinalPassDisposition::Rejected
                        | FinalPassDisposition::Dropped
                ),
                pass.reason.clone(),
            )
        })
        .unwrap_or_else(|| {
            (
                !final_pass_attempted,
                (!final_pass_attempted).then(|| "not_attempted".to_string()),
            )
        });
    Ok(ReplayDelivery {
        adjudicated_text: live_text.clone(),
        delivered_text: live_text,
        transcript_source: Some("ledger_projection".to_string()),
        engine_label: Some(streaming_engine_label.to_string()),
        final_pass_attempted,
        final_pass_skipped,
        final_pass_skip_reason,
    })
}

/// Replay one WAV through the production overlay engine cone.
pub async fn replay_overlay_recording(
    wav: &Path,
    language: Option<String>,
    settings: &UserSettings,
    lane: ProductionReplayLane,
) -> Result<ProductionOverlayReplay> {
    let (samples, sample_rate) = codescribe_core::audio::load_audio_file(wav)
        .map_err(|_| anyhow!("load replay audio failed"))?;
    if samples.is_empty() {
        return Err(anyhow!("replay WAV contains no samples"));
    }

    let session =
        replay_production_session(&samples, sample_rate, language.clone(), settings).await?;
    let live_text = {
        let mut ledger = session
            .acoustic_ledger
            .lock()
            .map_err(|_| anyhow!("production ledger lock poisoned"))?;
        project_ledger_truth(&session.events, &mut ledger)?
    };

    let final_verdict = match lane {
        ProductionReplayLane::AppleLexicon => None,
        ProductionReplayLane::LocalFinalPass => Some(
            codescribe_core::stt::transcribe_file_verdict(wav, language.as_deref())
                .map_err(|_| anyhow!("production local final pass failed"))?,
        ),
    };
    let delivery = finish_replay_delivery(
        live_text.clone(),
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
        final_pass_attempted: delivery.final_pass_attempted,
        final_pass_skipped: delivery.final_pass_skipped,
        final_pass_skip_reason: delivery.final_pass_skip_reason,
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
            confidence_flags: Vec::new(),
        }
    }

    #[test]
    fn replay_delivery_preserves_the_ledger_projection_without_rewrite() {
        let delivery = finish_replay_delivery(
            "Uzywam doker do kontenerow.".to_string(),
            None,
            "live_apple",
        )
        .expect("ledger projection should remain deliverable");
        assert_eq!(
            delivery.transcript_source.as_deref(),
            Some("ledger_projection")
        );
        assert_eq!(delivery.delivered_text, "Uzywam doker do kontenerow.");
        assert_eq!(delivery.adjudicated_text, delivery.delivered_text);
        assert_eq!(delivery.engine_label.as_deref(), Some("live_apple"));
        assert!(!delivery.final_pass_attempted);
        assert!(delivery.final_pass_skipped);
        assert_eq!(
            delivery.final_pass_skip_reason.as_deref(),
            Some("not_attempted")
        );
    }

    /// The exact field consumed by the production replay JSON must name the
    /// live Apple canvas when Layer 1 and local final pass are both disarmed.
    #[test]
    fn apple_only_replay_json_surface_never_reports_streaming_whisper() {
        let delivery = finish_replay_delivery("pacjent stabilny".to_string(), None, "live_apple")
            .expect("Apple live floor should remain deliverable");

        let row = serde_json::json!({ "engine_label": delivery.engine_label });
        assert_eq!(row["engine_label"], "live_apple");
        assert_ne!(row["engine_label"], "streaming_whisper");
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
            ProductionReplayLane::AppleLexicon,
        )
        .await
        .expect_err("a missing replay input must fail");
        let rendered = format!("{error:#}");
        assert!(!rendered.contains(&basename));
        assert!(!rendered.contains(&path.display().to_string()));
    }
}
