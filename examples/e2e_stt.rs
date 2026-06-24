use anyhow::Result;
use codescribe::whisper::LocalWhisperEngine;
use codescribe_core::pipeline::contracts::FileTranscriptionOptions;
use std::fs;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    // Model path: ~/.codescribe/models/ (unified standard)
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let model_path = PathBuf::from(&home).join(".codescribe/models/whisper-large-v3-turbo-mlx-q8");
    let audio_medium = PathBuf::from(
        "/Users/maciejgad/hosted/vista/api-test-suite/test-files/audio-real-medium.m4a",
    );
    let audio_short = PathBuf::from(
        "/Users/maciejgad/hosted/vista/api-test-suite/test-files/audio-real-short.m4a",
    );

    let language = std::env::var("CODESCRIBE_E2E_LANG").ok();

    println!("Checking model at: {}", model_path.display());

    // Ensure tokenizer.json exists
    let tokenizer_path = model_path.join("tokenizer.json");
    if !tokenizer_path.exists() {
        println!("tokenizer.json missing. Downloading from HF...");
        let url = "https://huggingface.co/openai/whisper-large-v3/resolve/main/tokenizer.json";
        let resp = reqwest::get(url).await?.error_for_status()?;
        let content = resp.bytes().await?;
        fs::write(&tokenizer_path, content)?;
        println!("Downloaded tokenizer.json");
    }

    // Ensure mel_filters.npz exists
    let mel_filters_path = model_path.join("mel_filters.npz");
    if !mel_filters_path.exists() {
        println!("mel_filters.npz missing. Downloading from OpenAI assets...");
        let url =
            "https://raw.githubusercontent.com/openai/whisper/main/whisper/assets/mel_filters.npz";
        let resp = reqwest::get(url).await?.error_for_status()?;
        let content = resp.bytes().await?;
        fs::write(&mel_filters_path, content)?;
        println!("Downloaded mel_filters.npz");
    }

    println!("Initializing engine...");
    let mut engine = LocalWhisperEngine::new(&model_path)?;
    println!("Engine initialized successfully.");

    let run_medium = std::env::var("CODESCRIBE_E2E_RUN_MEDIUM")
        .map(|v| v != "0" && v.to_lowercase() != "false")
        .unwrap_or(true);

    println!("Transcribing short audio: {}", audio_short.display());
    let start = std::time::Instant::now();
    let verdict_short = engine.transcribe_file_with_language(
        &audio_short,
        language.as_deref(),
        FileTranscriptionOptions::default(),
    )?;
    let duration = start.elapsed();
    println!("Short transcription completed in {:?}:", duration);
    println!("---");
    println!("{}", verdict_short.text);
    println!("---");

    if run_medium {
        println!("Transcribing medium audio: {}", audio_medium.display());
        let start = std::time::Instant::now();
        let verdict_medium = engine.transcribe_file_with_language(
            &audio_medium,
            language.as_deref(),
            FileTranscriptionOptions::default(),
        )?;
        let duration = start.elapsed();
        println!("Medium transcription completed in {:?}:", duration);
        println!("---");
        println!("{}", verdict_medium.text);
        println!("---");
    } else {
        println!("Skipping medium transcription (CODESCRIBE_E2E_RUN_MEDIUM disabled)");
    }

    Ok(())
}
