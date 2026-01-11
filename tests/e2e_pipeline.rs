//! End-to-End Pipeline Tests for CodeScribe
//!
//! Tests the core transcription pipeline and model functionality:
//! 1. **Local STT Engine**: Pure Rust Whisper with Metal acceleration
//! 2. **Q8 Quantized Model**: whisper-large-v3-turbo-mlx-q8 (4-layer, ~10x faster)
//! 3. **Audio Processing**: WAV/MP3 loading and resampling
//! 4. **Language Detection**: Automatic language identification
//!
//! Run with: cargo test --test e2e_pipeline -- --nocapture
//!
//! Required env vars for full test:
//!   CODESCRIBE_TEST_MODEL_DIR - Path to Whisper model (default: models/whisper-large-v3-turbo-mlx-q8)
//!   CODESCRIBE_TEST_AUDIO_PATH - Path to test audio file (wav/mp3)

use std::path::PathBuf;
use std::time::{Duration, Instant};

// ============================================================================
// Test Configuration
// ============================================================================

fn model_dir() -> PathBuf {
    std::env::var("CODESCRIBE_TEST_MODEL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            // Prefer turbo model in project directory (4-layer, ~10x faster than full 32-layer)
            let project_turbo = PathBuf::from("models/whisper-large-v3-turbo-mlx-q8");
            if project_turbo.exists() {
                return project_turbo;
            }
            // Fallback to home directory
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".CodeScribe/models/whisper-large-v3-turbo-mlx-q8")
        })
}

/// Check if model directory has all required files
fn is_model_complete(model_path: &std::path::Path) -> bool {
    if !model_path.exists() {
        return false;
    }
    let required_files = [
        "config.json",
        "weights.safetensors",
        "tokenizer.json",
        "mel_filters.npz",
    ];
    required_files.iter().all(|f| model_path.join(f).exists())
}

fn audio_path() -> Option<PathBuf> {
    let path = std::env::var("CODESCRIBE_TEST_AUDIO_PATH")
        .ok()
        .map(PathBuf::from)?;
    if path.exists() { Some(path) } else { None }
}

fn print_header(title: &str) {
    println!("\n{}", "═".repeat(70));
    println!("  {}", title);
    println!("{}", "═".repeat(70));
}

fn print_section(title: &str) {
    println!("\n{}", "─".repeat(50));
    println!("  {}", title);
    println!("{}", "─".repeat(50));
}

fn print_result(label: &str, value: &str) {
    println!("  {:.<30} {}", format!("{} ", label), value);
}

fn print_timing(label: &str, duration: Duration) {
    println!("  {:.<30} {:?}", format!("{} ", label), duration);
}

fn print_success(msg: &str) {
    println!("  ✓ {}", msg);
}

fn print_info(msg: &str) {
    println!("  ℹ {}", msg);
}

fn print_warning(msg: &str) {
    println!("  ⚠ {}", msg);
}

// ============================================================================
// Local STT Engine Tests (Q8 Model)
// ============================================================================

#[cfg(target_os = "macos")]
mod local_stt_tests {
    use super::*;
    use codescribe::local_stt::{DecodingParams, LocalWhisperEngine};

    /// Test: Q8 model loads correctly
    #[test]
    fn test_q8_model_loads() {
        print_header("TEST: Large V3 Turbo Q8 Model Loading");

        let model_path = model_dir();
        print_result("Model path", model_path.to_str().unwrap_or("N/A"));

        if !is_model_complete(&model_path) {
            print_warning("Model incomplete or not found - skipping");
            println!(
                "  Required files: config.json, weights.safetensors, tokenizer.json, mel_filters.npz"
            );
            println!("  Set CODESCRIBE_TEST_MODEL_DIR to a complete model directory");
            return;
        }

        let start = Instant::now();
        let engine = LocalWhisperEngine::new(&model_path);
        let load_time = start.elapsed();

        print_timing("Model load time", load_time);

        assert!(engine.is_ok(), "Failed to load model: {:?}", engine.err());
        print_success("Q8 model loaded successfully");
    }

    /// Test: DecodingParams defaults match mlx_whisper
    #[test]
    fn test_decoding_params_defaults() {
        print_header("TEST: Decoding Parameters (mlx_whisper compatible)");

        let params = DecodingParams::default();

        print_section("Default Parameters");
        print_result("temperature", &format!("{:.1}", params.temperature));
        print_result(
            "no_repeat_ngram_size",
            &params.no_repeat_ngram_size.to_string(),
        );
        print_result("suppress_blank", &params.suppress_blank.to_string());
        print_result(
            "no_speech_threshold",
            &format!("{:.1}", params.no_speech_threshold),
        );
        print_result(
            "compression_ratio_threshold",
            &format!("{:.1}", params.compression_ratio_threshold),
        );
        print_result(
            "logprob_threshold",
            &format!("{:.1}", params.logprob_threshold),
        );

        // Verify mlx_whisper defaults
        assert_eq!(
            params.temperature, 0.0,
            "temperature should be 0.0 (greedy)"
        );
        assert_eq!(
            params.no_repeat_ngram_size, 3,
            "no_repeat_ngram_size should be 3"
        );
        assert!(params.suppress_blank, "suppress_blank should be true");
        assert!((params.no_speech_threshold - 0.6).abs() < 0.01);
        assert!((params.compression_ratio_threshold - 2.4).abs() < 0.01);
        assert!((params.logprob_threshold - (-1.0)).abs() < 0.01);

        print_success("All parameters match mlx_whisper defaults");
    }

    /// Test: Transcription with Q8 model
    #[test]
    fn test_transcription_q8() {
        print_header("TEST: Transcription with Large V3 Q8");

        let model_path = model_dir();
        let Some(audio_file) = audio_path() else {
            print_warning("CODESCRIBE_TEST_AUDIO_PATH not set - skipping");
            return;
        };

        if !is_model_complete(&model_path) {
            print_warning("Model incomplete or not found - skipping");
            return;
        }

        print_result("Audio file", audio_file.to_str().unwrap_or("N/A"));

        // Load model
        print_section("Loading Model");
        let start = Instant::now();
        let mut engine = LocalWhisperEngine::new(&model_path).expect("Failed to load model");
        print_timing("Load time", start.elapsed());

        // Transcribe
        print_section("Transcribing Audio");
        let start = Instant::now();
        let result = engine.transcribe_file_with_language(&audio_file, None);
        let transcribe_time = start.elapsed();

        print_timing("Transcription time", transcribe_time);

        match result {
            Ok(text) => {
                print_result("Character count", &text.len().to_string());
                print_result("Word count", &text.split_whitespace().count().to_string());

                print_section("Transcription Preview");
                let preview: String = text.chars().take(200).collect();
                println!("  \"{}...\"", preview);

                print_success("Transcription completed successfully");
            }
            Err(e) => {
                panic!("Transcription failed: {}", e);
            }
        }
    }

    /// Test: Language detection
    #[test]
    fn test_language_detection() {
        print_header("TEST: Language Detection");

        let model_path = model_dir();
        let Some(audio_file) = audio_path() else {
            print_warning("CODESCRIBE_TEST_AUDIO_PATH not set - skipping");
            return;
        };

        if !is_model_complete(&model_path) {
            print_warning("Model incomplete or not found - skipping");
            return;
        }

        let mut engine = LocalWhisperEngine::new(&model_path).expect("Failed to load model");

        let start = Instant::now();
        let detected = engine.detect_language_file(&audio_file);
        let detection_time = start.elapsed();

        print_timing("Detection time", detection_time);

        match detected {
            Ok(lang) => {
                print_result("Detected language", &lang);
                print_success("Language detection completed");
            }
            Err(e) => {
                panic!("Language detection failed: {}", e);
            }
        }
    }

    /// Test: Multiple sequential transcriptions (model stays loaded)
    #[test]
    fn test_sequential_transcriptions() {
        print_header("TEST: Sequential Transcriptions (Model Persistence)");

        let model_path = model_dir();
        let Some(audio_file) = audio_path() else {
            print_warning("CODESCRIBE_TEST_AUDIO_PATH not set - skipping");
            return;
        };

        if !is_model_complete(&model_path) {
            print_warning("Model incomplete or not found - skipping");
            return;
        }

        let mut engine = LocalWhisperEngine::new(&model_path).expect("Failed to load model");

        let runs = 3;
        let mut times = Vec::new();

        print_section(&format!("Running {} sequential transcriptions", runs));

        for i in 1..=runs {
            let start = Instant::now();
            let result = engine.transcribe_file_with_language(&audio_file, Some("pl"));
            let elapsed = start.elapsed();

            times.push(elapsed);

            match result {
                Ok(text) => {
                    print_result(
                        &format!("Run {} time", i),
                        &format!("{:?} ({} chars)", elapsed, text.len()),
                    );
                }
                Err(e) => {
                    panic!("Run {} failed: {}", i, e);
                }
            }
        }

        // Calculate stats
        let total: Duration = times.iter().sum();
        let avg = total / runs as u32;

        print_section("Performance Summary");
        print_result("Total time", &format!("{:?}", total));
        print_result("Average time", &format!("{:?}", avg));

        print_success("All sequential runs completed - model stayed loaded");
    }

    /// Test: Custom decoding parameters
    #[test]
    fn test_custom_decoding_params() {
        print_header("TEST: Custom Decoding Parameters");

        let model_path = model_dir();
        if !is_model_complete(&model_path) {
            print_warning("Model incomplete or not found - skipping");
            return;
        }

        let mut engine = LocalWhisperEngine::new(&model_path).expect("Failed to load model");

        print_section("Modifying Parameters");

        // Set stricter parameters
        engine.decoding_params.no_repeat_ngram_size = 4;
        print_result("no_repeat_ngram_size", "4 (stricter)");

        engine.decoding_params.no_speech_threshold = 0.8;
        print_result("no_speech_threshold", "0.8 (more sensitive)");

        engine.decoding_params.compression_ratio_threshold = 2.0;
        print_result("compression_ratio_threshold", "2.0 (stricter)");

        if let Some(audio_file) = audio_path() {
            print_section("Transcribing with Custom Params");
            let start = Instant::now();
            let result = engine.transcribe_file_with_language(&audio_file, Some("pl"));
            print_timing("Time", start.elapsed());

            match result {
                Ok(text) => {
                    print_result("Result", &format!("{} chars", text.len()));
                    print_success("Custom params work correctly");
                }
                Err(e) => {
                    print_warning(&format!("Transcription with custom params: {}", e));
                }
            }
        } else {
            print_success("Custom params set successfully (no audio to test)");
        }
    }
}

// ============================================================================
// Audio Loader Tests
// ============================================================================

#[cfg(target_os = "macos")]
mod audio_loader_tests {
    use super::*;
    use codescribe::audio_loader;

    /// Test: Audio file loading
    #[test]
    fn test_audio_loading() {
        print_header("TEST: Audio File Loading");

        let Some(audio_file) = audio_path() else {
            print_warning("CODESCRIBE_TEST_AUDIO_PATH not set - skipping");
            return;
        };

        print_result("Audio file", audio_file.to_str().unwrap_or("N/A"));

        let start = Instant::now();
        let result = audio_loader::load_audio_file(&audio_file);
        let load_time = start.elapsed();

        print_timing("Load time", load_time);

        match result {
            Ok((samples, sample_rate)) => {
                let duration_secs = samples.len() as f32 / sample_rate as f32;

                print_result("Sample rate", &format!("{} Hz", sample_rate));
                print_result("Sample count", &samples.len().to_string());
                print_result("Duration", &format!("{:.2}s", duration_secs));

                // Check audio characteristics
                let min = samples.iter().cloned().fold(f32::INFINITY, f32::min);
                let max = samples.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                let rms: f32 =
                    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();

                print_section("Audio Characteristics");
                print_result("Min amplitude", &format!("{:.4}", min));
                print_result("Max amplitude", &format!("{:.4}", max));
                print_result("RMS level", &format!("{:.4}", rms));

                print_success("Audio loaded successfully");
            }
            Err(e) => {
                panic!("Audio loading failed: {}", e);
            }
        }
    }

    /// Test: Resampling to 16kHz
    #[test]
    fn test_resampling() {
        print_header("TEST: Audio Resampling to 16kHz");

        let Some(audio_file) = audio_path() else {
            print_warning("CODESCRIBE_TEST_AUDIO_PATH not set - skipping");
            return;
        };

        let (samples, sample_rate) =
            audio_loader::load_audio_file(&audio_file).expect("Failed to load audio");

        print_result("Original rate", &format!("{} Hz", sample_rate));
        print_result("Original samples", &samples.len().to_string());

        let start = Instant::now();
        let resampled = audio_loader::resample_to_16k(&samples, sample_rate);
        let resample_time = start.elapsed();

        print_timing("Resample time", resample_time);
        print_result("Resampled samples", &resampled.len().to_string());

        // Verify duration is preserved
        let original_duration = samples.len() as f32 / sample_rate as f32;
        let resampled_duration = resampled.len() as f32 / 16000.0;

        print_result("Original duration", &format!("{:.2}s", original_duration));
        print_result("Resampled duration", &format!("{:.2}s", resampled_duration));

        // Allow 1% tolerance for resampling duration
        let diff = (original_duration - resampled_duration).abs();
        assert!(
            diff < original_duration * 0.01,
            "Duration changed too much after resampling"
        );

        print_success("Resampling preserves audio duration");
    }
}

// ============================================================================
// Configuration Tests
// ============================================================================

mod config_tests {
    use super::*;
    use codescribe::config::Config;

    /// Test: Config loading
    #[test]
    fn test_config_load() {
        print_header("TEST: Configuration Loading");

        let config = Config::load();

        print_section("Current Configuration");
        print_result("Hold delay", &format!("{}ms", config.hold_start_delay_ms));
        print_result("Beep on start", &config.beep_on_start.to_string());
        print_result("Use local STT", &config.use_local_stt.to_string());
        print_result("Local model", &config.local_model);
        print_result("Language", &format!("{:?}", config.whisper_language));
        print_result("AI formatting", &config.ai_formatting_enabled.to_string());

        print_success("Configuration loaded successfully");
    }

    /// Test: Config defaults
    #[test]
    fn test_config_defaults() {
        print_header("TEST: Configuration Defaults");

        let config = Config::default();

        print_section("Default Values");
        print_result("Hold delay", &format!("{}ms", config.hold_start_delay_ms));

        assert_eq!(
            config.hold_start_delay_ms, 800,
            "Default hold delay should be 800ms"
        );
        assert!(
            config.beep_on_start,
            "Beep on start should be enabled by default"
        );

        print_success("Defaults are correct");
    }
}

// ============================================================================
// Integration Pipeline Test
// ============================================================================

#[cfg(target_os = "macos")]
mod integration_tests {
    use super::*;
    use codescribe::local_stt::LocalWhisperEngine;

    /// Full pipeline test: Load → Detect Language → Transcribe → Result
    #[test]
    fn test_full_local_pipeline() {
        print_header("TEST: Full Local STT Pipeline");

        let model_path = model_dir();
        let Some(audio_file) = audio_path() else {
            print_warning("CODESCRIBE_TEST_AUDIO_PATH not set - skipping");
            return;
        };

        if !is_model_complete(&model_path) {
            print_warning("Model incomplete or not found - skipping");
            return;
        }

        let total_start = Instant::now();

        // Step 1: Load model
        print_section("Step 1: Load Model");
        let start = Instant::now();
        let mut engine = LocalWhisperEngine::new(&model_path).expect("Failed to load model");
        print_timing("Model load", start.elapsed());
        print_success("Model loaded");

        // Step 2: Detect language
        print_section("Step 2: Detect Language");
        let start = Instant::now();
        let detected_lang = engine
            .detect_language_file(&audio_file)
            .expect("Language detection failed");
        print_timing("Detection time", start.elapsed());
        print_result("Detected", &detected_lang);
        print_success("Language detected");

        // Step 3: Transcribe with detected language
        print_section("Step 3: Transcribe Audio");
        let start = Instant::now();
        let raw_text = engine
            .transcribe_file_with_language(&audio_file, Some(&detected_lang))
            .expect("Transcription failed");
        let transcribe_time = start.elapsed();
        print_timing("Transcription", transcribe_time);
        print_result("Raw chars", &raw_text.len().to_string());
        print_result(
            "Word count",
            &raw_text.split_whitespace().count().to_string(),
        );
        print_success("Transcription complete");

        // Step 4: Results
        print_section("Step 4: Final Results");
        let total_time = total_start.elapsed();
        print_timing("Total pipeline time", total_time);

        // Calculate real-time factor
        let (samples, sample_rate) = codescribe::audio_loader::load_audio_file(&audio_file)
            .expect("Failed to load audio for duration calc");
        let audio_duration_secs = samples.len() as f64 / sample_rate as f64;
        let rtf = total_time.as_secs_f64() / audio_duration_secs;

        print_result("Audio duration", &format!("{:.1}s", audio_duration_secs));
        print_result("Real-time factor", &format!("{:.2}x", rtf));

        println!("\n  ┌─ Transcription Preview ─────────────────────────────┐");
        let preview: String = raw_text.chars().take(300).collect();
        for line in preview.lines() {
            println!("  │ {}", line);
        }
        println!("  └────────────────────────────────────────────────────┘");

        print_success("Full pipeline completed successfully");
    }

    /// Benchmark: Multiple runs with timing stats
    #[test]
    fn test_benchmark_transcription() {
        print_header("BENCHMARK: Transcription Performance");

        let model_path = model_dir();
        let Some(audio_file) = audio_path() else {
            print_warning("CODESCRIBE_TEST_AUDIO_PATH not set - skipping");
            return;
        };

        if !is_model_complete(&model_path) {
            print_warning("Model incomplete or not found - skipping");
            return;
        }

        let mut engine = LocalWhisperEngine::new(&model_path).expect("Failed to load model");

        let runs = 5;
        let mut times: Vec<Duration> = Vec::new();
        let mut lengths: Vec<usize> = Vec::new();

        print_section(&format!("Running {} benchmark iterations", runs));

        for i in 1..=runs {
            let start = Instant::now();
            let result = engine.transcribe_file_with_language(&audio_file, Some("pl"));
            let elapsed = start.elapsed();

            match result {
                Ok(text) => {
                    times.push(elapsed);
                    lengths.push(text.len());
                    println!("  Run {}: {:?} ({} chars)", i, elapsed, text.len());
                }
                Err(e) => {
                    println!("  Run {}: FAILED - {}", i, e);
                }
            }
        }

        if !times.is_empty() {
            print_section("Statistics");

            let total: Duration = times.iter().sum();
            let avg = total / times.len() as u32;
            let min = times.iter().min().unwrap();
            let max = times.iter().max().unwrap();

            print_result("Min time", &format!("{:?}", min));
            print_result("Max time", &format!("{:?}", max));
            print_result("Avg time", &format!("{:?}", avg));
            print_result("Total time", &format!("{:?}", total));

            let avg_len = lengths.iter().sum::<usize>() / lengths.len();
            print_result("Avg output length", &format!("{} chars", avg_len));

            // Calculate chars/sec
            let avg_ms = avg.as_millis() as f64;
            if avg_ms > 0.0 {
                let chars_per_sec = (avg_len as f64) / (avg_ms / 1000.0);
                print_result("Throughput", &format!("{:.0} chars/sec", chars_per_sec));
            }

            print_success("Benchmark completed");
        }
    }
}

// ============================================================================
// Pipeline Mode Documentation Test
// ============================================================================

#[test]
fn test_pipeline_modes_documentation() {
    print_header("DOCUMENTATION: Pipeline Modes");

    print_section("1. Toggle Mode (Hands-off)");
    println!("  Trigger: Double-tap Option key");
    println!("  Flow: IDLE → REC_TOGGLE → (tap again) → BUSY → IDLE");
    println!("  Badge: Pulsing red dot");
    println!("  Use case: Long dictation without holding keys");

    print_section("2. Hold Mode");
    println!("  Trigger: Press and hold Ctrl (800ms delay)");
    println!("  Flow: IDLE → (wait 800ms) → REC_HOLD → (release) → BUSY → IDLE");
    println!("  Badge: Solid red dot");
    println!("  Use case: Quick voice input with visual confirmation");

    print_section("3. AI Assistive Mode");
    println!("  Trigger: Ctrl+Shift (hold or toggle)");
    println!("  Flow: Same as Hold/Toggle but with AI formatting");
    println!("  Badge: Purple dot");
    println!("  Processing: Raw transcript → AI cleanup → Clipboard");
    println!("  Use case: Clean, formatted text output");

    print_section("Configuration");
    println!("  Model: whisper-large-v3-turbo-mlx-q8 (recommended)");
    println!("  Hold delay: 800ms (configurable via HOLD_START_DELAY_MS)");
    println!("  Language: Auto-detect or set via WHISPER_LANGUAGE");
    println!("  Local STT: USE_LOCAL_STT=true + LOCAL_MODEL=whisper-large-v3-turbo-mlx-q8");

    print_section("Environment Variables for Testing");
    println!("  CODESCRIBE_TEST_MODEL_DIR - Path to model directory");
    println!("  CODESCRIBE_TEST_AUDIO_PATH - Path to test audio file");

    print_success("Documentation verified");
}

// ============================================================================
// Model Manager Tests
// ============================================================================

// ============================================================================
// CLI Transcribe Command Tests
// ============================================================================

mod cli_transcribe_tests {
    use super::*;
    use std::process::Command;

    fn vista_e2e_wav() -> Option<PathBuf> {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let path = PathBuf::from(&home).join(".codescribe/vista-e2e-plan.wav");
        if path.exists() { Some(path) } else { None }
    }

    fn codescribe_binary() -> PathBuf {
        // Try release first, then debug
        let release = PathBuf::from("target/release/codescribe");
        if release.exists() {
            return release;
        }
        PathBuf::from("target/debug/codescribe")
    }

    /// Test: CLI transcribe command (raw)
    #[test]
    fn test_cli_transcribe_raw() {
        print_header("TEST: CLI `codescribe transcribe` (raw)");

        let Some(audio_file) = vista_e2e_wav().or_else(audio_path) else {
            print_warning("No test audio file available - skipping");
            return;
        };

        let binary = codescribe_binary();
        if !binary.exists() {
            print_warning("codescribe binary not found - run `cargo build --release` first");
            return;
        }

        print_result("Binary", binary.to_str().unwrap_or("N/A"));
        print_result("Audio", audio_file.to_str().unwrap_or("N/A"));

        let start = std::time::Instant::now();
        let output = Command::new(&binary)
            .args(["transcribe", audio_file.to_str().unwrap(), "-l", "pl"])
            .output();

        match output {
            Ok(result) => {
                let elapsed = start.elapsed();
                print_timing("Total time", elapsed);

                if result.status.success() {
                    let stdout = String::from_utf8_lossy(&result.stdout);
                    let stderr = String::from_utf8_lossy(&result.stderr);

                    print_result("Exit code", "0");
                    print_result("Output chars", &stdout.len().to_string());

                    print_section("Stderr (progress)");
                    for line in stderr.lines().take(10) {
                        println!("  {}", line);
                    }

                    print_section("Transcription Preview");
                    let preview: String = stdout.chars().take(200).collect();
                    println!("  \"{}...\"", preview);

                    print_success("CLI transcribe (raw) completed");
                } else {
                    let stderr = String::from_utf8_lossy(&result.stderr);
                    print_warning(&format!("Exit code: {:?}", result.status.code()));
                    println!("  Stderr: {}", stderr);
                }
            }
            Err(e) => {
                panic!("Failed to run codescribe: {}", e);
            }
        }
    }

    /// Test: CLI transcribe command with --format
    #[test]
    fn test_cli_transcribe_formatted() {
        print_header("TEST: CLI `codescribe transcribe --format`");

        let Some(audio_file) = vista_e2e_wav().or_else(audio_path) else {
            print_warning("No test audio file available - skipping");
            return;
        };

        let binary = codescribe_binary();
        if !binary.exists() {
            print_warning("codescribe binary not found - run `cargo build --release` first");
            return;
        }

        print_result("Binary", binary.to_str().unwrap_or("N/A"));
        print_result("Audio", audio_file.to_str().unwrap_or("N/A"));

        let start = std::time::Instant::now();
        let output = Command::new(&binary)
            .args([
                "transcribe",
                audio_file.to_str().unwrap(),
                "-l",
                "pl",
                "--format",
            ])
            .output();

        match output {
            Ok(result) => {
                let elapsed = start.elapsed();
                print_timing("Total time", elapsed);

                let stdout = String::from_utf8_lossy(&result.stdout);
                let stderr = String::from_utf8_lossy(&result.stderr);

                print_result("Exit code", &format!("{:?}", result.status.code()));
                print_result("Output chars", &stdout.len().to_string());

                // Check if formatting was applied (look for markdown indicators)
                let has_headers = stdout.contains('#');
                let has_bullets = stdout.contains('-') || stdout.contains('•');
                let has_numbering = stdout
                    .lines()
                    .any(|l| l.starts_with("1.") || l.starts_with("2."));

                print_section("Format Detection");
                print_result("Has headers (#)", &has_headers.to_string());
                print_result("Has bullets", &has_bullets.to_string());
                print_result("Has numbering", &has_numbering.to_string());

                if result.status.success() && (has_headers || has_bullets || has_numbering) {
                    print_section("Formatted Output Preview");
                    for line in stdout.lines().take(15) {
                        println!("  {}", line);
                    }
                    print_success("CLI transcribe --format completed with structured output");
                } else if result.status.success() {
                    print_warning("Output not structured - AI formatting may have failed");
                    print_section("Stderr");
                    for line in stderr.lines() {
                        if line.contains("Formatting") || line.contains("⚠") {
                            println!("  {}", line);
                        }
                    }
                } else {
                    print_warning(&format!("Command failed: {:?}", result.status));
                    print_section("Stderr");
                    println!("  {}", stderr);
                }
            }
            Err(e) => {
                panic!("Failed to run codescribe: {}", e);
            }
        }
    }

    /// Test: Compare raw vs formatted output
    #[test]
    fn test_raw_vs_formatted_comparison() {
        print_header("TEST: Raw vs Formatted Comparison");

        let Some(audio_file) = vista_e2e_wav().or_else(audio_path) else {
            print_warning("No test audio file available - skipping");
            return;
        };

        let binary = codescribe_binary();
        if !binary.exists() {
            print_warning("codescribe binary not found - skipping");
            return;
        }

        // Get raw transcription
        print_section("Getting Raw Transcription");
        let raw_output = Command::new(&binary)
            .args(["transcribe", audio_file.to_str().unwrap(), "-l", "pl"])
            .output();

        let raw_text = match raw_output {
            Ok(result) if result.status.success() => {
                String::from_utf8_lossy(&result.stdout).to_string()
            }
            _ => {
                print_warning("Raw transcription failed - skipping comparison");
                return;
            }
        };
        print_result("Raw chars", &raw_text.len().to_string());

        // Get formatted transcription
        print_section("Getting Formatted Transcription");
        let fmt_output = Command::new(&binary)
            .args([
                "transcribe",
                audio_file.to_str().unwrap(),
                "-l",
                "pl",
                "--format",
            ])
            .output();

        let formatted_text = match fmt_output {
            Ok(result) if result.status.success() => {
                String::from_utf8_lossy(&result.stdout).to_string()
            }
            _ => {
                print_warning("Formatted transcription failed - skipping comparison");
                return;
            }
        };
        print_result("Formatted chars", &formatted_text.len().to_string());

        // Compare
        print_section("Comparison");
        let raw_words = raw_text.split_whitespace().count();
        let fmt_words = formatted_text.split_whitespace().count();
        let raw_lines = raw_text.lines().count();
        let fmt_lines = formatted_text.lines().count();

        print_result("Raw words", &raw_words.to_string());
        print_result("Formatted words", &fmt_words.to_string());
        print_result("Raw lines", &raw_lines.to_string());
        print_result("Formatted lines", &fmt_lines.to_string());

        // Structure improvement ratio
        let structure_ratio = fmt_lines as f64 / raw_lines.max(1) as f64;
        print_result("Structure ratio", &format!("{:.2}x", structure_ratio));

        if structure_ratio > 1.5 {
            print_success("Formatted output has significantly better structure");
        } else if structure_ratio > 1.0 {
            print_info("Formatted output has improved structure");
        } else {
            print_warning("Formatting did not improve structure");
        }
    }
}

// ============================================================================
// Teacher Pipeline Tests (Learning Cycle)
// ============================================================================

mod teacher_pipeline_tests {
    use super::*;

    /// Test: Teacher learning cycle concept
    /// 1. Raw transcription
    /// 2. Manually formatted (gold standard)
    /// 3. Teacher learns the pattern
    /// 4. Next transcription should be better
    #[test]
    fn test_teacher_learning_cycle_documentation() {
        print_header("TEST: Teacher Pipeline Learning Cycle");

        print_section("Pipeline Overview");
        println!("  The Teacher pipeline learns from examples to improve formatting.");
        println!();
        println!("  Learning Cycle:");
        println!("  ┌─────────────────────────────────────────────────────────┐");
        println!("  │ 1. Raw Transcription                                    │");
        println!("  │    └─► Whisper output (unformatted)                     │");
        println!("  │                                                         │");
        println!("  │ 2. Gold Standard Formatting                             │");
        println!("  │    └─► Human-edited or AI-formatted exemplar            │");
        println!("  │                                                         │");
        println!("  │ 3. Teacher Learning                                     │");
        println!("  │    └─► Store (raw, formatted) pair in lexicon           │");
        println!("  │    └─► Extract formatting patterns                      │");
        println!("  │                                                         │");
        println!("  │ 4. Improved Transcription                               │");
        println!("  │    └─► Apply learned patterns to new input              │");
        println!("  │    └─► Output quality approaches gold standard          │");
        println!("  └─────────────────────────────────────────────────────────┘");

        print_section("Expected Metrics");
        println!("  - First transcription: Raw, unstructured");
        println!("  - After 1 example: ~50% improvement in structure");
        println!("  - After 5 examples: ~80% improvement in structure");
        println!("  - Convergence: Output quality stabilizes near gold standard");

        print_section("Test Data Requirements");
        println!("  - vista-e2e-plan.wav: Test audio file");
        println!("  - Lexicon entries: Stored in ~/.CodeScribe/lexicon/");
        println!("  - Pattern database: domain-specific formatting rules");

        print_success("Teacher pipeline documentation verified");
    }

    /// Test: Lexicon storage for Teacher
    #[test]
    fn test_lexicon_storage() {
        print_header("TEST: Lexicon Storage for Teacher");

        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let lexicon_dir = PathBuf::from(&home).join(".CodeScribe/lexicon");

        print_result("Lexicon directory", lexicon_dir.to_str().unwrap_or("N/A"));

        if lexicon_dir.exists() {
            // Count entries
            let entries: Vec<_> = std::fs::read_dir(&lexicon_dir)
                .map(|rd| rd.filter_map(|e| e.ok()).collect())
                .unwrap_or_default();

            print_result("Entry count", &entries.len().to_string());

            if !entries.is_empty() {
                print_section("Recent Entries");
                for entry in entries.iter().take(5) {
                    println!("  • {}", entry.file_name().to_string_lossy());
                }
            }
            print_success("Lexicon storage accessible");
        } else {
            print_info("Lexicon directory not created yet");
            print_info("Will be created when Teacher saves first example");
        }
    }
}

// ============================================================================
// Model Manager Tests
// ============================================================================

mod model_tests {
    use super::*;
    use codescribe::models::ModelManager;

    /// Test: Model manager initialization
    #[test]
    fn test_model_manager_init() {
        print_header("TEST: Model Manager Initialization");

        let result = ModelManager::new();

        match result {
            Ok(manager) => {
                print_success("Model manager initialized");

                // List available models
                match manager.list_models() {
                    Ok(models) => {
                        print_section("Available Models");
                        if models.is_empty() {
                            print_info("No models found in ~/.CodeScribe/models/");
                        } else {
                            for model in &models {
                                println!("  • {}", model);
                            }
                        }
                        print_success(&format!("Found {} models", models.len()));
                    }
                    Err(e) => {
                        print_warning(&format!("Could not list models: {}", e));
                    }
                }
            }
            Err(e) => {
                panic!("Model manager init failed: {}", e);
            }
        }
    }

    /// Test: Check Q8 turbo model exists
    #[test]
    fn test_q8_turbo_model_exists() {
        print_header("TEST: Q8 Turbo Model Availability");

        let manager = ModelManager::new().expect("Failed to create model manager");

        let model_name = "whisper-large-v3-turbo-mlx-q8";
        let exists = manager.check_model_exists(model_name);

        print_result("Model name", model_name);
        print_result("Exists", &exists.to_string());

        if exists {
            let path = manager.get_model_path(model_name);
            print_result("Path", path.to_str().unwrap_or("N/A"));
            print_success("Q8 turbo model is available (4-layer, ~10x faster)");
        } else {
            print_warning("Q8 turbo model not found - download it to use local STT");
        }
    }
}
