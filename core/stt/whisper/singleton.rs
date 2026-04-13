//! Global Whisper engine singleton with runtime-managed model lookup.
//!
//! Whisper embedding is currently disabled by build policy. The shipped runtime
//! resolves the model from `CODESCRIBE_MODEL_PATH`, configured model dirs,
//! bundled app resources, or the Hugging Face cache.
//!
//! Optional embedded hooks remain only for experimental/test builds.
//!
//! Created by M&K (c)2026 VetCoders

// This entire module is a public API for library consumers

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result, anyhow};
use tracing::{info, warn};

use crate::config::Config;
use crate::config::models::ModelManager;
use crate::hf_cache;
use crate::pipeline::contracts::{
    FileTranscriptionOptions, FinalPassDisposition, FinalPassMode, FinalPassVerdict, RawTranscript,
    TranscriptionSource, TranscriptionVerdict, VadVerdict,
};
use crate::pipeline::stream_postprocess::StreamPostProcessor;

use super::engine::LocalWhisperEngine;
use super::params::DecodingParams;

/// Default model name (for dev/fallback mode)
pub const DEFAULT_MODEL: &str = "whisper-large-v3-turbo-mlx-q8";
const DEFAULT_WHISPER_REPO: &str = "LibraxisAI/whisper-large-v3-turbo-mlx-q8";

/// Global singleton engine
static ENGINE: OnceLock<Mutex<LocalWhisperEngine>> = OnceLock::new();

/// Model path - used by the current runtime-managed Whisper path.
static MODEL_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Resolve the model path for runtime Whisper loading.
fn resolve_model_path_fallback() -> Result<PathBuf> {
    // 1. Dev override
    if let Ok(path) = std::env::var("CODESCRIBE_MODEL_PATH") {
        let p = PathBuf::from(&path);
        if p.join("tokenizer.json").exists() {
            info!(
                "DEV: Using model from CODESCRIBE_MODEL_PATH: {}",
                p.display()
            );
            return Ok(p);
        }
        warn!("CODESCRIBE_MODEL_PATH set but model incomplete: {}", path);
    }

    // 2. Configured model (LOCAL_MODEL)
    let config = Config::load();
    let configured_model = config.local_model;
    if !configured_model.trim().is_empty() {
        if configured_model.contains('/') {
            if let Some(snapshot) = hf_cache::find_snapshot_with_any(
                configured_model.trim(),
                &["config.json", "tokenizer.json", "mel_filters.npz"],
                &["weights.safetensors", "model.safetensors"],
            ) {
                info!("Using HF cache model: {}", snapshot.display());
                return Ok(snapshot);
            }
        } else if configured_model == DEFAULT_MODEL
            && let Some(snapshot) = hf_cache::find_snapshot_with_any(
                DEFAULT_WHISPER_REPO,
                &["config.json", "tokenizer.json", "mel_filters.npz"],
                &["weights.safetensors", "model.safetensors"],
            )
        {
            info!("Using HF cache model: {}", snapshot.display());
            return Ok(snapshot);
        }
    }
    if !configured_model.trim().is_empty()
        && let Ok(manager) = ModelManager::new()
    {
        let candidate = manager.get_model_path(&configured_model);
        if candidate.join("tokenizer.json").exists() {
            info!("Using configured model: {}", candidate.display());
            return Ok(candidate);
        }
    }

    // 3. Bundled .app fallback (Tauri builds without embedding)
    let exe = std::env::current_exe().context("Failed to get executable path")?;
    let exe_dir = exe.parent().context("Failed to get executable directory")?;

    let bundled_path = exe_dir.join("../Resources/models").join(DEFAULT_MODEL);

    if bundled_path.join("tokenizer.json").exists() {
        let canonical = bundled_path.canonicalize().unwrap_or(bundled_path);
        info!("Using bundled model: {}", canonical.display());
        return Ok(canonical);
    }

    Err(anyhow!(
        "Whisper model not available.\n\
         Current builds use runtime model loading.\n\
         Set CODESCRIBE_MODEL_PATH or warm the Hugging Face cache.\n\n\
         Download with: hf download {}",
        DEFAULT_WHISPER_REPO
    ))
}

/// Get the resolved model path used by runtime Whisper loading.
pub fn get_model_path() -> Result<&'static PathBuf> {
    if let Some(path) = MODEL_PATH.get() {
        return Ok(path);
    }

    let path = resolve_model_path_fallback()?;
    let _ = MODEL_PATH.set(path.clone());

    MODEL_PATH
        .get()
        .ok_or_else(|| anyhow!("Failed to store model path"))
}

/// Initialize the global engine (call once at startup).
///
/// Experimental embedded bytes still take precedence when compiled in, but the
/// shipped product path is runtime model resolution.
pub fn init() -> Result<()> {
    // 1. Optional embedded model path for experimental builds/tests.
    if let Some(embedded) = super::embedded::get_embedded_data() {
        let engine = LocalWhisperEngine::from_embedded(&embedded)
            .context("Failed to initialize from embedded model")?;

        ENGINE
            .set(Mutex::new(engine))
            .map_err(|_| anyhow!("Engine already initialized"))?;

        info!("Whisper engine initialized from embedded model (zero I/O)");
        return Ok(());
    }

    // 2. Primary shipped path: runtime model lookup.
    let path = get_model_path()?;
    let engine = LocalWhisperEngine::new_with_params(path, DecodingParams::default())
        .context("Failed to initialize Whisper engine from path")?;

    ENGINE
        .set(Mutex::new(engine))
        .map_err(|_| anyhow!("Engine already initialized"))?;

    info!("Whisper engine initialized from path: {}", path.display());
    Ok(())
}

/// Check if engine is initialized
pub fn is_initialized() -> bool {
    ENGINE.get().is_some()
}

/// Get the global engine (initializes on first call if needed)
pub fn engine() -> Result<&'static Mutex<LocalWhisperEngine>> {
    if !is_initialized() {
        init()?;
    }
    ENGINE
        .get()
        .ok_or_else(|| anyhow!("Engine not initialized"))
}

/// Transcribe audio samples using the global engine
pub fn transcribe(samples: &[f32], sample_rate: u32, language: Option<&str>) -> Result<String> {
    Ok(transcribe_with_segments(samples, sample_rate, language)?.text)
}

/// Transcribe audio samples with segment-level timestamps.
pub fn transcribe_with_segments(
    samples: &[f32],
    sample_rate: u32,
    language: Option<&str>,
) -> Result<RawTranscript> {
    let engine_mutex = engine()?;
    let mut engine = engine_mutex
        .lock()
        .map_err(|e| anyhow!("Failed to lock engine: {}", e))?;

    engine.transcribe_long_with_language_segments(samples, sample_rate, language)
}

/// Transcribe with streaming callback
pub fn transcribe_streaming<'a>(
    samples: &[f32],
    sample_rate: u32,
    language: Option<&str>,
    callback: Option<super::engine::ChunkCallback<'a>>,
) -> Result<String> {
    let engine_mutex = engine()?;
    let mut engine = engine_mutex
        .lock()
        .map_err(|e| anyhow!("Failed to lock engine: {}", e))?;

    engine.transcribe_long_streaming(samples, sample_rate, language, callback)
}

fn skipped_final_pass(options: FileTranscriptionOptions, reason: &str) -> Option<FinalPassVerdict> {
    match options.final_pass {
        FinalPassMode::None => None,
        mode => Some(FinalPassVerdict {
            mode,
            disposition: FinalPassDisposition::Skipped,
            reason: Some(reason.to_string()),
            lexicon_rewrites: 0,
            repetition_cleanups: 0,
        }),
    }
}

fn apply_requested_final_pass(
    raw: &RawTranscript,
    options: FileTranscriptionOptions,
) -> (String, Option<FinalPassVerdict>) {
    match options.final_pass {
        FinalPassMode::None => (raw.text.clone(), None),
        FinalPassMode::EmbeddedLexiconCleanup => {
            let mut processor = StreamPostProcessor::new();
            match processor.process_utterance(&raw.text) {
                Some(text) => {
                    let stats = processor.stats();
                    let disposition = if text == raw.text {
                        FinalPassDisposition::Unchanged
                    } else {
                        FinalPassDisposition::Changed
                    };

                    (
                        text,
                        Some(FinalPassVerdict {
                            mode: FinalPassMode::EmbeddedLexiconCleanup,
                            disposition,
                            reason: None,
                            lexicon_rewrites: stats.lexicon_rewrites,
                            repetition_cleanups: stats.repetition_cleanups,
                        }),
                    )
                }
                None => {
                    let stats = processor.stats();
                    (
                        String::new(),
                        Some(FinalPassVerdict {
                            mode: FinalPassMode::EmbeddedLexiconCleanup,
                            disposition: FinalPassDisposition::Dropped,
                            reason: Some("empty_after_cleanup".to_string()),
                            lexicon_rewrites: stats.lexicon_rewrites,
                            repetition_cleanups: stats.repetition_cleanups,
                        }),
                    )
                }
            }
        }
    }
}

/// Transcribe a file with full structured verdict (VAD stats, confidence, provenance).
pub fn transcribe_file_verdict(
    path: &std::path::Path,
    language: Option<&str>,
    options: FileTranscriptionOptions,
) -> Result<TranscriptionVerdict> {
    let (samples, sample_rate) =
        crate::audio::load_audio_file(path).context("Failed to load audio file")?;

    let (speech_samples, stats) = crate::vad::extract_speech(&samples, sample_rate);
    let total_sec = samples.len() as f32 / sample_rate as f32;
    let speech_sec = speech_samples.len() as f32 / sample_rate as f32;
    info!(
        "transcribe_file VAD: {:.1}s speech / {:.1}s total ({:.0}% speech)",
        speech_sec, total_sec, stats.speech_pct
    );

    let no_speech = speech_samples.is_empty();
    let vad = VadVerdict {
        speech_pct: stats.speech_pct,
        speech_windows: stats.speech_windows,
        total_windows: stats.total_windows,
        no_speech,
        no_speech_reason: stats.no_speech_reason.clone(),
    };

    if no_speech {
        info!("transcribe_file: no speech detected after VAD; returning empty verdict");
        return Ok(TranscriptionVerdict::from_parts(
            String::new(),
            RawTranscript::default(),
            Some(vad),
            TranscriptionSource::LocalFinalPass,
            skipped_final_pass(
                options,
                stats
                    .no_speech_reason
                    .as_deref()
                    .unwrap_or("vad_no_speech_detected"),
            ),
        ));
    }

    let raw = transcribe_with_segments(&speech_samples, sample_rate, language)?;
    let (text, final_pass) = apply_requested_final_pass(&raw, options);

    Ok(TranscriptionVerdict::from_parts(
        text,
        raw,
        Some(vad),
        TranscriptionSource::LocalFinalPass,
        final_pass,
    ))
}

/// Transcribe a file — backward-compatible wrapper returning plain text.
pub fn transcribe_file(path: &std::path::Path, language: Option<&str>) -> Result<String> {
    Ok(transcribe_file_verdict(path, language, FileTranscriptionOptions::default())?.text)
}

/// Detect language from audio samples
pub fn detect_language(samples: &[f32], sample_rate: u32) -> Result<String> {
    let engine_mutex = engine()?;
    let mut engine = engine_mutex
        .lock()
        .map_err(|e| anyhow!("Failed to lock engine: {}", e))?;

    engine.detect_language(samples, sample_rate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn requested_final_pass_reports_embedded_lexicon_changes() {
        let raw = RawTranscript {
            text: "doker".to_string(),
            ..Default::default()
        };

        let (text, final_pass) = apply_requested_final_pass(
            &raw,
            FileTranscriptionOptions {
                final_pass: FinalPassMode::EmbeddedLexiconCleanup,
            },
        );

        assert_eq!(text, "Docker");
        let final_pass = final_pass.expect("expected final-pass provenance");
        assert_eq!(final_pass.mode, FinalPassMode::EmbeddedLexiconCleanup);
        assert_eq!(final_pass.disposition, FinalPassDisposition::Changed);
        assert_eq!(final_pass.lexicon_rewrites, 1);
    }

    #[test]
    fn requested_final_pass_skips_when_no_speech_already_known() {
        let final_pass = skipped_final_pass(
            FileTranscriptionOptions {
                final_pass: FinalPassMode::EmbeddedLexiconCleanup,
            },
            "vad_no_speech_detected",
        )
        .expect("expected skipped final-pass provenance");

        assert_eq!(final_pass.disposition, FinalPassDisposition::Skipped);
        assert_eq!(final_pass.reason.as_deref(), Some("vad_no_speech_detected"));
    }

    #[test]
    #[serial]
    fn test_model_path_resolution_and_real_whisper_noop_load() {
        let path = match resolve_model_path_fallback() {
            Ok(path) => path,
            Err(err) => {
                println!("No model found (expected in CI): {err:?}");
                return;
            }
        };

        assert!(path.join("tokenizer.json").exists());
        println!("Found model at: {}", path.display());

        // This is the real contract we care about in core tests:
        // if the runtime can resolve a model, Whisper must actually load and
        // survive a no-op transcription path without mocking the engine.
        let text = transcribe(&[], 16_000, Some("pl")).expect("Whisper no-op load should work");
        assert!(
            text.is_empty(),
            "empty input should stay empty after no-op load"
        );
    }
}
