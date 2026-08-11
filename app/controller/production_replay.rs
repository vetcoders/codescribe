//! Production-owned replay seam for private overlay quality evaluation.
//!
//! Audio ingress is the only substituted boundary: decoded fixture PCM is fed
//! in 100 ms chunks instead of arriving from CoreAudio. Everything downstream
//! is shared with the overlay: recording-start Layer 1 policy, `SessionConfig`,
//! `transcription_session`, stop truth adjudication, and the unconditional
//! lexicon/text layer immediately before delivery.

use std::path::Path;

use anyhow::{Context, Result, anyhow};
use codescribe_core::asr_session::GatewaySessionAvailability;
use codescribe_core::audio::streaming_recorder::replay_production_session;
use codescribe_core::config::UserSettings;
use codescribe_core::pipeline::contracts::EngineEvent;
use codescribe_core::pipeline::contracts::TranscriptionVerdict;
use codescribe_core::pipeline::stream_postprocess::StreamPostProcessStats;
use codescribe_core::pipeline::streaming::assemble_live_from_events;

use super::helpers::SessionTelemetrySnapshot;
use super::truth::{adjudicate_recording_truth, postprocess_transcript_for_delivery};

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
) -> Result<ReplayDelivery> {
    let verdict = adjudicate_recording_truth(
        true,
        local_final_pass_attempted,
        local_final_pass_verdict,
        live_text,
        None,
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
        .with_context(|| format!("load replay WAV {}", wav.display()))?;
    if samples.is_empty() {
        return Err(anyhow!("replay WAV contains no samples"));
    }

    let session =
        replay_production_session(&samples, sample_rate, language.clone(), settings, gateway)
            .await?;
    let assembly = assemble_live_from_events(&session.events);
    let full = assembly.full_text();
    let floor = assembly.streaming_floor();
    let live_text = if floor.trim().is_empty() { full } else { floor };

    let (attempted, final_verdict) = match lane {
        ProductionReplayLane::AppleLexicon => (false, None),
        ProductionReplayLane::LocalFinalPass => (
            true,
            Some(
                codescribe_core::stt::transcribe_file_verdict(wav, language.as_deref())
                    .with_context(|| format!("production local final pass {}", wav.display()))?,
            ),
        ),
    };
    let delivery = finish_replay_delivery(live_text.clone(), attempted, final_verdict)?;

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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_stop_boundary_cannot_bypass_adjudication_or_lexicon() {
        let delivery =
            finish_replay_delivery("Uzywam doker do kontenerow.".to_string(), false, None)
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
    }
}
