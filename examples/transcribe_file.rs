//! Quick transcription utility for large audio files
//!
//! Usage: cargo run --release --example transcribe_file -- /path/to/audio.wav

use codescribe::whisper::LocalWhisperEngine;
use codescribe_core::pipeline::contracts::FileTranscriptionOptions;
use std::env;
use std::path::PathBuf;
use std::time::Instant;

/// Transcribe one file, with the language either given or detected.
///
/// Progress and timings go to stderr while only the transcript goes to stdout,
/// so the output stays pipeable into a file or another tool.
fn main() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: {} <audio_file> [language]", args[0]);
        eprintln!("  language: optional, e.g. 'pl', 'en' (default: auto-detect)");
        std::process::exit(1);
    }

    let audio_path = PathBuf::from(&args[1]);
    let language = args.get(2).map(|s| s.as_str());

    if !audio_path.exists() {
        eprintln!("Error: File not found: {}", audio_path.display());
        std::process::exit(1);
    }

    // Find model: CODESCRIBE_MODEL_PATH first — a measurement instrument that
    // ignores the product's override silently benchmarks the default model
    // against itself (this is exactly what happened to the 2026-08-09 F16-vs-q8
    // comparison). Fallback: ~/.codescribe/models/ (unified standard).
    let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let model_candidates = [
        env::var("CODESCRIBE_MODEL_PATH").ok().map(PathBuf::from),
        Some(PathBuf::from(&home).join(".codescribe/models/whisper-large-v3-turbo")),
        Some(PathBuf::from(&home).join(".codescribe/models/whisper-large-v3-turbo-mlx-q8")),
    ];

    let model_path = model_candidates
        .iter()
        .flatten()
        .find(|p| p.join("tokenizer.json").exists())
        .expect(
            "No complete model found. Need tokenizer.json in the model directory \
             (checked $CODESCRIBE_MODEL_PATH, then ~/.codescribe/models/).",
        );

    eprintln!("═══════════════════════════════════════════════════════════");
    eprintln!("  Local Whisper Transcription");
    eprintln!("═══════════════════════════════════════════════════════════");
    eprintln!("  Audio: {}", audio_path.display());
    eprintln!("  Model: {}", model_path.display());
    eprintln!("  Language: {}", language.unwrap_or("auto-detect"));
    eprintln!("───────────────────────────────────────────────────────────");

    // Load model
    eprintln!("  Loading model...");
    let start = Instant::now();
    let mut engine = LocalWhisperEngine::new(model_path)?;
    eprintln!("  Model loaded in {:?}", start.elapsed());

    // Detect language if not specified
    let lang = if let Some(l) = language {
        l.to_string()
    } else {
        eprintln!("  Detecting language...");
        let start = Instant::now();
        let detected = engine.detect_language_file(&audio_path)?;
        eprintln!("  Detected: {} ({:?})", detected, start.elapsed());
        detected
    };

    // Transcribe
    eprintln!("  Transcribing...");
    let start = Instant::now();
    let verdict = engine.transcribe_file_with_language(
        &audio_path,
        Some(&lang),
        FileTranscriptionOptions::default(),
    )?;
    let elapsed = start.elapsed();
    let text = verdict.text;

    eprintln!("───────────────────────────────────────────────────────────");
    eprintln!("  Transcription time: {:?}", elapsed);
    eprintln!("  Characters: {}", text.len());
    eprintln!("  Words: {}", text.split_whitespace().count());
    eprintln!("═══════════════════════════════════════════════════════════");
    eprintln!();

    // Output transcription to stdout
    println!("{}", text);

    Ok(())
}
