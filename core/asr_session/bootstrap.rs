//! Recording-start policy that joins persisted mode truth, consent, and the
//! canonical STT endpoint into the provider decision consumed by C1.
//!
//! Known Libraxis and loopback multipart endpoints map to their live Voice Lab
//! socket. Unknown HTTP providers remain unavailable and recording continues
//! with Apple + lexicon rather than pretending a file endpoint is streaming.

use std::fmt;

use tracing::warn;

use super::cloud::{
    CloudSessionLimits, GatewayConnection, GatewayWebSocketTransport, LiveCloudAsrSession,
};
use super::consent::authorize_cloud_egress;
use super::recorder::Layer1Decision;
use crate::config::{AsrProductMode, Config, UserSettings};

/// Availability of one validated live session at recording start.
///
/// `Invalid` is distinct from `Unavailable` for content-free diagnostics. The
/// raw endpoint and credential never cross this enum and are never formatted.
pub enum GatewaySessionAvailability {
    /// No known live endpoint is available.
    Unavailable,
    /// The resolved live connection failed validation.
    Invalid,
    /// A validated WebSocket endpoint and endpoint-owned credential.
    Ready(GatewayConnection),
}

/// Resolve the live WebSocket session directly from canonical STT config.
///
/// Libraxis' multipart file endpoint and its proven Voice Lab socket are two
/// transports owned by the same provider. Normal recording uses the latter;
/// explicit retranscribe keeps the former. A direct `ws(s)` endpoint is also
/// accepted, while OpenAI multipart is not mislabeled as Voice Lab streaming.
pub fn gateway_session_availability(config: &Config) -> GatewaySessionAvailability {
    let Some(endpoint) = config
        .stt_endpoint
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    else {
        return GatewaySessionAvailability::Unavailable;
    };
    let Some(endpoint) = live_websocket_endpoint(endpoint) else {
        return GatewaySessionAvailability::Unavailable;
    };
    let credential = config.stt_api_key.as_deref().unwrap_or_default();
    match GatewayConnection::new(endpoint, credential) {
        Ok(connection) => GatewaySessionAvailability::Ready(connection),
        Err(_) => GatewaySessionAvailability::Invalid,
    }
}

fn live_websocket_endpoint(endpoint: &str) -> Option<String> {
    let mut url = reqwest::Url::parse(endpoint).ok()?;
    let host = url.host_str()?.trim_matches(['[', ']']);
    match url.scheme() {
        "ws" | "wss" => return Some(url.to_string()),
        "http" | "https" => {}
        _ => return None,
    }

    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if host.eq_ignore_ascii_case("api.openai.com") {
        return None;
    }
    if host.eq_ignore_ascii_case("api.libraxis.cloud") {
        url.set_scheme("wss").ok()?;
        url.set_path("/v1/audio/transcribe");
        url.set_query(None);
        url.set_fragment(None);
        return Some(url.to_string());
    }
    if !loopback {
        return None;
    }

    let websocket_scheme = if url.scheme() == "https" { "wss" } else { "ws" };
    url.set_scheme(websocket_scheme).ok()?;
    if url.path().ends_with("/transcriptions") {
        let path = url.path().trim_end_matches("transcriptions").to_string() + "transcribe";
        url.set_path(&path);
    }
    Some(url.to_string())
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
/// live gateway connection. Every other state is normal Apple + lexicon.
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

    #[test]
    fn multipart_endpoint_maps_only_to_a_known_live_socket() {
        assert_eq!(
            live_websocket_endpoint("https://api.libraxis.cloud/v1/audio/transcriptions")
                .as_deref(),
            Some("wss://api.libraxis.cloud/v1/audio/transcribe")
        );
        assert_eq!(
            live_websocket_endpoint("http://127.0.0.1:8000/v1/audio/transcriptions").as_deref(),
            Some("ws://127.0.0.1:8000/v1/audio/transcribe")
        );
        assert_eq!(
            live_websocket_endpoint("https://api.openai.com/v1/audio/transcriptions"),
            None
        );
        assert_eq!(
            live_websocket_endpoint("https://custom.example/v1/audio/transcriptions"),
            None
        );
    }

    #[test]
    fn loopback_live_socket_needs_no_credential() {
        let config = Config {
            stt_endpoint: Some("http://localhost:8000/v1/audio/transcriptions".into()),
            stt_api_key: None,
            ..Config::default()
        };
        assert!(matches!(
            gateway_session_availability(&config),
            GatewaySessionAvailability::Ready(_)
        ));
    }
}
