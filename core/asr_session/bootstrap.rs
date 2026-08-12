//! Recording-start policy that joins persisted mode truth, consent, and a
//! minted gateway connection into the provider decision consumed by C1.
//!
//! The gateway mint itself is deliberately outside this repository boundary.
//! Until a caller supplies one validated, short-lived connection, cloud mode
//! is treated as unavailable and recording continues with Apple + lexicon.

use std::fmt;

use tracing::warn;

use super::cloud::{
    CloudSessionLimits, GatewayConnection, GatewayWebSocketTransport, LiveCloudAsrSession,
};
use super::consent::authorize_cloud_egress;
use super::recorder::Layer1Decision;
use crate::config::{AsrProductMode, UserSettings};

/// Availability of one short-lived gateway session at recording start.
///
/// `Invalid` is distinct from `Unavailable` for content-free diagnostics. The
/// raw endpoint and bearer never cross this enum and are never formatted.
pub enum GatewaySessionAvailability {
    /// No mint response is available (offline, timeout, or gateway absent).
    Unavailable,
    /// A mint response or normalized connection failed validation.
    Invalid,
    /// A validated, single-use WebSocket endpoint and bearer.
    Ready(GatewayConnection),
}

impl fmt::Debug for GatewaySessionAvailability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Unavailable => "GatewaySessionAvailability::Unavailable",
            Self::Invalid => "GatewaySessionAvailability::Invalid",
            Self::Ready(_) => "GatewaySessionAvailability::Ready([REDACTED])",
        })
    }
}

/// Build the one Layer 1 decision consumed by the real recorder path.
///
/// Cloud can arm only when the settings resolver still says `cloud`, explicit
/// audio-egress authorization succeeds, and the caller supplies a validated
/// minted gateway connection. Every other state is normal Apple + lexicon.
/// In particular, an unavailable cloud session never falls through to local
/// power or Whisper.
pub fn layer1_decision_for_recording(
    settings: &UserSettings,
    gateway: GatewaySessionAvailability,
) -> Layer1Decision {
    let resolved = settings.resolved_asr_mode();
    if resolved.mode != AsrProductMode::Cloud {
        return Layer1Decision::Disarmed;
    }

    let Ok(authorization) = authorize_cloud_egress(&resolved.consent) else {
        return Layer1Decision::Disarmed;
    };
    let GatewaySessionAvailability::Ready(connection) = gateway else {
        warn!(
            derivation = ?resolved.derivation,
            gateway = ?gateway,
            "Cloud Layer 1 unavailable at recording start; continuing with Apple + lexicon"
        );
        return Layer1Decision::Disarmed;
    };

    let limits = CloudSessionLimits::default();
    let Ok(transport) = GatewayWebSocketTransport::new(connection, limits) else {
        return Layer1Decision::Disarmed;
    };
    let Ok(session) = LiveCloudAsrSession::new(transport, limits, authorization) else {
        return Layer1Decision::Disarmed;
    };
    Layer1Decision::Armed(Box::new(session))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cloud_settings(consent: Option<&str>) -> UserSettings {
        UserSettings {
            asr_mode: Some("cloud".to_string()),
            cloud_consent: consent.map(str::to_string),
            ..UserSettings::default()
        }
    }

    fn ready() -> GatewaySessionAvailability {
        GatewaySessionAvailability::Ready(
            GatewayConnection::new("wss://gateway.invalid/v1/stt/live", "short-lived-token")
                .expect("valid normalized gateway connection"),
        )
    }

    #[test]
    fn consented_cloud_with_valid_mint_arms_the_real_provider() {
        let decision = layer1_decision_for_recording(&cloud_settings(Some("granted")), ready());
        assert!(decision.is_armed());
    }

    #[test]
    fn missing_denied_malformed_or_offline_cloud_stays_apple_only() {
        let cases = [
            (cloud_settings(None), ready()),
            (cloud_settings(Some("denied")), ready()),
            (
                cloud_settings(Some("granted")),
                GatewaySessionAvailability::Invalid,
            ),
            (
                cloud_settings(Some("granted")),
                GatewaySessionAvailability::Unavailable,
            ),
        ];

        for (settings, gateway) in cases {
            assert!(!layer1_decision_for_recording(&settings, gateway).is_armed());
        }
    }

    #[test]
    fn cloud_failure_and_explicit_local_mode_never_load_an_in_process_model() {
        let probe = || {
            crate::stt::whisper::singleton::test_init_calls()
                + crate::stt::whisper::singleton::test_load_calls()
        };
        let before = probe();

        let offline = layer1_decision_for_recording(
            &cloud_settings(Some("granted")),
            GatewaySessionAvailability::Unavailable,
        );
        let local_power = layer1_decision_for_recording(
            &UserSettings {
                asr_mode: Some("local_power".to_string()),
                ..UserSettings::default()
            },
            ready(),
        );

        assert!(!offline.is_armed());
        assert!(
            !local_power.is_armed(),
            "L0 owns the explicit helper provider"
        );
        assert_eq!(probe().saturating_sub(before), 0);
    }
}
