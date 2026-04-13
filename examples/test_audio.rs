use anyhow::Result;
use codescribe::whisper::LocalWhisperEngine;
use codescribe_core::pipeline::contracts::FileTranscriptionOptions;
use std::path::PathBuf;

fn main() -> Result<()> {
    // Model path: ~/.codescribe/models/ (unified standard)
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let model = PathBuf::from(&home).join(".codescribe/models/whisper-large-v3-turbo-mlx-q8");
    println!("Loading model...");
    let mut engine = LocalWhisperEngine::new(&model)?;
    println!("Model loaded.\n");

    let files: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();

    if files.is_empty() {
        println!("Usage: cargo run --release --example test_audio -- <audio1> <audio2> ...");
        return Ok(());
    }

    for f in files {
        println!("=== {} ===", f.file_name().unwrap().to_string_lossy());

        let start = std::time::Instant::now();
        let lang = engine.detect_language_file(&f)?;
        println!("Detected: {}", lang);

        let verdict =
            engine.transcribe_file_with_language(&f, None, FileTranscriptionOptions::default())?;
        println!("Time: {:?}", start.elapsed());
        println!("---\n{}\n---\n", verdict.text);
    }

    Ok(())
}
