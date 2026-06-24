//! Apple SpeechAnalyzer adapter (macOS 26+) via Swift bridge subprocess.
//!
//! This module provides a zero-model-size on-device STT backend that maps to
//! the existing `TranscriptionAdapter` contract used by the streaming pipeline.
//! Runtime fallback is handled in `core/stt/mod.rs`.
//!
//! Bridge protocol: JSON request on stdin, JSON response on stdout.

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
    let (supported, installed) = probe_bridge(&locale, allow_download)?;

    if !supported {
        bail!("SpeechAnalyzer does not support locale '{locale}'");
    }
    if !installed {
        bail!(
            "SpeechAnalyzer assets for locale '{locale}' are missing; set {ENV_ALLOW_DOWNLOAD}=1 to auto-install in bridge"
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
    let (samples, sample_rate) =
        crate::audio::load_audio_file(path).with_context(|| format!("load {}", path.display()))?;
    transcribe_long_with_segments(&samples, sample_rate, language)
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

    let segments = response
        .segments
        .into_iter()
        .map(|seg| TranscriptSegment {
            text: seg.text,
            start_ts: seg.start_ts,
            end_ts: seg.end_ts,
        })
        .collect();

    Ok(RawTranscript {
        text: response.text.trim().to_string(),
        segments,
        ..Default::default()
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
        .with_context(|| format!("failed to spawn Apple STT bridge '{bridge_bin}'"))?;

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

fn probe_bridge(locale: &str, allow_download: bool) -> Result<(bool, bool)> {
    let request = BridgeRequest {
        protocol_version: 1,
        command: "probe",
        locale,
        audio_path: None,
        allow_download,
    };
    let response = run_bridge_with_timeout(&request, Some(BRIDGE_PROBE_TIMEOUT))
        .context("Apple STT bridge probe failed")?;
    Ok((
        response.locale_supported.unwrap_or(false),
        response.locale_installed.unwrap_or(false),
    ))
}

fn bridge_binary() -> String {
    match std::env::var(ENV_STT_BRIDGE) {
        Ok(value) if !value.trim().is_empty() => value.trim().to_string(),
        _ => DEFAULT_BRIDGE_BIN.to_string(),
    }
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
}
