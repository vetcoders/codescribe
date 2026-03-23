use std::io::Write;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, FromRequest, Multipart, Request, State};
use axum::http::StatusCode;
use axum::http::header::{CONTENT_TYPE, HeaderName};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde::{Deserialize, Serialize};
use tempfile::Builder;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::info;

const AUDIO_FIELD_NAMES: &[&str] = &["audio", "file"];
const LANGUAGE_HEADER: HeaderName = HeaderName::from_static("x-language");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptionInput {
    pub bytes: Vec<u8>,
    pub filename: String,
    pub content_type: Option<String>,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TranscribeResponse {
    pub text: String,
}

pub trait TranscriptionBackend: Send + Sync + 'static {
    fn transcribe(&self, input: TranscriptionInput) -> anyhow::Result<TranscribeResponse>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct LocalTranscriptionBackend;

impl TranscriptionBackend for LocalTranscriptionBackend {
    fn transcribe(&self, input: TranscriptionInput) -> anyhow::Result<TranscribeResponse> {
        let suffix =
            infer_audio_suffix(Some(input.filename.as_str()), input.content_type.as_deref());
        let mut temp_file = Builder::new()
            .prefix("vista-kernel-stt-")
            .suffix(&suffix)
            .tempfile()
            .context("Failed to allocate temporary audio file")?;

        temp_file
            .write_all(&input.bytes)
            .context("Failed to write uploaded audio to a temporary file")?;
        temp_file
            .flush()
            .context("Failed to flush temporary audio file")?;

        let (samples, sample_rate) = crate::audio::load_audio_file(temp_file.path())
            .context("Failed to decode uploaded audio")?;
        let mut transcript = crate::stt::transcribe_long_with_segments(
            &samples,
            sample_rate,
            input.language.as_deref(),
        )
        .context("Local STT transcription failed")?;

        info!("Applying Level 1 Quality Loop Lexicons to transcript");
        transcript.text = crate::quality::lexicon::apply_lexicons(transcript.text);

        Ok(TranscribeResponse {
            text: transcript.text,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ServerConfig {
    pub max_upload_bytes: usize,
}

impl ServerConfig {
    pub fn from_env() -> Self {
        let max_upload_mb = std::env::var("BACKEND_MAX_UPLOAD_MB")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(20);

        Self {
            max_upload_bytes: max_upload_mb.saturating_mul(1024 * 1024),
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self::from_env()
    }
}

pub struct ServerHandle {
    local_addr: SocketAddr,
    shutdown_tx: Option<oneshot::Sender<()>>,
    task: JoinHandle<anyhow::Result<()>>,
}

impl ServerHandle {
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub async fn shutdown(mut self) -> anyhow::Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }

        self.join().await
    }

    pub async fn join(mut self) -> anyhow::Result<()> {
        self.shutdown_tx.take();
        self.task
            .await
            .context("STT server task panicked")?
            .context("STT server exited with an error")
    }
}

#[derive(Clone)]
struct AppState {
    backend: Arc<dyn TranscriptionBackend>,
    max_upload_bytes: usize,
}

pub fn router() -> Router {
    router_with_backend(Arc::new(LocalTranscriptionBackend), ServerConfig::default())
}

pub fn router_with_backend(backend: Arc<dyn TranscriptionBackend>, config: ServerConfig) -> Router {
    let state = AppState {
        backend,
        max_upload_bytes: config.max_upload_bytes,
    };

    Router::new()
        .route("/transcribe", post(transcribe_handler))
        .layer(DefaultBodyLimit::max(config.max_upload_bytes))
        .with_state(state)
}

pub async fn start(bind_addr: SocketAddr) -> anyhow::Result<ServerHandle> {
    start_with_backend(
        bind_addr,
        Arc::new(LocalTranscriptionBackend),
        ServerConfig::default(),
    )
    .await
}

pub async fn start_with_backend(
    bind_addr: SocketAddr,
    backend: Arc<dyn TranscriptionBackend>,
    config: ServerConfig,
) -> anyhow::Result<ServerHandle> {
    let listener = TcpListener::bind(bind_addr)
        .await
        .with_context(|| format!("Failed to bind STT server to {}", bind_addr))?;
    let local_addr = listener
        .local_addr()
        .context("Failed to resolve bound STT server address")?;
    let app = router_with_backend(backend, config);
    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    let task = tokio::spawn(async move {
        info!("vista-kernel STT server listening on {}", local_addr);
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
            .map_err(anyhow::Error::from)
    });

    Ok(ServerHandle {
        local_addr,
        shutdown_tx: Some(shutdown_tx),
        task,
    })
}

async fn transcribe_handler(State(state): State<AppState>, request: Request) -> Response {
    match extract_input(request, &state).await {
        Ok(input) => {
            let backend = Arc::clone(&state.backend);
            match tokio::task::spawn_blocking(move || backend.transcribe(input)).await {
                Ok(Ok(response)) => (StatusCode::OK, Json(response)).into_response(),
                Ok(Err(error)) => {
                    ServerError::internal(format!("Transcription failed: {error}")).into_response()
                }
                Err(error) => ServerError::internal(format!("Transcription task crashed: {error}"))
                    .into_response(),
            }
        }
        Err(error) => error.into_response(),
    }
}

async fn extract_input(
    request: Request,
    state: &AppState,
) -> Result<TranscriptionInput, ServerError> {
    let content_type = header_value(request.headers(), CONTENT_TYPE);
    let header_language = header_value(request.headers(), LANGUAGE_HEADER).map(normalize_language);

    if is_multipart(content_type.as_deref()) {
        extract_multipart_input(request, state, content_type, header_language).await
    } else {
        extract_raw_input(request, state, content_type, header_language).await
    }
}

async fn extract_multipart_input(
    request: Request,
    state: &AppState,
    fallback_content_type: Option<String>,
    fallback_language: Option<String>,
) -> Result<TranscriptionInput, ServerError> {
    let mut multipart = Multipart::from_request(request, state)
        .await
        .map_err(|rejection| ServerError::new(rejection.status(), rejection.body_text()))?;

    let mut bytes = None;
    let mut filename = None;
    let mut content_type = fallback_content_type;
    let mut language = fallback_language;

    while let Some(field) = multipart.next_field().await.map_err(|error| {
        ServerError::bad_request(format!("Malformed multipart payload: {error}"))
    })? {
        match field.name() {
            Some(name) if AUDIO_FIELD_NAMES.contains(&name) => {
                if bytes.is_none() {
                    filename = field.file_name().map(ToOwned::to_owned);
                    content_type = field.content_type().map(ToOwned::to_owned).or(content_type);
                    let uploaded = field.bytes().await.map_err(|error| {
                        ServerError::bad_request(format!("Failed to read audio field: {error}"))
                    })?;
                    enforce_upload_limit(uploaded.len(), state.max_upload_bytes)?;
                    bytes = Some(uploaded.to_vec());
                }
            }
            Some("language") => {
                let value = field.text().await.map_err(|error| {
                    ServerError::bad_request(format!("Failed to read language field: {error}"))
                })?;
                if !value.trim().is_empty() {
                    language = Some(normalize_language(value));
                }
            }
            _ => {}
        }
    }

    let bytes = bytes.ok_or_else(|| {
        ServerError::bad_request("Missing multipart audio field 'audio' or 'file'")
    })?;
    if bytes.is_empty() {
        return Err(ServerError::bad_request("Uploaded audio is empty"));
    }

    let filename =
        filename.unwrap_or_else(|| default_filename(content_type.as_deref(), "audio-upload"));

    Ok(TranscriptionInput {
        bytes,
        filename,
        content_type,
        language,
    })
}

async fn extract_raw_input(
    request: Request,
    state: &AppState,
    content_type: Option<String>,
    language: Option<String>,
) -> Result<TranscriptionInput, ServerError> {
    let body = Bytes::from_request(request, state)
        .await
        .map_err(|rejection| ServerError::new(rejection.status(), rejection.body_text()))?;
    enforce_upload_limit(body.len(), state.max_upload_bytes)?;
    if body.is_empty() {
        return Err(ServerError::bad_request("Request body is empty"));
    }

    Ok(TranscriptionInput {
        bytes: body.to_vec(),
        filename: default_filename(content_type.as_deref(), "audio"),
        content_type,
        language,
    })
}

fn enforce_upload_limit(size: usize, limit: usize) -> Result<(), ServerError> {
    if size > limit {
        return Err(ServerError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "Audio payload too large: received {} bytes, limit is {} bytes",
                size, limit
            ),
        ));
    }

    Ok(())
}

fn header_value(headers: &axum::http::HeaderMap, name: HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn is_multipart(content_type: Option<&str>) -> bool {
    content_type
        .map(|value| {
            value
                .to_ascii_lowercase()
                .starts_with("multipart/form-data")
        })
        .unwrap_or(false)
}

fn normalize_language(value: impl AsRef<str>) -> String {
    value.as_ref().trim().to_ascii_lowercase()
}

fn default_filename(content_type: Option<&str>, stem: &str) -> String {
    let suffix = infer_audio_suffix(None, content_type);
    format!("{stem}{suffix}")
}

fn infer_audio_suffix(filename_hint: Option<&str>, content_type: Option<&str>) -> String {
    if let Some(filename) = filename_hint {
        let extension = Path::new(filename)
            .extension()
            .and_then(|value| value.to_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_lowercase());

        if let Some(extension) = extension {
            return format!(".{extension}");
        }
    }

    match content_type
        .map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or(value)
                .trim()
                .to_ascii_lowercase()
        })
        .as_deref()
    {
        Some("audio/mpeg") | Some("audio/mp3") => ".mp3".to_string(),
        Some("audio/mp4") | Some("audio/x-m4a") => ".m4a".to_string(),
        Some("audio/webm") => ".webm".to_string(),
        Some("audio/ogg") => ".ogg".to_string(),
        Some("audio/flac") => ".flac".to_string(),
        _ => ".wav".to_string(),
    }
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Debug)]
struct ServerError {
    status: StatusCode,
    message: String,
}

impl ServerError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }
}

impl IntoResponse for ServerError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorResponse {
                error: self.message,
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::{Body, to_bytes};
    use axum::http::Request as HttpRequest;
    use tower::ServiceExt;

    #[derive(Default)]
    struct RecordingBackend {
        calls: std::sync::Mutex<Vec<TranscriptionInput>>,
    }

    impl TranscriptionBackend for RecordingBackend {
        fn transcribe(&self, input: TranscriptionInput) -> anyhow::Result<TranscribeResponse> {
            self.calls.lock().unwrap().push(input.clone());
            Ok(TranscribeResponse {
                text: format!("{} bytes", input.bytes.len()),
            })
        }
    }

    #[tokio::test]
    async fn router_accepts_multipart_audio_field() {
        let backend = Arc::new(RecordingBackend::default());
        let app = router_with_backend(backend.clone(), ServerConfig::default());
        let boundary = "vista-boundary";
        let mut body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"audio\"; filename=\"clip.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
        )
        .into_bytes();
        body.extend_from_slice(b"RIFFtest");
        body.extend_from_slice(
            format!(
                "\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"language\"\r\n\r\npl\r\n--{boundary}--\r\n"
            )
            .as_bytes(),
        );

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/transcribe")
                    .header(
                        CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let payload: TranscribeResponse =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(payload.text, "8 bytes");

        let calls = backend.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].filename, "clip.wav");
        assert_eq!(calls[0].content_type.as_deref(), Some("audio/wav"));
        assert_eq!(calls[0].language.as_deref(), Some("pl"));
        assert_eq!(calls[0].bytes, b"RIFFtest");
    }

    #[tokio::test]
    async fn router_accepts_raw_audio_body() {
        let backend = Arc::new(RecordingBackend::default());
        let app = router_with_backend(backend.clone(), ServerConfig::default());

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/transcribe")
                    .header(CONTENT_TYPE, "audio/webm")
                    .header(LANGUAGE_HEADER, "EN")
                    .body(Body::from("raw-bytes"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let payload: TranscribeResponse =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(payload.text, "9 bytes");

        let calls = backend.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].filename, "audio.webm");
        assert_eq!(calls[0].content_type.as_deref(), Some("audio/webm"));
        assert_eq!(calls[0].language.as_deref(), Some("en"));
        assert_eq!(calls[0].bytes, b"raw-bytes");
    }
}
