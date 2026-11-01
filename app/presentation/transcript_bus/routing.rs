//! Channel addressing of a session bus: the delivered channel message, the live
//! agent-channel cohort (`co_audience`), the joined channel's publication
//! evidence and the closing seal with its send hold.

use std::io;
use std::sync::Arc;

use chrono::{SecondsFormat, Utc};
use codescribe_core::pipeline::acoustic_ledger::AcousticLedger;
use serde::Serialize;

use super::envelope::*;
use super::{ProjectedSealCoverageReceipt, TranscriptBus};

/// A channel message as delivered: the finished text and the name of the
/// revision it is (`committed`, `light_plus` or `formatter`). `fallback` says
/// why a formatter revision was not delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChannelDelivery {
    pub text: String,
    pub revision: &'static str,
    pub fallback: Option<&'static str>,
}

/// One open agent channel as the controller's registry holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ChannelCohortMember {
    #[serde(skip)]
    pub digit: u8,
    pub channel: String,
    pub audience: String,
    /// Capture identity (`agent-channel-{digit}-{uuid}`), not the agent's.
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_session_id: Option<String>,
}

/// The controller's live agent-channel membership. The registry owner swaps
/// the inner `Arc` once per registry change while holding the registry lock,
/// and never holds this write lock while taking a bus writer lock. A bus
/// writer may take the read lock: that read is the linearization point.
pub(crate) type ChannelCohort = Arc<std::sync::RwLock<Arc<Vec<ChannelCohortMember>>>>;

/// Where a channel bus reads its `co_audience` from.
pub(super) enum CoAudienceSource {
    /// Shared bus, Fn+0, or a channel never admitted to the registry.
    Absent,
    /// Open: the cohort current at each emission, own digit excluded.
    Emission { cohort: ChannelCohort, own: u8 },
    /// Closing: the cohort frozen just before this channel left the registry,
    /// shared by every channel removed in the same batch.
    CaptureClose {
        cohort: Arc<Vec<ChannelCohortMember>>,
        own: u8,
    },
}

impl CoAudienceSource {
    /// The basis label and the other members, or `None` when there are none.
    pub(super) fn members(&self) -> Option<(&'static str, Vec<ChannelCohortMember>)> {
        let (basis, cohort, own) = match self {
            Self::Absent => return None,
            Self::Emission { cohort, own } => (
                "emission",
                Arc::clone(&cohort.read().unwrap_or_else(|error| error.into_inner())),
                *own,
            ),
            Self::CaptureClose { cohort, own } => ("capture_close", Arc::clone(cohort), *own),
        };
        let others: Vec<ChannelCohortMember> = cohort
            .iter()
            .filter(|member| member.digit != own)
            .cloned()
            .collect();
        (!others.is_empty()).then_some((basis, others))
    }
}

pub(super) struct ChannelPublicationEvidence {
    pub(super) coverage: ProjectedSealCoverageReceipt,
    pub(super) entirely_unwitnessed: bool,
}

impl TranscriptBus {
    /// Freeze the channel's acoustic observer only after its producer joined.
    /// Missing or foreign evidence stays unknown and never becomes silence.
    pub(crate) fn observe_channel_publication_evidence(&self, ledger: &AcousticLedger) {
        use codescribe_core::pipeline::acoustic_ledger::SlotWitness;

        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if self.session.mode != TranscriptMode::Agent
            || self.session.audience.is_none()
            || writer.channel_seal_published
            || writer.ended
        {
            return;
        }
        writer.channel_publication_evidence = None;
        let Some(coverage) = ledger.latest_seal_coverage() else {
            return;
        };
        if coverage.session_id != self.session.session_id
            || Some(coverage.capture_epoch) != writer.capture_epoch
            || coverage.status.unavailable_reason().is_some()
            || coverage.availability != "observed"
            || coverage.speech_producer != "silero_boundaries"
            || coverage.observed_samples.is_none_or(|samples| samples == 0)
            || coverage.sample_rate_hz.is_none_or(|rate| rate == 0)
        {
            return;
        }
        let mut has_slots = false;
        let mut entirely_unwitnessed = true;
        for occurrence in ledger.occurrences() {
            if occurrence.session != self.session.session_id
                || Some(occurrence.capture_epoch) != writer.capture_epoch
            {
                return;
            }
            let Some(slots) = ledger.slots_of(occurrence) else {
                return;
            };
            if slots.is_empty() {
                // A measured deletion leaves a known-empty owner. It is not
                // missing witness evidence for the remaining epoch text.
                if ledger
                    .text_of(occurrence)
                    .is_some_and(|text| text.trim().is_empty())
                {
                    continue;
                }
                return;
            }
            for slot in slots.iter().filter(|slot| !slot.text.trim().is_empty()) {
                has_slots = true;
                entirely_unwitnessed &= matches!(
                    &slot.witness,
                    SlotWitness::Unwitnessed | SlotWitness::Absent { .. }
                );
            }
        }
        // Visibility-only receipts are explicitly unwitnessed text too. They
        // must not bypass publication merely because they own no word slot.
        let has_unanchored_label = ledger.layer_trail().iter().any(|entry| {
            entry.observation.occurrence.session == self.session.session_id
                && Some(entry.observation.occurrence.capture_epoch) == writer.capture_epoch
                && matches!(&entry.decision,
                    codescribe_core::pipeline::acoustic_ledger::MutationReceipt::KeepVisibleUnanchored { label, .. }
                    if !label.trim().is_empty())
        });
        writer.channel_publication_evidence = Some(ChannelPublicationEvidence {
            coverage: ProjectedSealCoverageReceipt::from(coverage),
            entirely_unwitnessed: (has_slots || has_unanchored_label) && entirely_unwitnessed,
        });
    }

    /// Publish the joined channel's final snapshot before its non-open receipt.
    /// The producer has stopped, so no ledger seal can follow. A sealed
    /// document keeps its authority unchanged. A ledger-free document whose
    /// closing snapshot is the reducer's terminal direct revision closes
    /// marked `coverage: direct`: complete, but measured by no ledger. Any
    /// other document still closes, marked `coverage: refused`, so the send
    /// hold covers spoken words without claiming the ledger certified them.
    /// Hold metadata belongs only to these closing wire rows, never drafts.
    ///
    /// Maciej, 2026-10-17 09:59Z: the pause before sending covered typed
    /// messages only. On a day the ledger sealed no channel take, every spoken
    /// message left at once, with no Cancel/Edit.
    ///
    /// `delivery` is the channel's finished message (`agent_channel.segment_finish`):
    /// its text replaces the rendered document on both closing rows and its
    /// revision name is stamped as `delivered_revision`. `None` delivers the
    /// committed document unchanged.
    pub(crate) fn publish_channel_closed(
        &self,
        hold_sec: u32,
        delivery: Option<&ChannelDelivery>,
    ) -> io::Result<()> {
        if self.session.mode != TranscriptMode::Agent || self.session.audience.is_none() {
            return Ok(());
        }
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        // A live channel opens with its recipients receipt and never writes
        // `session_started`. Requiring `started` here meant no channel closing
        // decision was ever published (refusal, hold, refused coverage), and
        // the follower's lifecycle flush delivered the last draft instead:
        // take f32ff617, 2026-10-22, a 224 ms unwitnessed "Dziękuję.".
        let opened = writer.started || writer.channel_recipients.is_some();
        if !opened || writer.channel_seal_published {
            return Ok(());
        }
        // A small measured sound cannot publish an entirely unwitnessed label.
        // Compare sample counts without rounding the 400 ms boundary; missing
        // measurements and witnessed short words never enter this refusal.
        let refusal_coverage = writer
            .channel_publication_evidence
            .as_ref()
            .and_then(|evidence| {
                let sample_rate_hz = evidence.coverage.sample_rate_hz?;
                (evidence.entirely_unwitnessed
                    && u128::from(evidence.coverage.speech_samples) * 1_000
                        < u128::from(sample_rate_hz) * 400)
                    .then(|| evidence.coverage.clone())
            });
        if let Some(coverage) = refusal_coverage {
            // Consume the terminal boundary before persistence: an uncertain
            // append must not turn a refused capture into a retried delivery.
            writer.channel_seal_published = true;
            writer.ended = true;
            writer.end_reason = Some(TranscriptSessionEndReason::Completed);
            writer.sequence = writer.sequence.saturating_add(1);
            let receipt = serde_json::json!({
                "schema": "codescribe.transcript-publication.v1",
                "status": "channel_publication_refused",
                "session_id": self.session.session_id,
                "capture_epoch": writer.capture_epoch,
                "sequence": writer.sequence,
                "emitted_at": Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true),
                "reason": "sound_without_speech",
                "entirely_unwitnessed": true,
                "speech_floor_ms": 400,
                "seal_coverage": coverage,
            });
            Self::append_projection_locked(&mut writer, &receipt)?;
            let capture_epoch = writer.capture_epoch;
            return self.write_event_locked(
                &mut writer,
                CleanTranscriptEvent {
                    schema: "codescribe.transcript.v1".to_string(),
                    sequence: 0,
                    session_id: String::new(),
                    mode: self.session.mode,
                    utterance_id: None,
                    emitted_at: String::new(),
                    status: "session_ended".to_string(),
                    sample_rate_hz: coverage.sample_rate_hz,
                    capture_epoch,
                    sample_start: None,
                    sample_end: None,
                    audio_start_seconds: None,
                    audio_end_seconds: None,
                    text: String::new(),
                    phase: TranscriptProjectionPhase::NoSpeech,
                    can_paste: false,
                    can_insert: false,
                    can_copy: false,
                    can_retranscribe: false,
                    can_format: false,
                    can_send_to_agent: false,
                    terminal: true,
                    segments: Vec::new(),
                    words: Vec::new(),
                    coverage: None,
                    pipeline_session_id: None,
                    end_reason: Some(TranscriptSessionEndReason::Completed),
                    source: None,
                },
            );
        }
        let certified = writer.sealed;
        let Some(mut projection) = writer.last_projection.clone() else {
            if writer.direct_terminal.is_some() {
                writer.channel_seal_published = true;
            }
            return Ok(());
        };
        if projection.rendered_text.trim().is_empty() {
            if writer.direct_terminal.is_some() {
                writer.channel_seal_published = true;
            }
            return Ok(());
        }
        let direct = !certified
            && writer
                .direct_terminal
                .as_ref()
                .is_some_and(|terminal| terminal.revision == projection.reducer_revision);
        let now = Utc::now();
        projection.sequence = writer.sequence.saturating_add(1);
        projection.emitted_at = now.to_rfc3339_opts(SecondsFormat::Micros, true);
        projection.terminal = true;
        if let Some(delivery) = delivery.filter(|delivery| !delivery.text.trim().is_empty()) {
            projection.rendered_text = delivery.text.clone();
        }
        let mut evidence = serde_json::to_value(&projection).map_err(io::Error::other)?;
        evidence["status"] = serde_json::json!("transcript_sealed");
        if let Some(delivery) = delivery {
            evidence["delivered_revision"] = serde_json::json!(delivery.revision);
            if let Some(reason) = delivery.fallback {
                evidence["delivered_revision_fallback"] = serde_json::json!(reason);
            }
        }
        // The one row that authorizes delivery under `publication: controller`.
        evidence["closing_decision"] = serde_json::json!("publish");
        if !certified {
            evidence["coverage"] = serde_json::json!(if direct { "direct" } else { "refused" });
        }
        if hold_sec > 0 {
            evidence["hold_until"] = serde_json::json!(
                (now + chrono::TimeDelta::seconds(i64::from(hold_sec)))
                    .to_rfc3339_opts(SecondsFormat::Micros, true)
            );
        }
        let mut clean = evidence.clone();
        clean["schema"] = serde_json::json!("codescribe.transcript.v1");
        clean["sequence"] = serde_json::json!(projection.sequence.saturating_add(1));
        clean["text"] = serde_json::json!(&projection.rendered_text);
        // Mark before append: uncertain persistence never authorizes a retry.
        writer.channel_seal_published = true;
        writer.sequence = projection.sequence;
        writer.last_projection = Some(projection);
        Self::append_projection_locked(&mut writer, &evidence)?;
        writer.sequence = writer.sequence.saturating_add(1);
        Self::append_projection_locked(&mut writer, &clean)
    }

    /// The committed text the channel's closing seal would deliver, or `None`
    /// when this session has nothing left to publish.
    pub(crate) fn channel_closing_text(&self) -> Option<String> {
        let writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if writer.channel_seal_published {
            return None;
        }
        writer
            .last_projection
            .as_ref()
            .map(|projection| projection.rendered_text.clone())
            .filter(|text| !text.trim().is_empty())
    }

    /// Channel boundaries share the session's destinations and failure state.
    pub(crate) fn record_channel_receipt(&self, receipt: &serde_json::Value) {
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if receipt["schema"] == "codescribe.channel-recipients.v1"
            && receipt["session_id"] == self.session.session_id
            && receipt["recipients"].is_array()
            && writer.channel_recipients.is_none()
        {
            writer.channel_recipients = Some(receipt.clone());
        }
        if let Err(error) = Self::append_projection_locked(&mut writer, receipt) {
            self.log_write_error(error);
        }
    }

    /// Read the controller's live cohort at every later emission. Called once
    /// the channel is admitted, and again when a failed close keeps it open.
    pub(crate) fn attach_co_audience(&self, cohort: ChannelCohort, own: u8) {
        self.writer
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .co_audience = CoAudienceSource::Emission { cohort, own };
    }

    /// Stop following the live cohort: later rows of this closing session
    /// carry `cohort`, taken before this channel left the registry. A bus
    /// that never followed the cohort stays without co-audience.
    pub(crate) fn freeze_co_audience(&self, cohort: Arc<Vec<ChannelCohortMember>>) {
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let own = match &writer.co_audience {
            CoAudienceSource::Absent => return,
            CoAudienceSource::Emission { own, .. } | CoAudienceSource::CaptureClose { own, .. } => {
                *own
            }
        };
        writer.co_audience = CoAudienceSource::CaptureClose { cohort, own };
    }
}
