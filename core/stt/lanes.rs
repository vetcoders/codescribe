//! Atomic STT transport rows; consumers never infer one lane from another.
use super::tail_provider::{SttAuthMode, stt_auth_mode};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SttLane {
    File,
    Live,
}
impl SttLane {
    pub const ALL: [Self; 2] = [Self::File, Self::Live];
    pub fn id(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Live => "live",
        }
    }
    pub fn wire_key(self) -> &'static str {
        match self {
            Self::File => "STT_FILE_ENDPOINT",
            Self::Live => "STT_LIVE_ENDPOINT",
        }
    }
    pub fn key_account(self) -> &'static str {
        match self {
            Self::File => "STT_FILE_API_KEY",
            Self::Live => "STT_LIVE_API_KEY",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Self::File => "File transcription",
            Self::Live => "Live transcript",
        }
    }
    pub fn accepts(self) -> &'static str {
        match self {
            Self::File => "https multipart /v1/audio/transcriptions or NDJSON …:stream",
            Self::Live => "wss live socket (stt-ws-v1 or xAI /v1/stt)",
        }
    }
    pub fn placeholder(self) -> &'static str {
        match self {
            Self::File => "https://…/v1/audio/transcriptions",
            Self::Live => "wss://…/v1/audio/transcribe",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SttEndpointError {
    Empty,
    Parse,
    Scheme(&'static str),
    PlaintextRemote,
    Credentials,
    NoHost,
}

impl std::fmt::Display for SttEndpointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("endpoint is empty"),
            Self::Parse => f.write_str("invalid endpoint URL"),
            Self::Scheme(value) => write!(f, "endpoint requires {value}"),
            Self::PlaintextRemote => {
                f.write_str("plaintext endpoints are allowed only on loopback")
            }
            Self::Credentials => f.write_str("endpoint must not contain credentials"),
            Self::NoHost => f.write_str("endpoint has no host"),
        }
    }
}

impl std::error::Error for SttEndpointError {}

/// Validate transport, TLS and user-info before a row can be persisted or used.
pub fn validate_stt_endpoint(lane: SttLane, raw: &str) -> Result<String, SttEndpointError> {
    use SttEndpointError::*;
    if raw.trim().is_empty() {
        return Err(Empty);
    }
    let url = reqwest::Url::parse(raw.trim()).map_err(|_| Parse)?;
    let (plain, secure) = match lane {
        SttLane::File => ("http", "https"),
        SttLane::Live => ("ws", "wss"),
    };
    if url.scheme() != plain && url.scheme() != secure {
        return Err(Scheme(match lane {
            SttLane::File => "http(s)",
            SttLane::Live => "ws(s)",
        }));
    }
    let host = url.host_str().ok_or(NoHost)?.trim_matches(['[', ']']);
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Credentials);
    }
    if url.scheme() == plain
        && !(host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback()))
    {
        return Err(PlaintextRemote);
    }
    Ok(url.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSttLane {
    pub lane: SttLane,
    pub endpoint: String,
    pub key_account: &'static str,
    pub auth_mode: SttAuthMode,
    pub api_key: Option<String>,
}
impl ResolvedSttLane {
    pub fn key_missing(&self) -> bool {
        // Official vendors resolve OAuth (including refresh) or their vendor key at
        // request time. This snapshot-only admission check must not open Keychain.
        crate::llm::speech::vendor_for_endpoint(&self.endpoint).is_none()
            && self.auth_mode != SttAuthMode::Unauthenticated
            && self
                .api_key
                .as_deref()
                .is_none_or(|key| key.trim().is_empty())
    }
}
impl crate::config::Config {
    /// Read only this immutable snapshot, never Keychain or process env.
    pub fn stt_lane(&self, lane: SttLane) -> Option<ResolvedSttLane> {
        let (endpoint, key) = match lane {
            SttLane::File => (&self.stt_file_endpoint, &self.stt_file_api_key),
            SttLane::Live => (&self.stt_live_endpoint, &self.stt_live_api_key),
        };
        let endpoint = endpoint.as_deref()?.trim();
        if endpoint.is_empty() {
            return None;
        }
        Some(ResolvedSttLane {
            lane,
            endpoint: endpoint.into(),
            key_account: lane.key_account(),
            auth_mode: stt_auth_mode(endpoint),
            api_key: key
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_vendor_admission_defers_credentials_without_reading_keychain() {
        let config = crate::config::Config {
            stt_file_endpoint: Some("https://api.openai.com/v1/audio/transcriptions".into()),
            stt_live_endpoint: Some("wss://api.x.ai/v1/stt".into()),
            ..Default::default()
        };
        for lane in SttLane::ALL {
            let row = config.stt_lane(lane).unwrap();
            assert_eq!(row.api_key, None);
            assert!(!row.key_missing()); // Actual absence is a typed request-time error.
        }
    }

    #[test]
    fn validates_transport_and_credential_boundaries() {
        for (lane, url) in [
            (
                SttLane::File,
                "wss://api.libraxis.cloud/v1/audio/transcribe",
            ),
            (SttLane::File, "http://example.com/stt"),
            (SttLane::File, "https://user:secret@example.com/stt"),
            (SttLane::Live, "https://example.com/stt"),
            (SttLane::Live, concat!("ws", "://example.com/stt")),
            (SttLane::Live, "wss://user@example.com/stt"),
        ] {
            assert!(validate_stt_endpoint(lane, url).is_err(), "{url}");
        }
        for (lane, url) in [
            (
                SttLane::File,
                "https://api.openai.com/v1/audio/transcriptions",
            ),
            (SttLane::File, "https://api.x.ai/v1/stt"),
            (SttLane::Live, "wss://api.x.ai/v1/stt"),
            (SttLane::File, "http://[::1]/stt"),
            (SttLane::Live, "ws://127.0.0.1/stt"),
        ] {
            assert_eq!(
                validate_stt_endpoint(lane, &format!(" {url} ")).unwrap(),
                url
            );
        }
    }

    #[test]
    fn resolved_rows_keep_endpoint_and_credential_together() {
        let config = crate::config::Config {
            stt_file_endpoint: Some("https://api.openai.com/v1/audio/transcriptions".into()),
            stt_file_api_key: Some("file-secret".into()),
            stt_live_endpoint: Some("wss://api.libraxis.cloud/v1/audio/transcribe".into()),
            ..Default::default()
        };
        let file = config.stt_lane(SttLane::File).unwrap();
        let live = config.stt_lane(SttLane::Live).unwrap();
        assert_eq!(file.api_key.as_deref(), Some("file-secret"));
        assert!(!file.key_missing());
        assert!(live.key_missing());
        assert_eq!(live.key_account, "STT_LIVE_API_KEY");
        assert!(
            crate::config::Config::default()
                .stt_lane(SttLane::File)
                .is_none()
        );
    }
}
