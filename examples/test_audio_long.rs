use anyhow::Result;
use codescribe::whisper::{DecodingParams, LocalWhisperEngine};
use std::path::PathBuf;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() {
        println!(
            "Usage: cargo run --release --example test_audio_long -- [--model PATH] <audio1> ..."
        );
        return Ok(());
    }

    let (model, files): (PathBuf, Vec<PathBuf>) = if args[0] == "--model" {
        (
            PathBuf::from(&args[1]),
            args[2..].iter().map(PathBuf::from).collect(),
        )
    } else {
        (
            PathBuf::from("models/whisper-large-v3-mlx-q8"),
            args.iter().map(PathBuf::from).collect(),
        )
    };

    println!("Loading model: {:?}", model);
    let start_load = std::time::Instant::now();

    // Use custom decoding params for better quality
    let params = DecodingParams::default();
    let mut engine = LocalWhisperEngine::new_with_params(&model, params)?;
    println!("Model loaded in {:?}\n", start_load.elapsed());
    println!("Decoding params: {:?}", engine.decoding_params());

    if files.is_empty() {
        println!("No audio files specified");
        return Ok(());
    }

    for f in files {
        println!("=== {} ===", f.file_name().unwrap().to_string_lossy());

        let (samples, sample_rate) = codescribe::audio_loader::load_audio_file(&f)?;
        let duration_sec = samples.len() as f32 / sample_rate as f32;
        println!("Audio duration: {:.1}s", duration_sec);

        let start = std::time::Instant::now();
        let lang = engine.detect_language(&samples, sample_rate)?;
        println!("Detected: {} ({:?})", lang, start.elapsed());

        let start = std::time::Instant::now();
        let text = engine.transcribe_long_with_language(&samples, sample_rate, Some(&lang))?;
        let elapsed = start.elapsed();
        let rtf = duration_sec / elapsed.as_secs_f32();

        println!("Transcription: {:?} (RTF: {:.1}x)", elapsed, rtf);
        println!("---\n{}\n---\n", text);
    }

    Ok(())
}
