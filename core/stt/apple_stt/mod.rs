//! Apple on-device STT adapter (macOS 26+) via Swift bridge subprocess.
//!
//! Dual-backend probe/transcribe order (per locale):
//! 1. **SpeechTranscriber** (`SpeechAnalyzer`) when the locale is supported+installed
//! 2. **SFSpeechRecognizer on-device** when ST lacks the locale but SF supports it
//!    (notably **pl-PL** — product foundation, not a "legacy" path)
//! 3. Honest error only when neither backend can serve the locale
//!
//! Runtime fallback to Candle Whisper is handled in `core/stt/mod.rs`.
//!
//! Bridge protocol: JSON request on stdin, JSON response on stdout (protocol v1;
//! optional `backend` field is additive).

use std::fs;
use std::io::{Read as _, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::pipeline::contracts::{
    RawTranscript, SpeechUtterance, TranscriptSegment, TranscriptionAdapter,
    TranscriptionEngineMode, TranscriptionEngineVerdict, TranscriptionSource, TranscriptionVerdict,
};

const MIN_SUPPORTED_MACOS_MAJOR: u32 = 26;
const DEFAULT_LOCALE: &str = "pl-PL";
const DEFAULT_BRIDGE_BIN: &str = "codescribe-stt-bridge";

const ENV_STT_BRIDGE: &str = "CODESCRIBE_APPLE_STT_BRIDGE";
const ENV_LOCALE: &str = "CODESCRIBE_APPLE_STT_LOCALE";
const ENV_ALLOW_DOWNLOAD: &str = "CODESCRIBE_APPLE_STT_ALLOW_DOWNLOAD";

const BRIDGE_TRANSCRIBE_TIMEOUT: Duration = Duration::from_secs(30);
const BRIDGE_PROBE_TIMEOUT: Duration = Duration::from_secs(120);

/// Zero-sized adapter using Apple's SpeechAnalyzer via subprocess bridge.
pub struct AppleSpeechAnalyzerAdapter;

impl AppleSpeechAnalyzerAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AppleSpeechAnalyzerAdapter {
    fn default() -> Self {
        Self
    }
}

impl TranscriptionAdapter for AppleSpeechAnalyzerAdapter {
    fn transcribe(
        &self,
        utterance: &SpeechUtterance,
        language: Option<&str>,
    ) -> Result<RawTranscript> {
        transcribe_long_with_segments(&utterance.samples, utterance.sample_rate, language)
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct BridgeRequest<'a> {
    protocol_version: u8,
    command: &'a str,
    locale: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    audio_path: Option<&'a str>,
    allow_download: bool,
}

/// Apple bridge backend selected for a locale (matches Swift `AppleSttBackend`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppleSttBackend {
    SpeechTranscriber,
    SfSpeechRecognizer,
}

impl AppleSttBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SpeechTranscriber => "speech_transcriber",
            Self::SfSpeechRecognizer => "sf_speech_recognizer",
        }
    }

    pub fn from_bridge(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "speech_transcriber" => Some(Self::SpeechTranscriber),
            "sf_speech_recognizer" => Some(Self::SfSpeechRecognizer),
            _ => None,
        }
    }

    pub fn engine_mode(self) -> TranscriptionEngineMode {
        match self {
            Self::SpeechTranscriber => TranscriptionEngineMode::SpeechTranscriber,
            Self::SfSpeechRecognizer => TranscriptionEngineMode::SfSpeechOnDevice,
        }
    }
}

#[derive(Debug, Deserialize, Default)]
struct BridgeResponse {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    status: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    segments: Vec<BridgeSegment>,
    #[serde(default)]
    locale_supported: Option<bool>,
    #[serde(default)]
    locale_installed: Option<bool>,
    /// Optional dual-backend field: `speech_transcriber` | `sf_speech_recognizer`.
    #[serde(default)]
    backend: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

impl BridgeResponse {
    fn is_ok(&self) -> bool {
        self.ok || self.status.eq_ignore_ascii_case("ok")
    }
}

#[derive(Debug, Deserialize)]
struct BridgeSegment {
    text: String,
    start_ts: f32,
    end_ts: f32,
}

/// Initialize Apple STT backend once (platform + bridge + locale readiness).
pub fn init() -> Result<()> {
    static INIT: OnceLock<std::result::Result<(), String>> = OnceLock::new();
    let cached = INIT.get_or_init(|| init_impl().map_err(|e| format!("{:#}", e)));
    match cached {
        Ok(()) => Ok(()),
        Err(message) => bail!("{message}"),
    }
}

fn init_impl() -> Result<()> {
    ensure_supported_platform()?;
    let locale = resolved_locale(None);
    let allow_download = env_bool(ENV_ALLOW_DOWNLOAD, true);
    let probe = probe_bridge(&locale, allow_download)?;

    if !probe.supported {
        bail!(
            "Apple on-device STT does not support locale '{locale}' \
             (neither SpeechTranscriber nor SFSpeechRecognizer on-device)"
        );
    }
    if !probe.installed {
        let backend = probe
            .backend
            .map(|b| b.as_str())
            .unwrap_or("unknown_backend");
        bail!(
            "Apple on-device STT assets for locale '{locale}' via {backend} are missing; \
             set {ENV_ALLOW_DOWNLOAD}=1 to auto-install SpeechTranscriber assets when applicable"
        );
    }

    if let Some(backend) = probe.backend {
        tracing::info!(
            "Apple STT ready locale={locale} backend={}",
            backend.as_str()
        );
    }

    Ok(())
}

/// Runtime availability guard used by engine router.
pub(crate) fn is_runtime_available() -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    match macos_major_version() {
        Ok(major) => major >= MIN_SUPPORTED_MACOS_MAJOR,
        Err(_) => false,
    }
}

/// Transcribe a single chunk (same implementation as long for this backend).
// FORGOTTEN-GEM(vc-prune 2026-06-10): parked sync transcription contract —
// see core/stt/mod.rs::candle_transcribe_chunk for the cluster rationale.
#[allow(dead_code)]
pub(crate) fn transcribe_chunk(
    audio: &[f32],
    sample_rate: u32,
    language: Option<&str>,
) -> Result<String> {
    Ok(transcribe_long_with_segments(audio, sample_rate, language)?.text)
}

/// Transcribe audio with optional segment timestamps.
pub(crate) fn transcribe_long_with_segments(
    audio: &[f32],
    sample_rate: u32,
    language: Option<&str>,
) -> Result<RawTranscript> {
    transcribe_via_bridge(audio, sample_rate, language)
}

/// "Try" variant kept for scheduler API symmetry.
#[allow(dead_code)]
pub(crate) fn try_transcribe_long_with_segments(
    audio: &[f32],
    sample_rate: u32,
    language: Option<&str>,
) -> Result<RawTranscript> {
    transcribe_via_bridge(audio, sample_rate, language)
}

/// Convenience helper for batch/offline file transcription.
pub fn transcribe_file(path: &Path, language: Option<&str>) -> Result<RawTranscript> {
    Ok(transcribe_file_with_backend(path, language)?.0)
}

/// File transcription with Apple backend provenance for final-pass adjudication.
pub fn transcribe_file_verdict(
    path: &Path,
    language: Option<&str>,
) -> Result<TranscriptionVerdict> {
    let (raw, backend) = transcribe_file_with_backend(path, language)?;
    let mode = backend
        .map(AppleSttBackend::engine_mode)
        .unwrap_or(TranscriptionEngineMode::SfSpeechOnDevice);
    let text = raw.text.clone();
    Ok(TranscriptionVerdict::from_parts(
        text,
        raw,
        None,
        TranscriptionSource::LocalFinalPass,
        TranscriptionEngineVerdict::apple(mode),
        None,
    ))
}

/// Transcribe a path already on disk without re-encoding (preferred final-pass path).
fn transcribe_file_with_backend(
    path: &Path,
    language: Option<&str>,
) -> Result<(RawTranscript, Option<AppleSttBackend>)> {
    init()?;
    let locale = resolved_locale(language);
    let audio_path = path.display().to_string();
    let request = BridgeRequest {
        protocol_version: 1,
        command: "transcribe",
        locale: &locale,
        audio_path: Some(audio_path.as_str()),
        allow_download: env_bool(ENV_ALLOW_DOWNLOAD, true),
    };
    let response = run_bridge_with_timeout(&request, Some(BRIDGE_TRANSCRIBE_TIMEOUT))
        .context("Apple STT bridge transcribe failed")?;
    let backend = response
        .backend
        .as_deref()
        .and_then(AppleSttBackend::from_bridge);
    Ok((raw_transcript_from_bridge_response(response), backend))
}

fn transcribe_via_bridge(
    audio: &[f32],
    sample_rate: u32,
    language: Option<&str>,
) -> Result<RawTranscript> {
    if audio.is_empty() {
        return Ok(RawTranscript::default());
    }

    init()?;

    let wav = TempWavFile::write(audio, sample_rate)?;
    let audio_path = wav.path().display().to_string();
    let locale = resolved_locale(language);
    let request = BridgeRequest {
        protocol_version: 1,
        command: "transcribe",
        locale: &locale,
        audio_path: Some(audio_path.as_str()),
        allow_download: env_bool(ENV_ALLOW_DOWNLOAD, true),
    };
    let response = run_bridge_with_timeout(&request, Some(BRIDGE_TRANSCRIBE_TIMEOUT))
        .context("Apple STT bridge transcribe failed")?;

    Ok(raw_transcript_from_bridge_response(response))
}

fn raw_transcript_from_bridge_response(response: BridgeResponse) -> RawTranscript {
    let segments = response
        .segments
        .into_iter()
        .filter_map(bridge_segment_to_transcript_segment)
        .collect();
    RawTranscript {
        text: response.text.trim().to_string(),
        segments,
        ..Default::default()
    }
}

fn bridge_segment_to_transcript_segment(seg: BridgeSegment) -> Option<TranscriptSegment> {
    let text = seg.text.trim().to_string();
    if text.is_empty()
        || !seg.start_ts.is_finite()
        || !seg.end_ts.is_finite()
        || seg.end_ts < seg.start_ts
    {
        return None;
    }
    Some(TranscriptSegment {
        text,
        start_ts: seg.start_ts,
        end_ts: seg.end_ts,
    })
}

fn run_bridge_with_timeout(
    request: &BridgeRequest<'_>,
    timeout: Option<std::time::Duration>,
) -> Result<BridgeResponse> {
    let bridge_bin = bridge_binary();
    let mut child = Command::new(&bridge_bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "failed to spawn Apple STT bridge '{}'",
                bridge_bin.display()
            )
        })?;

    // Write request to stdin; on error, reap the child to avoid zombie.
    let write_result = (|| -> Result<()> {
        let stdin = child
            .stdin
            .as_mut()
            .context("Apple STT bridge stdin unavailable")?;
        let payload = serde_json::to_vec(request).context("serialize bridge request")?;
        stdin
            .write_all(&payload)
            .context("write bridge request payload")?;
        stdin.write_all(b"\n").context("write bridge request EOL")?;
        Ok(())
    })();
    if let Err(e) = write_result {
        let _ = child.kill();
        let _ = child.wait();
        return Err(e);
    }

    // Drop stdin handle so the bridge sees EOF and can finish.
    drop(child.stdin.take());

    let output = if let Some(dur) = timeout {
        wait_with_timeout(&mut child, dur)?
    } else {
        child
            .wait_with_output()
            .context("wait for Apple STT bridge output")?
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        if detail.is_empty() {
            bail!("Apple STT bridge exited with status {}", output.status);
        }
        bail!(
            "Apple STT bridge exited with status {}: {}",
            output.status,
            detail
        );
    }

    let stdout = String::from_utf8(output.stdout).context("bridge stdout is not UTF-8")?;
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        bail!("Apple STT bridge returned empty response");
    }

    let response: BridgeResponse =
        serde_json::from_str(trimmed).context("parse Apple STT bridge JSON response")?;
    if !response.is_ok() {
        let message = response
            .error
            .unwrap_or_else(|| "bridge returned error without message".to_string());
        bail!("{message}");
    }
    Ok(response)
}

/// Wait for child process with a timeout. Kills+reaps on timeout to avoid zombies.
fn wait_with_timeout(child: &mut std::process::Child, timeout: Duration) -> Result<Output> {
    let pid = child.id();
    let mut stdout_handle = child.stdout.take();
    let mut stderr_handle = child.stderr.take();

    let stdout_thread = std::thread::spawn(move || -> Vec<u8> {
        let mut buf = Vec::new();
        if let Some(ref mut r) = stdout_handle {
            let _ = r.read_to_end(&mut buf);
        }
        buf
    });
    let stderr_thread = std::thread::spawn(move || -> Vec<u8> {
        let mut buf = Vec::new();
        if let Some(ref mut r) = stderr_handle {
            let _ = r.read_to_end(&mut buf);
        }
        buf
    });

    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = stdout_thread.join().unwrap_or_default();
                let stderr = stderr_thread.join().unwrap_or_default();
                return Ok(Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    tracing::warn!(
                        "Apple STT bridge (pid {pid}) timed out after {}s, killing",
                        timeout.as_secs()
                    );
                    let _ = child.kill();
                    let _ = child.wait();
                    bail!("Apple STT bridge timed out after {}s", timeout.as_secs());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(anyhow::anyhow!("wait for bridge pid {pid}: {e}"));
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProbeResult {
    supported: bool,
    installed: bool,
    backend: Option<AppleSttBackend>,
}

/// Pure probe interpretation used by init and unit tests (router selection truth).
fn interpret_probe_response(response: &BridgeResponse) -> ProbeResult {
    ProbeResult {
        supported: response.locale_supported.unwrap_or(false),
        installed: response.locale_installed.unwrap_or(false),
        backend: response
            .backend
            .as_deref()
            .and_then(AppleSttBackend::from_bridge),
    }
}

fn probe_bridge(locale: &str, allow_download: bool) -> Result<ProbeResult> {
    let request = BridgeRequest {
        protocol_version: 1,
        command: "probe",
        locale,
        audio_path: None,
        allow_download,
    };
    let response = run_bridge_with_timeout(&request, Some(BRIDGE_PROBE_TIMEOUT))
        .context("Apple STT bridge probe failed")?;
    Ok(interpret_probe_response(&response))
}

/// Preferred backend label for a locale given a probe snapshot (test seam).
pub fn preferred_backend_for_probe(
    supported: bool,
    installed: bool,
    backend: Option<&str>,
) -> Result<AppleSttBackend> {
    if !supported {
        bail!("locale unsupported by both SpeechTranscriber and SFSpeechRecognizer on-device");
    }
    if !installed {
        bail!("locale supported but assets not installed");
    }
    backend
        .and_then(AppleSttBackend::from_bridge)
        .ok_or_else(|| anyhow::anyhow!("probe reported ready without backend label"))
}

fn bridge_binary() -> PathBuf {
    let current_exe = std::env::current_exe().ok();
    bridge_binary_for_current_exe(current_exe.as_deref())
}

fn bridge_binary_for_current_exe(current_exe: Option<&Path>) -> PathBuf {
    if let Some(override_bin) = bridge_override_binary() {
        return override_bin;
    }

    bundled_bridge_binary_for_exe(current_exe).unwrap_or_else(|| PathBuf::from(DEFAULT_BRIDGE_BIN))
}

fn bridge_override_binary() -> Option<PathBuf> {
    match std::env::var(ENV_STT_BRIDGE) {
        Ok(value) if !value.trim().is_empty() => Some(PathBuf::from(value.trim())),
        _ => None,
    }
}

fn bundled_bridge_binary_for_exe(current_exe: Option<&Path>) -> Option<PathBuf> {
    let executable_dir = current_exe?.parent()?;
    let contents_dir = executable_dir.parent()?;
    let bundle_dir = contents_dir.parent()?;
    if executable_dir.file_name()?.to_str()? != "MacOS" {
        return None;
    }
    if contents_dir.file_name()?.to_str()? != "Contents" {
        return None;
    }
    if bundle_dir.extension()?.to_str()? != "app" {
        return None;
    }

    let candidate = executable_dir.join(DEFAULT_BRIDGE_BIN);
    candidate.is_file().then_some(candidate)
}

/// Cheap, process-cached check that the Apple STT bridge binary can actually be
/// launched: an explicit `CODESCRIBE_APPLE_STT_BRIDGE` path wins first, then a
/// bridge bundled beside the current `.app` executable, then the default bare
/// command name on `PATH`. AUTO engine selection gates on this so it never
/// advertises Apple on a host where the bridge is absent (which wastes a probe
/// and then silently falls back to Candle). Explicit `CODESCRIBE_STT_ENGINE=apple`
/// bypasses this and still probes + fails loudly.
pub(crate) fn is_bridge_resolvable() -> bool {
    static RESOLVABLE: OnceLock<bool> = OnceLock::new();
    *RESOLVABLE.get_or_init(bridge_binary_resolvable)
}

fn bridge_binary_resolvable() -> bool {
    let current_exe = std::env::current_exe().ok();
    bridge_binary_resolvable_for_current_exe(current_exe.as_deref())
}

fn bridge_binary_resolvable_for_current_exe(current_exe: Option<&Path>) -> bool {
    if let Some(override_bin) = bridge_override_binary() {
        return bridge_candidate_resolvable(&override_bin);
    }
    if bundled_bridge_binary_for_exe(current_exe).is_some() {
        return true;
    }

    which_in_path(DEFAULT_BRIDGE_BIN).is_some()
}

fn bridge_candidate_resolvable(candidate: &Path) -> bool {
    let bin = candidate.to_string_lossy();
    if candidate.is_absolute() || bin.contains(std::path::MAIN_SEPARATOR) {
        return candidate.is_file();
    }

    which_in_path(&bin).is_some()
}

fn which_in_path(bin: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(bin))
        .find(|candidate| candidate.is_file())
}

fn ensure_supported_platform() -> Result<()> {
    if !cfg!(target_os = "macos") {
        bail!(
            "Apple SpeechAnalyzer requires macOS; current target OS is {}",
            std::env::consts::OS
        );
    }

    let major = macos_major_version()?;
    if major < MIN_SUPPORTED_MACOS_MAJOR {
        bail!(
            "Apple SpeechAnalyzer requires macOS {}+; detected macOS {}",
            MIN_SUPPORTED_MACOS_MAJOR,
            major
        );
    }

    Ok(())
}

fn macos_major_version() -> Result<u32> {
    static VERSION: OnceLock<std::result::Result<u32, String>> = OnceLock::new();
    let cached =
        VERSION.get_or_init(|| detect_macos_major_version().map_err(|e| format!("{:#}", e)));
    match cached {
        Ok(value) => Ok(*value),
        Err(message) => bail!("{message}"),
    }
}

fn detect_macos_major_version() -> Result<u32> {
    #[cfg(not(target_os = "macos"))]
    {
        bail!("sw_vers unavailable on non-macOS platform");
    }

    #[cfg(target_os = "macos")]
    {
        let output = Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .context("run sw_vers -productVersion")?;
        if !output.status.success() {
            bail!("sw_vers failed with status {}", output.status);
        }
        let version = String::from_utf8(output.stdout).context("sw_vers stdout is not UTF-8")?;
        parse_macos_major_version(&version)
    }
}

fn parse_macos_major_version(version: &str) -> Result<u32> {
    let major_str = version
        .trim()
        .split('.')
        .next()
        .context("missing major version component")?;
    let major = major_str
        .parse::<u32>()
        .with_context(|| format!("invalid macOS major version: '{major_str}'"))?;
    Ok(major)
}

fn resolved_locale(language: Option<&str>) -> String {
    if let Ok(override_locale) = std::env::var(ENV_LOCALE) {
        let trimmed = override_locale.trim();
        if !trimmed.is_empty() {
            return normalize_locale(trimmed);
        }
    }

    match language {
        Some(lang) if !lang.trim().is_empty() => normalize_locale(lang),
        _ => DEFAULT_LOCALE.to_string(),
    }
}

fn normalize_locale(locale: &str) -> String {
    let normalized = locale.trim().replace('_', "-");
    if normalized.is_empty() {
        return DEFAULT_LOCALE.to_string();
    }

    match normalized.to_ascii_lowercase().as_str() {
        "pl" => "pl-PL".to_string(),
        "en" => "en-US".to_string(),
        _ => normalized,
    }
}

fn env_bool(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(value) => parse_bool_flag(&value).unwrap_or(default),
        Err(_) => default,
    }
}

fn parse_bool_flag(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

struct TempWavFile {
    path: PathBuf,
}

impl TempWavFile {
    fn write(samples: &[f32], sample_rate: u32) -> Result<Self> {
        if sample_rate == 0 {
            bail!("sample_rate must be > 0");
        }

        let path =
            std::env::temp_dir().join(format!("codescribe-apple-stt-{}.wav", Uuid::new_v4()));
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec)
            .with_context(|| format!("create {}", path.display()))?;
        for sample in samples {
            let scaled = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
            writer
                .write_sample(scaled)
                .context("write PCM16 sample to temp wav")?;
        }
        writer
            .finalize()
            .with_context(|| format!("finalize {}", path.display()))?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempWavFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    #[serial]
    fn bridge_resolvable_honors_explicit_override_path() {
        let previous = std::env::var(ENV_STT_BRIDGE).ok();

        let present = std::env::temp_dir().join(format!("cs-stt-bridge-{}", Uuid::new_v4()));
        std::fs::write(&present, b"#!/bin/sh\n").expect("write probe bin");
        // SAFETY: serialized by #[serial]; env restored before the test returns.
        unsafe { std::env::set_var(ENV_STT_BRIDGE, &present) };
        assert!(
            bridge_binary_resolvable(),
            "an existing override path must resolve"
        );

        let missing =
            std::env::temp_dir().join(format!("cs-stt-bridge-missing-{}", Uuid::new_v4()));
        unsafe { std::env::set_var(ENV_STT_BRIDGE, &missing) };
        assert!(
            !bridge_binary_resolvable(),
            "a missing override path must be unresolvable"
        );

        // SAFETY: serialized by #[serial].
        unsafe {
            match previous {
                Some(value) => std::env::set_var(ENV_STT_BRIDGE, value),
                None => std::env::remove_var(ENV_STT_BRIDGE),
            }
        }
        let _ = std::fs::remove_file(&present);
    }

    #[test]
    #[serial]
    fn bridge_resolver_prefers_env_then_bundle_then_path() {
        let previous_bridge = std::env::var_os(ENV_STT_BRIDGE);
        let previous_path = std::env::var_os("PATH");
        let root = std::env::temp_dir().join(format!("cs-stt-resolver-{}", Uuid::new_v4()));
        let bundle_macos = root.join("Codescribe.app/Contents/MacOS");
        let exe = bundle_macos.join("Codescribe");
        let bundled = bundle_macos.join(DEFAULT_BRIDGE_BIN);
        let path_dir = root.join("path-bin");
        let path_bridge = path_dir.join(DEFAULT_BRIDGE_BIN);
        let override_bin = root.join("override-bridge");
        let missing_override = root.join("missing-override");

        std::fs::create_dir_all(&bundle_macos).expect("create bundle dir");
        std::fs::create_dir_all(&path_dir).expect("create PATH dir");
        std::fs::write(&exe, b"app").expect("write fake app executable");
        std::fs::write(&bundled, b"#!/bin/sh\n").expect("write bundled bridge");
        std::fs::write(&path_bridge, b"#!/bin/sh\n").expect("write PATH bridge");
        std::fs::write(&override_bin, b"#!/bin/sh\n").expect("write override bridge");

        // SAFETY: serialized by #[serial]; env restored before the test returns.
        unsafe {
            std::env::set_var("PATH", &path_dir);
            std::env::set_var(ENV_STT_BRIDGE, &override_bin);
        }
        assert_eq!(
            bridge_binary_for_current_exe(Some(&exe)),
            override_bin,
            "explicit env override must beat the bundled bridge"
        );
        assert!(
            bridge_binary_resolvable_for_current_exe(Some(&exe)),
            "existing env override path must resolve"
        );

        // SAFETY: serialized by #[serial].
        unsafe { std::env::set_var(ENV_STT_BRIDGE, &missing_override) };
        assert_eq!(
            bridge_binary_for_current_exe(Some(&exe)),
            missing_override,
            "missing env override still wins instead of falling through"
        );
        assert!(
            !bridge_binary_resolvable_for_current_exe(Some(&exe)),
            "missing explicit override must not fall through to bundle or PATH"
        );

        // SAFETY: serialized by #[serial].
        unsafe { std::env::remove_var(ENV_STT_BRIDGE) };
        assert_eq!(
            bridge_binary_for_current_exe(Some(&exe)),
            bundled,
            "bundled bridge must beat PATH when env is unset"
        );
        assert!(
            bridge_binary_resolvable_for_current_exe(Some(&exe)),
            "bundled bridge must resolve"
        );

        std::fs::remove_file(&bundled).expect("remove bundled bridge");
        assert_eq!(
            bridge_binary_for_current_exe(Some(&exe)),
            PathBuf::from(DEFAULT_BRIDGE_BIN),
            "missing bundled bridge must fall through to bare PATH command"
        );
        assert!(
            bridge_binary_resolvable_for_current_exe(Some(&exe)),
            "PATH bridge must resolve after bundled bridge is absent"
        );

        // SAFETY: serialized by #[serial].
        unsafe {
            match previous_bridge {
                Some(value) => std::env::set_var(ENV_STT_BRIDGE, value),
                None => std::env::remove_var(ENV_STT_BRIDGE),
            }
            match previous_path {
                Some(value) => std::env::set_var("PATH", value),
                None => std::env::remove_var("PATH"),
            }
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn adapter_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<AppleSpeechAnalyzerAdapter>();
    }

    #[test]
    fn locale_normalization_for_short_codes() {
        assert_eq!(normalize_locale("pl"), "pl-PL");
        assert_eq!(normalize_locale("en"), "en-US");
    }

    #[test]
    fn locale_normalization_preserves_tags() {
        assert_eq!(normalize_locale("pl-PL"), "pl-PL");
        assert_eq!(normalize_locale("en_GB"), "en-GB");
    }

    #[test]
    fn parse_macos_major_version_standard_formats() {
        assert_eq!(parse_macos_major_version("26.0").unwrap(), 26);
        assert_eq!(parse_macos_major_version("26.1.3\n").unwrap(), 26);
    }

    #[test]
    fn parse_bool_flag_common_values() {
        assert_eq!(parse_bool_flag("1"), Some(true));
        assert_eq!(parse_bool_flag("TRUE"), Some(true));
        assert_eq!(parse_bool_flag("no"), Some(false));
        assert_eq!(parse_bool_flag("0"), Some(false));
        assert_eq!(parse_bool_flag("maybe"), None);
    }

    #[test]
    fn probe_pl_via_sf_speech_is_ready() {
        let response: BridgeResponse = serde_json::from_str(
            r#"{
                "ok": true,
                "status": "ok",
                "text": "",
                "segments": [],
                "locale_supported": true,
                "locale_installed": true,
                "backend": "sf_speech_recognizer"
            }"#,
        )
        .expect("fixture");
        let probe = interpret_probe_response(&response);
        assert!(probe.supported);
        assert!(probe.installed);
        assert_eq!(probe.backend, Some(AppleSttBackend::SfSpeechRecognizer));
        let backend = preferred_backend_for_probe(true, true, Some("sf_speech_recognizer"))
            .expect("pl sf lane");
        assert_eq!(backend, AppleSttBackend::SfSpeechRecognizer);
        assert_eq!(
            backend.engine_mode(),
            TranscriptionEngineMode::SfSpeechOnDevice
        );
    }

    #[test]
    fn probe_en_us_prefers_speech_transcriber() {
        let response: BridgeResponse = serde_json::from_str(
            r#"{
                "ok": true,
                "status": "ok",
                "locale_supported": true,
                "locale_installed": true,
                "backend": "speech_transcriber"
            }"#,
        )
        .expect("fixture");
        let probe = interpret_probe_response(&response);
        assert_eq!(probe.backend, Some(AppleSttBackend::SpeechTranscriber));
        let backend = preferred_backend_for_probe(true, true, Some("speech_transcriber"))
            .expect("en ST lane");
        assert_eq!(backend, AppleSttBackend::SpeechTranscriber);
    }

    #[test]
    fn probe_neither_backend_is_honest_error() {
        let err = preferred_backend_for_probe(false, false, None).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("unsupported"),
            "expected honest unsupported error, got: {msg}"
        );
    }

    #[test]
    fn bridge_response_segments_flow_to_raw_transcript_and_silero_tail_drop() {
        let response: BridgeResponse = serde_json::from_str(
            r#"{
                "ok": true,
                "status": "ok",
                "text": "To jest początek Dziękuję za uwagę",
                "segments": [
                    {"text": "To jest początek", "start_ts": 0.0, "end_ts": 0.4},
                    {"text": "Dziękuję za uwagę", "start_ts": 2.0, "end_ts": 2.4}
                ]
            }"#,
        )
        .expect("fixture bridge response must parse");

        let raw = raw_transcript_from_bridge_response(response);

        assert_eq!(raw.segments.len(), 2);
        assert!(
            raw.segments
                .windows(2)
                .all(|pair| pair[0].end_ts <= pair[1].start_ts),
            "Apple bridge segments must preserve a monotonic timeline"
        );

        let timeline = crate::vad::discriminator::VadTimeline {
            classes: vec![
                crate::pipeline::contracts::VadClass::Speech,
                crate::pipeline::contracts::VadClass::Speech,
                crate::pipeline::contracts::VadClass::TrailingSilence,
                crate::pipeline::contracts::VadClass::TrailingSilence,
                crate::pipeline::contracts::VadClass::TrailingSilence,
            ],
            window_sec: 0.5,
        };
        let vad_config = crate::vad::VadConfig {
            tail_drop_enabled: true,
            ..Default::default()
        };
        let outcome = crate::stt::whisper::map_whisper_segments_to_silero(
            &raw.segments,
            &timeline,
            &vad_config,
        );

        assert_eq!(outcome.dropped_count, 1);
        assert_eq!(outcome.text, "To jest początek");
    }
}
