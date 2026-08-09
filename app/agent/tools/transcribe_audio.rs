//! The `transcribe_audio` agent tool: transcribe a local audio file on request.
//!
//! Registered as [`ToolRisk::ReadOnly`], which makes the path gate the load-
//! bearing part of this module. A model-supplied path is untrusted input, so
//! `validate_audio_path` requires an absolute path to an existing file with a
//! supported extension, canonicalizes it (collapsing `..` and symlinks), and
//! only then checks containment in `~/.codescribe` or the agent assets
//! directory. Canonicalizing *before* the containment check is what stops a
//! symlink inside an allowed directory from reading arbitrary files.
//!
//! Transcription runs through the shared Whisper singleton behind the
//! `TranscribeAudioEngine` trait, so the JSON contract and the path gate are
//! testable without loading a model. Silero VAD pre-filters the audio; silent
//! input returns an empty transcript instead of Whisper's hallucinations.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use codescribe_core::agent::{
    AgentAssetStore, ToolDefinition, ToolRegistry, ToolResultContent, ToolRisk,
};
use codescribe_core::{audio, stt::whisper, vad};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Transcribe an existing local audio file through Codescribe's shared Whisper
/// singleton. Non-embedded builds need the normal runtime model fallback to be
/// available, typically via `CODESCRIBE_MODEL_PATH`.
pub fn register(registry: &mut ToolRegistry) {
    registry
        .register_native(
            transcribe_audio_definition(),
            Box::new(|input| Box::pin(handle_transcribe_audio(input))),
            ToolRisk::ReadOnly,
        )
        .expect("register transcribe_audio tool");
}

/// The tool schema advertised to the model: name, description, and the
/// `path` / `language` input contract.
fn transcribe_audio_definition() -> ToolDefinition {
    ToolDefinition {
        name: "transcribe_audio".to_string(),
        description: "Transcribe an existing audio file by absolute path. Read-only; paths must be inside ~/.codescribe or the agent assets directory. Uses the shared Whisper singleton; set CODESCRIBE_MODEL_PATH when no embedded model is available.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to a WAV/M4A/MP3 audio file under ~/.codescribe or the agent assets directory."
                },
                "language": {
                    "type": "string",
                    "description": "Optional BCP-47/Whisper language hint such as 'pl' or 'en'. Omit to auto-detect."
                }
            },
            "required": ["path"]
        }),
    }
}

/// Tool entry point: run the real Whisper engine and map the outcome to tool
/// result content.
///
/// Errors become [`ToolResultContent::Error`] rather than propagating, so a bad
/// path or a missing model is something the model can read and correct instead
/// of a failed turn.
async fn handle_transcribe_audio(input: Value) -> Vec<ToolResultContent> {
    match transcribe_audio_from_input_with_engine(&input, &WhisperSingleton) {
        Ok(output) => vec![ToolResultContent::Text(output)],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

/// The whole tool body, parameterized over the engine so tests can drive it
/// without Whisper.
///
/// Validates the path, VAD-filters the samples, resolves the language (caller
/// hint wins over detection), transcribes, and serializes
/// `TranscribeAudioOutput`. Audio with no detected speech short-circuits to an
/// empty transcript before language detection or inference is attempted.
fn transcribe_audio_from_input_with_engine(
    input: &Value,
    engine: &dyn TranscribeAudioEngine,
) -> Result<String> {
    let path_str = input
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .context("Missing required non-empty string field 'path'")?;
    let language_hint = input
        .get("language")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    let path = validate_audio_path(path_str)?;

    engine.init().context(
        "Failed to initialize shared Whisper engine. If this build has no embedded model, set CODESCRIBE_MODEL_PATH to a complete Whisper model directory.",
    )?;
    let model_id = engine
        .model_id()
        .context("Failed to resolve active Whisper model id")?;

    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Path is canonicalized and restricted to ~/.codescribe or AgentAssetStore::assets_dir() by validate_audio_path().
    let (samples, sample_rate) = audio::load_audio_file(&path)
        .with_context(|| format!("Failed to load audio from {}", path.display()))?;
    let duration_seconds = samples_duration_seconds(samples.len(), sample_rate);

    let (speech_samples, vad_output) = engine
        .extract_speech(&samples, sample_rate)
        .context("Failed to run Silero VAD pre-filter")?;
    let speech_duration_seconds = samples_duration_seconds(speech_samples.len(), sample_rate);

    if speech_samples.is_empty() {
        return serialize_output(TranscribeAudioOutput {
            path: path.display().to_string(),
            transcript: String::new(),
            model_id,
            detected_language: None,
            language_source: "none".to_string(),
            duration_seconds,
            speech_duration_seconds,
            sample_rate,
            input_samples: samples.len(),
            speech_samples: 0,
            vad: vad_output,
        });
    }

    let (language, language_source) = match language_hint {
        Some(language) => (language, "provided".to_string()),
        None => (
            engine
                .detect_language(&speech_samples, sample_rate)
                .context("Failed to detect audio language")?,
            "detected".to_string(),
        ),
    };

    let transcript = engine
        .transcribe(&speech_samples, sample_rate, &language)
        .context("Failed to transcribe audio with shared Whisper engine")?;

    serialize_output(TranscribeAudioOutput {
        path: path.display().to_string(),
        transcript,
        model_id,
        detected_language: Some(language),
        language_source,
        duration_seconds,
        speech_duration_seconds,
        sample_rate,
        input_samples: samples.len(),
        speech_samples: speech_samples.len(),
        vad: vad_output,
    })
}

/// The STT capabilities this tool needs, extracted as a seam for testing.
///
/// Production uses `WhisperSingleton`; tests substitute a fake so the path
/// gate and the JSON output contract can be exercised without a multi-GB model.
trait TranscribeAudioEngine {
    /// Ensure the engine is loaded, doing nothing if it already is.
    fn init(&self) -> Result<()>;
    /// Identifier of the active model, reported back for provenance.
    fn model_id(&self) -> Result<String>;
    /// Drop non-speech audio, returning the kept samples and the VAD stats.
    fn extract_speech(&self, samples: &[f32], sample_rate: u32) -> Result<(Vec<f32>, VadOutput)>;
    /// Detect the spoken language, used only when the caller gave no hint.
    fn detect_language(&self, samples: &[f32], sample_rate: u32) -> Result<String>;
    /// Transcribe speech samples in a known language.
    fn transcribe(&self, samples: &[f32], sample_rate: u32, language: &str) -> Result<String>;
}

/// Production engine: the process-wide Whisper singleton plus Silero VAD.
struct WhisperSingleton;

impl TranscribeAudioEngine for WhisperSingleton {
    fn init(&self) -> Result<()> {
        if whisper::is_initialized() {
            return Ok(());
        }
        whisper::init()
    }

    fn model_id(&self) -> Result<String> {
        if whisper::embedded::is_embedded_available() {
            return Ok("embedded".to_string());
        }

        let path = whisper::get_model_path()?;
        Ok(path
            .file_name()
            .and_then(|name| name.to_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| path.display().to_string()))
    }

    fn extract_speech(&self, samples: &[f32], sample_rate: u32) -> Result<(Vec<f32>, VadOutput)> {
        let (speech_samples, stats) = vad::extract_speech(samples, sample_rate);
        Ok((speech_samples, VadOutput::from(stats)))
    }

    fn detect_language(&self, samples: &[f32], sample_rate: u32) -> Result<String> {
        whisper::detect_language(samples, sample_rate)
    }

    fn transcribe(&self, samples: &[f32], sample_rate: u32, language: &str) -> Result<String> {
        whisper::transcribe(samples, sample_rate, Some(language))
    }
}

/// Turn an untrusted path string into a canonical path safe to read, or fail.
///
/// Order matters: cheap structural checks (absolute, exists, is a file,
/// supported extension) run first, then canonicalization resolves `..` and
/// symlinks, and only the canonical result is containment-checked. Checking
/// containment before canonicalizing would let a symlink placed inside an
/// allowed directory escape it.
fn validate_audio_path(path_str: &str) -> Result<PathBuf> {
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Input path is validated below (absolute, existing file, supported audio extension, canonicalized, and restricted to ~/.codescribe or AgentAssetStore::assets_dir()) before any filesystem read.
    let path = PathBuf::from(path_str);
    if !path.is_absolute() {
        bail!("Path must be absolute: {path_str}");
    }

    if !path.exists() {
        bail!("Path does not exist: {path_str}");
    }

    if !path.is_file() {
        bail!("Path is not a file: {path_str}");
    }

    ensure_supported_audio_extension(&path)?;

    let canonical = path
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize path: {path_str}"))?;
    ensure_allowed_audio_path(&canonical)?;
    Ok(canonical)
}

/// Reject anything that is not a decodable audio container.
///
/// Case-insensitive; the error names the accepted extensions so the model can
/// correct itself rather than retry blindly.
fn ensure_supported_audio_extension(path: &Path) -> Result<()> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .context("Audio path must include a supported extension (wav, m4a, mp3, flac, ogg)")?;

    match extension.as_str() {
        "wav" | "m4a" | "mp3" | "flac" | "ogg" => Ok(()),
        other => {
            bail!("Unsupported audio extension '{other}'. Expected wav, m4a, mp3, flac, or ogg")
        }
    }
}

/// Confine reads to `~/.codescribe` or the agent assets directory.
///
/// Both roots are canonicalized before comparison so the two sides of the
/// prefix test are in the same form. Expects an already-canonical `path`.
fn ensure_allowed_audio_path(path: &Path) -> Result<()> {
    let home_var = std::env::var("HOME").context("HOME environment variable is not set")?;
    let codescribe_dir = canonical_or_original(PathBuf::from(home_var).join(".codescribe"));
    let assets_dir = canonical_or_original(AgentAssetStore::assets_dir());

    if is_path_allowed(path, &codescribe_dir, &assets_dir) {
        return Ok(());
    }

    bail!(
        "Audio path is outside allowed directories (~/.codescribe or agent assets): {}",
        path.display()
    )
}

/// Canonicalize an allow-list root, keeping the original if it does not exist.
///
/// A missing assets directory must not widen the gate: the non-canonical path is
/// still a prefix no candidate can match by accident.
fn canonical_or_original(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

/// The containment predicate, split out so it is unit-testable against fixed
/// roots without touching the filesystem or `HOME`.
fn is_path_allowed(path: &Path, codescribe_dir: &Path, assets_dir: &Path) -> bool {
    path.starts_with(codescribe_dir) || path.starts_with(assets_dir)
}

/// Sample count as wall-clock seconds, returning `0.0` for a zero sample rate
/// instead of dividing by zero.
fn samples_duration_seconds(sample_count: usize, sample_rate: u32) -> f32 {
    if sample_rate == 0 {
        0.0
    } else {
        sample_count as f32 / sample_rate as f32
    }
}

/// Render the result as pretty JSON — the tool's actual return payload.
fn serialize_output(output: TranscribeAudioOutput) -> Result<String> {
    serde_json::to_string_pretty(&output).context("Failed to serialize transcribe_audio output")
}

/// The JSON the model receives back.
///
/// Deliberately more than the transcript: the model needs to distinguish "the
/// file was silent" from "transcription failed", so duration, speech duration,
/// and the VAD breakdown travel alongside the text. `Deserialize` exists for the
/// tests, which parse the payload back to assert on the contract.
#[derive(Debug, Deserialize, Serialize, PartialEq)]
struct TranscribeAudioOutput {
    /// Canonical path actually read, after validation.
    path: String,
    /// Recognized text; empty when VAD found no speech.
    transcript: String,
    /// Active Whisper model — `"embedded"` or the model directory name.
    model_id: String,
    /// Language used for decoding, absent when nothing was transcribed.
    detected_language: Option<String>,
    /// How the language was chosen: `provided`, `detected`, or `none`.
    language_source: String,
    /// Full audio length in seconds.
    duration_seconds: f32,
    /// Length of the speech kept by VAD, in seconds.
    speech_duration_seconds: f32,
    /// Sample rate of the decoded audio, in Hz.
    sample_rate: u32,
    /// Sample count before VAD filtering.
    input_samples: usize,
    /// Sample count after VAD filtering.
    speech_samples: usize,
    /// VAD statistics explaining what was kept and why.
    vad: VadOutput,
}

/// Serializable projection of the VAD stats, so an empty transcript comes with
/// the evidence for why it is empty.
#[derive(Debug, Deserialize, Serialize, PartialEq)]
struct VadOutput {
    /// Share of analysis windows classified as speech, 0–100.
    speech_pct: f32,
    /// Windows that cleared the speech threshold.
    speech_windows: usize,
    /// Windows analyzed in total.
    total_windows: usize,
    /// Why no speech was found, when that is the outcome.
    no_speech_reason: Option<String>,
}

impl From<vad::VadExtractStats> for VadOutput {
    fn from(stats: vad::VadExtractStats) -> Self {
        Self {
            speech_pct: stats.speech_pct,
            speech_windows: stats.speech_windows,
            total_windows: stats.total_windows,
            no_speech_reason: stats.no_speech_reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    struct FakeEngine;

    impl TranscribeAudioEngine for FakeEngine {
        fn init(&self) -> Result<()> {
            Ok(())
        }

        fn model_id(&self) -> Result<String> {
            Ok("fake-whisper".to_string())
        }

        fn extract_speech(
            &self,
            samples: &[f32],
            _sample_rate: u32,
        ) -> Result<(Vec<f32>, VadOutput)> {
            Ok((
                samples.to_vec(),
                VadOutput {
                    speech_pct: 100.0,
                    speech_windows: 1,
                    total_windows: 1,
                    no_speech_reason: None,
                },
            ))
        }

        fn detect_language(&self, _samples: &[f32], _sample_rate: u32) -> Result<String> {
            Ok("pl".to_string())
        }

        fn transcribe(&self, samples: &[f32], _sample_rate: u32, language: &str) -> Result<String> {
            assert_eq!(language, "pl");
            assert!(!samples.is_empty());
            Ok("ala ma kota".to_string())
        }
    }

    #[test]
    #[serial]
    fn transcribes_allowed_audio_path_with_mock_engine() {
        let assets_dir = AgentAssetStore::assets_dir();
        std::fs::create_dir_all(&assets_dir).expect("create assets dir");
        let audio_path = assets_dir.join(format!(
            "transcribe_audio_fixture_{}.wav",
            std::process::id()
        ));
        write_fixture_wav(&audio_path);

        let output = transcribe_audio_from_input_with_engine(
            &json!({ "path": audio_path.display().to_string() }),
            &FakeEngine,
        )
        .expect("mock transcription should succeed");

        let parsed: TranscribeAudioOutput =
            serde_json::from_str(&output).expect("structured output");
        assert_eq!(parsed.transcript, "ala ma kota");
        assert_eq!(parsed.model_id, "fake-whisper");
        assert_eq!(parsed.detected_language.as_deref(), Some("pl"));
        assert_eq!(parsed.language_source, "detected");
        assert!(parsed.duration_seconds > 0.0);
        assert!(parsed.speech_duration_seconds > 0.0);
        std::fs::remove_file(audio_path).ok();
    }

    #[test]
    fn rejects_audio_paths_outside_allowed_roots() {
        let path = PathBuf::from("/tmp/codescribe-outside.wav");
        std::fs::write(&path, b"not really audio").expect("write outside file");

        let error = validate_audio_path(path.to_str().expect("utf8 path"))
            .expect_err("outside path must be rejected");

        assert!(error.to_string().contains("outside allowed directories"));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn allows_paths_under_codescribe_or_assets_roots() {
        let codescribe_dir = PathBuf::from("/Users/tester/.codescribe");
        let assets_dir = PathBuf::from("/Users/tester/.codescribe/assets");

        assert!(is_path_allowed(
            Path::new("/Users/tester/.codescribe/transcriptions/a.wav"),
            &codescribe_dir,
            &assets_dir,
        ));
        assert!(is_path_allowed(
            Path::new("/Users/tester/.codescribe/assets/a.wav"),
            &codescribe_dir,
            &assets_dir,
        ));
        assert!(!is_path_allowed(
            Path::new("/Users/tester/Desktop/a.wav"),
            &codescribe_dir,
            &assets_dir,
        ));
    }

    fn write_fixture_wav(path: &Path) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).expect("create wav");
        for index in 0..1600 {
            let phase = index as f32 / 16_000.0 * 440.0 * std::f32::consts::TAU;
            let sample = (phase.sin() * i16::MAX as f32 * 0.2) as i16;
            writer.write_sample(sample).expect("write wav sample");
        }
        writer.finalize().expect("finalize wav");
    }
}
