//! Neutral Layer 1 ASR session contract.
//!
//! Layer 0 (the Apple live canvas) draws instantly and owns committed text.
//! Layer 1 is a *refiner*: it may fill gaps and patch tails, and it may never
//! rewrite what the canvas already committed. This module is the seam that
//! Layer 1 providers plug into — typed and vendor-neutral.
//!
//! ## What lives here
//!
//! - [`events`] — the typed event vocabulary: every event carries session,
//!   utterance and sequence identity, partial-vs-final is a variant (not a
//!   boolean), the audio span is optional and bounded, and errors/usage are
//!   typed with no free-form payload.
//! - [`ingest`] — the ordering state machine: monotonic sequencing, idempotent
//!   duplicate finals, and a sealed utterance that no later partial can reopen.
//! - [`provider`] — [`AsrSessionProvider`] plus the selection types that keep
//!   Layer 0 canvas choice and Layer 1 refiner mode on two separate axes.
//! - [`fake`] — a deterministic in-memory provider for tests and later cuts.
//! - [`cloud`] — the dedicated live gateway session, bounded PCM transport,
//!   Voice Lab wire adapter, and Codescribe-owned stream-global event
//!   sequencing.
//! - [`local_helper`] — the provider-compatible, injected child-process
//!   boundary whose confirmed exit is the local-weight reclaim authority.
//! - [`recorder`] — the per-recording lane the live session drives: injected
//!   [`recorder::Layer1Decision`], bounded non-blocking PCM fan-out, volatile
//!   partial draft, and typed degrade paths that always land on canvas +
//!   lexicon.
//! - [`consent`] — the audio-egress gate: a cloud session is constructible
//!   only through an explicit-consent authorization witness, and every
//!   refusal degrades to canvas + lexicon, never a local model.
//!
//! ## What deliberately does NOT live here
//!
//! No settings UI or local model. The Voice Lab wire is isolated behind the
//! normalized provider contract; the mode/consent *records* live in
//! `crate::config::cloud_asr` (the settings brain).
//!
//! The existing whole-file `client::transcribe_cloud` API is **outside** this
//! contract. It uploads one completed recording only for explicit retranscribe
//! surfaces — normal recording never routes that file pass through this seam.
//!
//! ## Doctrine encoded in the types
//!
//! - A refiner failure degrades to canvas + lexicon
//!   ([`LayerSelection::degraded`]); it can never swap the canvas engine, and
//!   nothing here can trigger a local model load.
//! - A final seals its utterance. Re-delivery of that same final is idempotent;
//!   anything else aimed at a sealed utterance is refused rather than applied.
//! - Errors carry a typed kind and nothing else, so no transcript fragment,
//!   audio, or credential can ride an error into a log line.

/// Dedicated provider-neutral live cloud gateway transport and session adapter.
pub mod cloud;
/// Audio-egress consent gate in front of Layer 1 session construction (C2).
pub mod consent;
/// Typed Layer 1 session events, identity, bounded ranges, errors, and usage.
pub mod events;
/// Deterministic in-memory provider used by tests and follow-on transport cuts.
pub mod fake;
/// Ordering state machine: monotonic sequencing and idempotent duplicate finals.
pub mod ingest;
/// Killable local-helper lifecycle and injected process boundary (L0).
pub mod local_helper;
/// Provider trait plus the canvas/refiner selection split.
pub mod provider;
/// Recorder-side Layer 1 lane: injected decision, bounded fan-out, degrade paths.
pub mod recorder;

#[cfg(test)]
mod tests;

pub use cloud::{
    CloudGatewayTransport, CloudSessionLimits, CloudSessionTelemetry, GatewayConnection,
    GatewayErrorCode, GatewayEvent, GatewayPcmFrame, GatewaySessionConfig, GatewayTransportPoll,
    GatewayWebSocketTransport, LiveCloudAsrSession,
};
pub use consent::{
    CloudEgressAuthorization, CloudSessionError, authorize_cloud_egress, refiner_for,
};
pub use events::{
    AsrErrorKind, AsrSessionEvent, AudioRange, ErrorEvent, SessionId, TranscriptEvent, UsageEvent,
};
pub use fake::FakeAsrSessionProvider;
pub use ingest::{IngestVerdict, SessionIngest};
pub use local_helper::{
    LocalHelperAsrSession, LocalHelperExit, LocalHelperLauncher, LocalHelperLifecycle,
    LocalHelperProcess,
};
pub use provider::{AsrSessionProvider, CanvasEngine, LayerSelection, RefinerMode, SessionInput};
pub use recorder::{
    FanOutVerdict, LAYER1_DEGRADED_WARNING_CODE, Layer1Decision, Layer1DegradeReason,
    Layer1LaneState, Layer1LaneTelemetry, Layer1SessionOutcome, LocalTailPatchDisposition,
    RecorderLayer1Lane, RecorderLifecycleEvent, RecorderLifecycleEvents, RecorderLifecycleHandle,
    apply_recorder_lifecycle_event, recorder_lifecycle_channel,
};

/// Content-free recording-start decision. Endpoints and credentials never
/// enter this receipt, even when provider construction is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layer1DecisionReceipt {
    pub asr_mode: &'static str,
    pub refiner: &'static str,
    pub reason: &'static str,
    pub consent: &'static str,
}

/// Resolve and log Layer 1 exactly once before the recording lane opens.
/// Both microphone capture and production replay consume this entrypoint.
pub fn layer1_decision(
    snapshot: &crate::config::RuntimeSettingsSnapshot,
) -> (Layer1Decision, Layer1DecisionReceipt) {
    let (decision, receipt) = layer1_decision_with_factory(snapshot, |snapshot, authorization| {
        let values = snapshot.values();
        let endpoint = values
            .stt_live_endpoint
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or("live_endpoint_missing")?;
        let key = values
            .stt_live_api_key
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or("live_key_missing")?;
        // This baseline speaks the Voice Lab live WebSocket directly. The
        // connection is constructed from the loader's live lane, never env or the
        // file-upload lane. Construction is dormant; open starts its worker.
        let connection =
            GatewayConnection::new(endpoint, key).map_err(|_| "live_connection_invalid")?;
        let limits = CloudSessionLimits {
            // Capture is native-rate, not necessarily 16 kHz: retain the
            // bounded 200 ms frame budget through 192 kHz capture/replay.
            max_frame_samples: 38_400,
            ..CloudSessionLimits::default()
        };
        let transport = GatewayWebSocketTransport::new(connection, limits)
            .map_err(|_| "cloud_transport_invalid")?;
        let provider = LiveCloudAsrSession::new(transport, limits, authorization)
            .map_err(|_| "cloud_provider_invalid")?;
        Ok(Box::new(provider))
    });
    tracing::info!(
        asr_mode = receipt.asr_mode,
        refiner = receipt.refiner,
        reason = receipt.reason,
        consent = receipt.consent,
        "layer1_decision"
    );
    (decision, receipt)
}

/// One policy body; tests replace only dormant cloud transport construction.
fn layer1_decision_with_factory(
    snapshot: &crate::config::RuntimeSettingsSnapshot,
    cloud_factory: impl FnOnce(
        &crate::config::RuntimeSettingsSnapshot,
        CloudEgressAuthorization,
    ) -> Result<Box<dyn AsrSessionProvider + Send>, &'static str>,
) -> (Layer1Decision, Layer1DecisionReceipt) {
    use crate::config::cloud_asr::{AudioEgressConsent, ModeDerivation};

    // resolved_asr_mode delegates to the single resolve_asr_product_mode law.
    let resolved = snapshot.user_settings().resolved_asr_mode();
    let mut receipt = Layer1DecisionReceipt {
        asr_mode: resolved.mode.as_str(),
        refiner: "off",
        reason: "layered_off",
        consent: match resolved.consent {
            AudioEgressConsent::Granted(_) => "granted",
            AudioEgressConsent::Denied => "denied",
            AudioEgressConsent::Unanswered => "missing",
        },
    };
    match refiner_for(&resolved) {
        RefinerMode::CloudSession => {
            // Keep the non-constructible witness at the actual factory seam.
            let provider = authorize_cloud_egress(&resolved.consent)
                .map_err(|_| "consent_required")
                .and_then(|authorization| cloud_factory(snapshot, authorization));
            match provider {
                Ok(provider) => {
                    receipt.refiner = "cloud_session";
                    receipt.reason = "cloud_ready";
                    (Layer1Decision::Armed(provider), receipt)
                }
                Err(reason) => {
                    receipt.reason = reason;
                    // A failed cloud selection never silently loads local weights.
                    (Layer1Decision::Disarmed, receipt)
                }
            }
        }
        RefinerMode::LocalHelper => {
            // Only Local Power consumes the diagnostic local-lane override.
            // Cloud admission and consent refusals have their own reasons.
            let local = snapshot.local_tail_patch_decision();
            if !local.is_armed() {
                receipt.reason = match local.local_tail_patch_disposition() {
                    Some(LocalTailPatchDisposition::DegradedInvalidOverride) => "layered_invalid",
                    _ => "layered_off",
                };
                return (Layer1Decision::Disarmed, receipt);
            }
            receipt.refiner = "local_tail_patch";
            receipt.reason = "local_tail_patch_armed";
            (local, receipt)
        }
        RefinerMode::Off => {
            receipt.reason = match resolved.derivation {
                ModeDerivation::ConsentMissingFallback => "consent_missing",
                ModeDerivation::ConsentDeniedFallback => "consent_denied",
                ModeDerivation::UnknownModeFallback => "asr_mode_invalid",
                _ => "apple_only",
            };
            (Layer1Decision::Disarmed, receipt)
        }
    }
}
