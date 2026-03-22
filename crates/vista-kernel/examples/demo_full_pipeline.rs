//! Full pipeline demo: Transcription + AI Formatting
//!
//! Shows real outputs from:
//! 1. Local Whisper STT (transcription)
//! 2. AI formatting (normal mode)
//! 3. AI assistive mode (kurier/enhancer)
//!
//! Usage:
//!   cargo run --release --example demo_full_pipeline -- <audio_file>
//!   cargo run --release --example demo_full_pipeline -- --assistive <audio_file>
//!
//! Requires:
//!   - Model at ~/.codescribe/models/whisper-large-v3-turbo-mlx-q8 (or set --model)
//!   - LLM_ENDPOINT + LLM_MODEL (or LLM_FORMATTING_* overrides) for formatting

use anyhow::Result;
use qube_stt::stt::whisper::{DecodingParams, LocalWhisperEngine};
use std::path::PathBuf;
use vista_kernel::stream_postprocess::StreamPostProcessor;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() {
        println!("Usage: cargo run --release --example demo_full_pipeline -- [OPTIONS] <audio>");
        println!();
        println!("Options:");
        println!(
            "  --model PATH     Model directory (default: ~/.codescribe/models/whisper-large-v3-turbo-mlx-q8)"
        );
        println!("  --assistive      Use assistive mode (kurier/enhancer) instead of formatting");
        println!("  --raw            Skip AI formatting, show raw transcription only");
        println!();
        println!("Environment:");
        println!("  LLM_ENDPOINT         LLM endpoint URL (e.g., http://localhost:11434/api/chat)");
        println!("  LLM_MODEL            Model name (e.g., qwen3-coder:480b-cloud)");
        println!("  LLM_FORMATTING_*     Optional overrides for formatting");
        return Ok(());
    }

    // Parse args
    // Model path: ~/.codescribe/models/ (unified standard)
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let mut model = PathBuf::from(&home).join(".codescribe/models/whisper-large-v3-turbo-mlx-q8");
    let mut assistive = false;
    let mut raw_only = false;
    let mut audio_file: Option<PathBuf> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--model" => {
                i += 1;
                model = PathBuf::from(&args[i]);
            }
            "--assistive" => assistive = true,
            "--raw" => raw_only = true,
            _ => audio_file = Some(PathBuf::from(&args[i])),
        }
        i += 1;
    }

    let audio_file = audio_file.ok_or_else(|| anyhow::anyhow!("No audio file specified"))?;

    println!("═══════════════════════════════════════════════════════════");
    println!("  CodeScribe Full Pipeline Demo");
    println!("═══════════════════════════════════════════════════════════");
    println!("  Audio: {}", audio_file.display());
    println!("  Model: {}", model.display());
    println!(
        "  Mode:  {}",
        if assistive {
            "ASSISTIVE (kurier)"
        } else {
            "FORMATTING"
        }
    );
    println!("───────────────────────────────────────────────────────────");

    // 1. Load model
    println!("\n[1/3] Loading Whisper model...");
    let start = std::time::Instant::now();
    let params = DecodingParams::default();
    let mut engine = LocalWhisperEngine::new_with_params(&model, params)?;
    println!("      Model loaded in {:?}", start.elapsed());
    println!("      Params: {:?}", engine.decoding_params());

    // 2. Transcribe
    println!("\n[2/3] Transcribing audio...");
    let start = std::time::Instant::now();
    let (samples, sample_rate) = vista_kernel::audio::load_audio_file(&audio_file)?;
    let duration_sec = samples.len() as f32 / sample_rate as f32;
    println!("      Audio duration: {:.1}s", duration_sec);

    // By-passing VAD because it aggressively drops the quiet "No to dobra" start and trailing "Toolchain 2024".
    // For file ingestion, we want 100% audio context to achieve human reference.
    let speech_samples = samples.clone();

    let lang = engine.detect_language(&speech_samples, sample_rate)?;
    println!("      Detected language: {}", lang);

    let mut raw_text =
        engine.transcribe_long_with_language(&speech_samples, sample_rate, Some(&lang))?;

    let mut postprocessor = StreamPostProcessor::new();
    if let Some(processed) = postprocessor.process_utterance(&raw_text) {
        raw_text = processed;
    }
    let transcribe_time = start.elapsed();
    let rtf = duration_sec / transcribe_time.as_secs_f32();
    println!(
        "      Transcription time: {:?} (RTF: {:.1}x)",
        transcribe_time, rtf
    );
    println!(
        "      Raw chars: {}, words: {}",
        raw_text.len(),
        raw_text.split_whitespace().count()
    );

    println!("\n───────────────────────────────────────────────────────────");
    println!("  RAW TRANSCRIPTION:");
    println!("───────────────────────────────────────────────────────────");
    // Show first 500 chars
    let preview = if raw_text.len() > 500 {
        format!(
            "{}...\n[truncated, {} more chars]",
            &raw_text[..500],
            raw_text.len() - 500
        )
    } else {
        raw_text.clone()
    };
    println!("{}", preview);

    if raw_only {
        println!("\n═══════════════════════════════════════════════════════════");
        return Ok(());
    }

    // 3. AI Formatting
    println!(
        "\n[3/3] AI {} ...",
        if assistive { "ASSISTIVE" } else { "FORMATTING" }
    );

    // Check env vars
    let llm_endpoint = std::env::var("LLM_FORMATTING_ENDPOINT")
        .ok()
        .or_else(|| std::env::var("LLM_ENDPOINT").ok());
    let llm_model = std::env::var("LLM_FORMATTING_MODEL")
        .ok()
        .or_else(|| std::env::var("LLM_MODEL").ok());

    if llm_endpoint.is_none() || llm_model.is_none() {
        println!("      SKIPPED - LLM_ENDPOINT and/or LLM_MODEL not set");
        println!("\n═══════════════════════════════════════════════════════════");
        return Ok(());
    }

    println!("      LLM_ENDPOINT: {}", llm_endpoint.as_ref().unwrap());
    println!("      LLM_MODEL: {}", llm_model.as_ref().unwrap());

    let start = std::time::Instant::now();
    let formatted: String =
        vista_kernel::ai_formatting::format_text(&raw_text, Some(&lang), assistive).await;
    let format_time = start.elapsed();
    println!("      Format time: {:?}", format_time);
    println!("      Formatted chars: {}", formatted.len());

    println!("\n───────────────────────────────────────────────────────────");
    println!(
        "  {} OUTPUT:",
        if assistive { "ASSISTIVE" } else { "FORMATTED" }
    );
    println!("───────────────────────────────────────────────────────────");
    // Show first 500 chars
    let preview = if formatted.len() > 500 {
        format!(
            "{}...\n[truncated, {} more chars]",
            &formatted[..500],
            formatted.len() - 500
        )
    } else {
        formatted.clone()
    };
    println!("{}", preview);

    println!("\n═══════════════════════════════════════════════════════════");
    println!("  SUMMARY:");
    println!(
        "  - Transcribe: {:?} ({} chars)",
        transcribe_time,
        raw_text.len()
    );
    println!(
        "  - Format:     {:?} ({} chars)",
        format_time,
        formatted.len()
    );
    println!(
        "  - Delta:      {} chars",
        formatted.len() as i64 - raw_text.len() as i64
    );
    println!("═══════════════════════════════════════════════════════════");

    Ok(())
}
