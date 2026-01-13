use anyhow::Result;
use codescribe::whisper::LocalWhisperEngine;
use std::path::PathBuf;

fn main() -> Result<()> {
    let model = PathBuf::from("models/whisper-large-v3-mlx-q8");
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

        let text = engine.transcribe_file_with_language(&f, None)?;
        println!("Time: {:?}", start.elapsed());
        println!("---\n{}\n---\n", text);
    }

    Ok(())
}
