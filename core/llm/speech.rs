//! On-demand vendor speech, sharing the assistive provider and account authority.
//! This sibling of the LLM transport deliberately does not enable local CSM.
use super::{
    account_auth,
    provider::ProviderKind,
    vendors::{openai, xai},
};
use crate::config::{Config, RuntimeLlmLane, keychain};
use base64::Engine as _;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    path::{Path, PathBuf},
    time::Duration,
};

/// Cancellable output independent of the disabled local TTS engine.
pub mod playback;

/// All speech responses and cache entries are mono signed little-endian PCM16.
pub const SAMPLE_RATE: u32 = 24_000;
/// Content-free diagnostic errors: never include response bodies or credentials.
#[derive(Debug, PartialEq, Eq)]
pub enum SpeechError {
    /// No vendor speech protocol exists.
    Unsupported(String),
    /// Neither account nor API key is present.
    MissingCredentials(&'static str),
    /// A signed-in account failed; API key fallback is forbidden.
    Account(&'static str),
    /// The selected credential has no supported public speech capability here.
    Capability(&'static str),
    /// Invalid input, settings, response, or storage operation.
    Invalid(&'static str),
    /// HTTP refusal, safe for probes and UI.
    Http(u16),
    /// Transport did not return an HTTP response.
    Transport,
}
impl fmt::Display for SpeechError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(v) => write!(f, "unavailable: {v} has no speech endpoint"),
            Self::MissingCredentials(v) => {
                write!(f, "unavailable: {v}: no signed-in account and no API key")
            }
            Self::Account(v) => write!(
                f,
                "unavailable: {v} account authentication failed; sign in again"
            ),
            Self::Capability(reason) => f.write_str(reason),
            Self::Invalid(s) => f.write_str(s),
            Self::Http(s) => write!(f, "Speech endpoint returned HTTP {s}"),
            Self::Transport => f.write_str("Speech endpoint transport failed"),
        }
    }
}
impl std::error::Error for SpeechError {}

/// The chosen credential mechanism, never the credential itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthSource {
    OAuth,
    ApiKey,
}
impl AuthSource {
    /// Machine-readable probe label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OAuth => "oauth",
            Self::ApiKey => "api_key",
        }
    }
}
/// Intentionally not Debug: contains a bearer secret.
pub struct SpeechAuth {
    pub bearer: String,
    pub source: AuthSource,
}

fn pins(vendor: ProviderKind) -> Result<(&'static str, &'static str, &'static str), SpeechError> {
    match vendor {
        ProviderKind::OpenAiResponses => {
            Ok(("openai", openai::API_KEY_ACCOUNT, openai::TTS_ENDPOINT))
        }
        ProviderKind::XaiResponses => Ok(("xai", xai::API_KEY_ACCOUNT, xai::TTS_ENDPOINT)),
        _ => Err(SpeechError::Unsupported(format!("{vendor:?}"))),
    }
}
/// Recognize only exact TLS vendor origins. Never send a vendor token to a custom endpoint.
pub fn vendor_for_endpoint(endpoint: &str) -> Option<ProviderKind> {
    let url = reqwest::Url::parse(endpoint).ok()?;
    if !matches!(url.scheme(), "https" | "wss")
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    match url.host_str()? {
        "api.openai.com" => Some(ProviderKind::OpenAiResponses),
        "api.x.ai" => Some(ProviderKind::XaiResponses),
        _ => None,
    }
}
/// Request-time account lookup; never called by the settings loader.
pub fn vendor_signed_in(vendor: ProviderKind) -> bool {
    account_auth::account_status(vendor).signed_in
}
fn api_key(vendor: ProviderKind, fallback: Option<&str>) -> Option<String> {
    let (_, account, _) = pins(vendor).ok()?;
    fallback
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| keychain::runtime_key(account))
        .filter(|s| !s.trim().is_empty())
}
async fn resolve_with<F, Fut, K>(
    vendor: ProviderKind,
    signed_in: bool,
    oauth: F,
    key: K,
) -> Result<SpeechAuth, SpeechError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<String, SpeechError>>,
    K: FnOnce() -> Option<String>,
{
    let (name, _, _) = pins(vendor)?;
    // Founder's order (2026-09-09 16:41): the signed-in account before the
    // stored key; the key serves a vendor with no account.
    if signed_in {
        let bearer = oauth().await?;
        if bearer.trim().is_empty() {
            return Err(SpeechError::Account(name));
        }
        return Ok(SpeechAuth {
            bearer,
            source: AuthSource::OAuth,
        });
    }
    let bearer = key()
        .filter(|s| !s.trim().is_empty())
        .ok_or(SpeechError::MissingCredentials(name))?;
    Ok(SpeechAuth {
        bearer,
        source: AuthSource::ApiKey,
    })
}
/// OAuth first, and only an absent account admits API key fallback.
pub async fn resolve_vendor_auth(
    vendor: ProviderKind,
    fallback_key: Option<&str>,
) -> Result<SpeechAuth, SpeechError> {
    resolve_vendor_auth_using(vendor, || api_key(vendor, fallback_key)).await
}

async fn resolve_vendor_auth_using(
    vendor: ProviderKind,
    key: impl FnOnce() -> Option<String>,
) -> Result<SpeechAuth, SpeechError> {
    let (name, _, _) = pins(vendor)?;
    let signed_in = vendor_signed_in(vendor);
    if signed_in {
        speech_capability(vendor, AuthSource::OAuth)?;
    }
    resolve_with(
        vendor,
        signed_in,
        || async move {
            account_auth::access_token(vendor)
                .await
                .map_err(|_| SpeechError::Account(name))
        },
        key,
    )
    .await
}

/// Settings sealed once for a synthesis; cache identity includes all audible options.
#[derive(Clone, Debug)]
pub struct SpeechOptions {
    pub vendor: ProviderKind,
    pub model: String,
    pub voice: String,
    pub speed: f32,
}
impl SpeechOptions {
    /// Resolve vendor-specific defaults and validate the optional environment overrides.
    pub fn for_vendor(vendor: ProviderKind) -> Result<Self, SpeechError> {
        pins(vendor)?;
        let read = |key, default: &str| {
            std::env::var(key)
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| default.into())
        };
        let (model, voice) = match vendor {
            ProviderKind::OpenAiResponses => (
                read("SPEECH_TTS_MODEL_OPENAI", openai::DEFAULT_TTS_MODEL),
                read("SPEECH_TTS_VOICE_OPENAI", openai::DEFAULT_TTS_VOICE),
            ),
            _ => (
                xai::DEFAULT_TTS_MODEL.into(),
                read("SPEECH_TTS_VOICE_XAI", xai::DEFAULT_TTS_VOICE),
            ),
        };
        let speed = read("SPEECH_TTS_SPEED", "1.25")
            .parse::<f32>()
            .map_err(|_| SpeechError::Invalid("SPEECH_TTS_SPEED must be a number"))?;
        let value = Self {
            vendor,
            model,
            voice,
            speed,
        };
        value.validate()?;
        Ok(value)
    }
    fn validate(&self) -> Result<(), SpeechError> {
        pins(self.vendor)?;
        let range = if self.vendor == ProviderKind::XaiResponses {
            0.7..=1.5
        } else {
            0.25..=4.0
        };
        if !range.contains(&self.speed) {
            return Err(SpeechError::Invalid(
                "SPEECH_TTS_SPEED outside vendor range (OpenAI 0.25–4; xAI 0.7–1.5)",
            ));
        }
        if self.voice.trim().is_empty() {
            return Err(SpeechError::Invalid("Speech voice is empty"));
        }
        if self.vendor == ProviderKind::OpenAiResponses && self.model.trim().is_empty() {
            return Err(SpeechError::Invalid("Speech model is empty"));
        }
        Ok(())
    }
    /// Vendor cap in Unicode characters, never UTF-8 bytes.
    pub fn character_cap(&self) -> usize {
        if self.vendor == ProviderKind::XaiResponses {
            15_000
        } else {
            4096
        }
    }
    /// Vendor body; xAI has no model parameter.
    pub fn request_body(&self, text: &str) -> Value {
        if self.vendor == ProviderKind::XaiResponses {
            json!({"text":text,"voice_id":self.voice,"language":"auto","output_format":{"codec":"pcm","sample_rate":SAMPLE_RATE},"speed":self.speed})
        } else {
            json!({"model":self.model,"input":text,"voice":self.voice,"response_format":"pcm","speed":self.speed})
        }
    }
    fn cache_key(&self, text: &str) -> String {
        let identity = json!({"vendor":format!("{:?}",self.vendor),"model":self.model,"voice":self.voice,"speed":self.speed,"text":text,"sample_rate":SAMPLE_RATE});
        format!("{:x}", Sha256::digest(identity.to_string().as_bytes()))
    }
}
fn lane_vendor(lane: &RuntimeLlmLane) -> Result<ProviderKind, SpeechError> {
    let vendor = lane
        .vendor()
        .ok_or_else(|| SpeechError::Unsupported(lane.provider_display_name().into()))?;
    pins(vendor).map_err(|_| SpeechError::Unsupported(lane.provider_display_name().into()))?;
    Ok(vendor)
}
/// ChatGPT/Codex login is not evidence of public OpenAI audio permissions.
/// Keep the selected account authoritative instead of silently using a key.
fn speech_capability(vendor: ProviderKind, source: AuthSource) -> Result<(), SpeechError> {
    pins(vendor)?;
    if vendor == ProviderKind::OpenAiResponses && source == AuthSource::OAuth {
        return Err(SpeechError::Capability(
            "OpenAI account speech is not supported by this integration; Codex chat login does not establish public audio permissions. No API-key fallback was attempted.",
        ));
    }
    Ok(())
}
/// None means credentials and configuration are present; it is not an API liveness claim.
pub fn speech_availability() -> Option<String> {
    let check = || -> Result<(), SpeechError> {
        let settings = Config::load_runtime_snapshot()
            .map_err(|_| SpeechError::Invalid("Speech settings could not be loaded"))?;
        let lane = settings.llm_lanes().assistive();
        let vendor = lane_vendor(lane)?;
        SpeechOptions::for_vendor(vendor)?;
        if vendor_signed_in(vendor) {
            speech_capability(vendor, AuthSource::OAuth)?;
        } else if api_key(vendor, lane.credential().request_api_key().as_deref()).is_none() {
            return Err(SpeechError::MissingCredentials(pins(vendor)?.0));
        }
        Ok(())
    };
    check().err().map(|e| e.to_string())
}
/// Audio plus content-free cache/transport evidence.
pub struct SpeechAudio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub cached: bool,
    pub http_status: Option<u16>,
    pub auth_source: AuthSource,
}
impl SpeechAudio {
    /// PCM duration, not request latency.
    pub fn duration_ms(&self) -> u64 {
        self.samples.len() as u64 * 1000 / u64::from(self.sample_rate)
    }
}
/// Reject partial samples rather than silently truncating a corrupt response.
pub fn decode_pcm(bytes: &[u8]) -> Result<Vec<f32>, SpeechError> {
    if bytes.is_empty() || !bytes.len().is_multiple_of(2) {
        return Err(SpeechError::Invalid("Invalid PCM16 speech response"));
    }
    Ok(bytes
        .chunks_exact(2)
        .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0)
        .collect())
}
/// Lossless cap splitting, preferring whitespace boundaries when possible.
pub fn chunks(text: &str, cap: usize) -> Vec<&str> {
    if cap == 0 {
        return vec![];
    }
    let mut remaining = text;
    let mut result = Vec::new();
    while !remaining.is_empty() {
        let end = remaining
            .char_indices()
            .nth(cap)
            .map_or(remaining.len(), |(i, _)| i);
        let split = if end == remaining.len() {
            end
        } else {
            remaining[..end]
                .char_indices()
                .rev()
                .find(|(_, c)| c.is_whitespace())
                .map_or(end, |(i, c)| i + c.len_utf8())
        };
        result.push(&remaining[..split]);
        remaining = &remaining[split..];
    }
    result
}
fn cache_dir() -> Result<PathBuf, SpeechError> {
    let home =
        directories::BaseDirs::new().ok_or(SpeechError::Invalid("Home directory unavailable"))?;
    Ok(home.home_dir().join(".codescribe/cache/tts"))
}
/// Synthesize using the current assistive lane, sealed for the entire request.
pub async fn synthesize(text: &str) -> Result<SpeechAudio, SpeechError> {
    let settings = Config::load_runtime_snapshot()
        .map_err(|_| SpeechError::Invalid("Speech settings could not be loaded"))?;
    let lane = settings.llm_lanes().assistive();
    let vendor = lane_vendor(lane)?;
    let options = SpeechOptions::for_vendor(vendor)?;
    let auth = resolve_vendor_auth_using(vendor, || {
        api_key(vendor, lane.credential().request_api_key().as_deref())
    })
    .await?;
    synthesize_with(text, &options, &auth).await
}
/// Probe entry point; uses the same network and cache path as the Agent.
pub async fn synthesize_with(
    text: &str,
    options: &SpeechOptions,
    auth: &SpeechAuth,
) -> Result<SpeechAudio, SpeechError> {
    speech_capability(options.vendor, auth.source)?;
    let (_, _, endpoint) = pins(options.vendor)?;
    synthesize_at(text, options, auth, endpoint, &cache_dir()?).await
}
async fn synthesize_at(
    text: &str,
    options: &SpeechOptions,
    auth: &SpeechAuth,
    endpoint: &str,
    cache: &Path,
) -> Result<SpeechAudio, SpeechError> {
    options.validate()?;
    if auth.bearer.trim().is_empty() {
        return Err(SpeechError::MissingCredentials(pins(options.vendor)?.0));
    }
    if text.trim().is_empty() {
        return Err(SpeechError::Invalid("No text to speak"));
    }
    tokio::fs::create_dir_all(cache)
        .await
        .map_err(|_| SpeechError::Invalid("Cannot create speech cache"))?;
    // A cache hit is never evidence that a different account has permission.
    // Hash the credential into the namespace; never store the bearer itself.
    let path = cache.join(format!(
        "{}.pcm",
        authenticated_cache_key(options, text, auth, endpoint)
    ));
    if let Ok(bytes) = tokio::fs::read(&path).await
        && let Ok(samples) = decode_pcm(&bytes)
    {
        return Ok(SpeechAudio {
            samples,
            sample_rate: SAMPLE_RATE,
            cached: true,
            http_status: None,
            auth_source: auth.source,
        });
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(180))
        .connect_timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| SpeechError::Transport)?;
    let mut pcm = Vec::new();
    let mut status = None;
    for chunk in chunks(text, options.character_cap()) {
        let response = client
            .post(endpoint)
            .bearer_auth(&auth.bearer)
            .json(&options.request_body(chunk))
            .send()
            .await
            .map_err(|_| SpeechError::Transport)?;
        let code = response.status().as_u16();
        if !response.status().is_success() {
            return Err(SpeechError::Http(code));
        }
        status = Some(code);
        let is_json = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|s| s.starts_with("application/json"));
        let bytes = response.bytes().await.map_err(|_| SpeechError::Transport)?;
        let bytes = if options.vendor == ProviderKind::XaiResponses && is_json {
            let body: Value = serde_json::from_slice(&bytes)
                .map_err(|_| SpeechError::Invalid("Invalid xAI speech response"))?;
            base64::engine::general_purpose::STANDARD
                .decode(
                    body.get("audio")
                        .and_then(Value::as_str)
                        .ok_or(SpeechError::Invalid("Missing xAI audio"))?,
                )
                .map_err(|_| SpeechError::Invalid("Invalid xAI base64 audio"))?
        } else {
            bytes.to_vec()
        };
        decode_pcm(&bytes)?;
        pcm.extend(bytes);
    }
    let samples = decode_pcm(&pcm)?;
    // Atomic per-call temporary file prevents concurrent writers publishing partial PCM.
    let tmp = cache.join(format!("{}.tmp", uuid::Uuid::new_v4()));
    let write = async {
        use tokio::io::AsyncWriteExt;
        let mut open = std::fs::OpenOptions::new();
        open.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            open.mode(0o600);
        }
        // Establish the owned file before yielding. Cancellation cannot race
        // a background open into recreating it after the cleanup guard drops.
        let mut file = tokio::fs::File::from_std(open.open(&tmp)?);
        let _cleanup = SpeechCacheTemporary(tmp.clone());
        file.write_all(&pcm).await?;
        file.sync_all().await?;
        tokio::fs::rename(&tmp, &path).await
    }
    .await;
    if write.is_err() {
        return Err(SpeechError::Invalid("Cannot write speech cache"));
    }
    Ok(SpeechAudio {
        samples,
        sample_rate: SAMPLE_RATE,
        cached: false,
        http_status: status,
        auth_source: auth.source,
    })
}

struct SpeechCacheTemporary(PathBuf);

impl Drop for SpeechCacheTemporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn authenticated_cache_key(
    options: &SpeechOptions,
    text: &str,
    auth: &SpeechAuth,
    endpoint: &str,
) -> String {
    let identity = json!({
        "version": 2,
        "audio": options.cache_key(text),
        "endpoint": endpoint,
        "auth_source": auth.source.as_str(),
        "credential": format!("{:x}", Sha256::digest(auth.bearer.as_bytes())),
    });
    format!("{:x}", Sha256::digest(identity.to_string().as_bytes()))
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod rc_w1_tests {
    use super::*;
    const ENDPOINT: &str = "https://api.openai.com/v1/audio/speech";

    fn options() -> SpeechOptions {
        SpeechOptions {
            vendor: ProviderKind::OpenAiResponses,
            model: "test-model".into(),
            voice: "cedar".into(),
            speed: 1.0,
        }
    }

    #[tokio::test]
    async fn public_speech_refuses_codex_oauth_before_cache_or_network() {
        let result = synthesize_with(
            "Hello",
            &options(),
            &SpeechAuth {
                bearer: "test-account".into(),
                source: AuthSource::OAuth,
            },
        )
        .await;
        assert!(matches!(result, Err(SpeechError::Capability(_))));
        assert!(speech_capability(ProviderKind::XaiResponses, AuthSource::OAuth).is_ok());
        assert!(speech_capability(ProviderKind::OpenAiResponses, AuthSource::ApiKey).is_ok());
        assert!(matches!(
            speech_capability(ProviderKind::AnthropicMessages, AuthSource::ApiKey),
            Err(SpeechError::Unsupported(_))
        ));
    }

    #[tokio::test]
    async fn empty_or_failed_account_never_reads_a_fallback_key() {
        for failure in [false, true] {
            let result = resolve_with(
                ProviderKind::XaiResponses,
                true,
                || async move {
                    if failure {
                        Err(SpeechError::Account("xai"))
                    } else {
                        Ok("  ".into())
                    }
                },
                || panic!("selected account forbids key fallback"),
            )
            .await;
            assert!(matches!(result, Err(SpeechError::Account("xai"))));
        }
    }

    #[test]
    fn cache_identity_covers_audio_endpoint_and_credential() {
        let auth = SpeechAuth {
            bearer: "test-key".into(),
            source: AuthSource::ApiKey,
        };
        let original = options();
        let key = authenticated_cache_key(&original, "Hello", &auth, ENDPOINT);
        let mut variants = Vec::new();
        let mut voice = options();
        voice.voice = "marin".into();
        variants.push(voice);
        let mut model = options();
        model.model = "other-model".into();
        variants.push(model);
        let mut speed = options();
        speed.speed = 1.25;
        variants.push(speed);
        for variant in variants {
            assert_ne!(
                key,
                authenticated_cache_key(&variant, "Hello", &auth, ENDPOINT)
            );
        }
        for (text, endpoint, bearer, source) in [
            ("Other", ENDPOINT, "test-key", AuthSource::ApiKey),
            (
                "Hello",
                "https://other.invalid/speech",
                "test-key",
                AuthSource::ApiKey,
            ),
            ("Hello", ENDPOINT, "other-key", AuthSource::ApiKey),
            ("Hello", ENDPOINT, "test-key", AuthSource::OAuth),
        ] {
            let changed = SpeechAuth {
                bearer: bearer.into(),
                source,
            };
            assert_ne!(
                key,
                authenticated_cache_key(&original, text, &changed, endpoint)
            );
        }
        assert!(!key.contains("test-key"));
    }

    #[tokio::test]
    async fn changed_credential_cannot_hide_refusal_behind_cached_audio() {
        let mut server = mockito::Server::new_async().await;
        let allowed = server
            .mock("POST", "/")
            .match_header("authorization", "Bearer allowed-key")
            .with_status(200)
            .with_body(vec![0, 0, 1, 0])
            .expect(1)
            .create_async()
            .await;
        let refused = server
            .mock("POST", "/")
            .match_header("authorization", "Bearer refused-key")
            .with_status(403)
            .with_body("private provider detail")
            .expect(1)
            .create_async()
            .await;
        let dir = tempfile::tempdir().unwrap();
        let auth = SpeechAuth {
            bearer: "allowed-key".into(),
            source: AuthSource::ApiKey,
        };
        let first = synthesize_at("Hello", &options(), &auth, &server.url(), dir.path())
            .await
            .unwrap();
        let repeat = synthesize_at("Hello", &options(), &auth, &server.url(), dir.path())
            .await
            .unwrap();
        assert!(!first.cached);
        assert!(repeat.cached);
        let auth = SpeechAuth {
            bearer: "refused-key".into(),
            source: AuthSource::ApiKey,
        };
        let result = synthesize_at("Hello", &options(), &auth, &server.url(), dir.path()).await;
        assert!(matches!(result, Err(SpeechError::Http(403))));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        allowed.assert_async().await;
        refused.assert_async().await;
    }

    #[test]
    fn cancelled_cache_write_removes_only_its_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let partial = dir.path().join("owned.tmp");
        let complete = dir.path().join("complete.pcm");
        std::fs::write(&partial, [0]).unwrap();
        std::fs::write(&complete, [0, 0]).unwrap();
        drop(SpeechCacheTemporary(partial.clone()));
        assert!(!partial.exists());
        assert_eq!(std::fs::read(complete).unwrap(), vec![0, 0]);
    }
}
