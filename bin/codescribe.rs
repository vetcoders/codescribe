//! codescribe CLI — file transcription that speaks the SAME pipeline as the GUI.
//!
//! WHY THIS EXISTS AGAIN. The previously installed `codescribe` binary was
//! v0.11.0 (built 2026-06-08) and the crate then dropped its default bin
//! entirely, so `make install` silently stopped replacing it — every
//! `codescribe transcribe` since ran a pre-layered pipeline two months behind
//! the product (operator, 2026-08-09: "dostosuj tryby transcribe do tego czym
//! rzeczywiście gada codescribe na GUI"). This bin routes through the exact
//! stages a GUI delivery does, in the same order:
//!
//!   Whisper file final  → lexicon post-process → Light+ → (optional AI format)
//!   `transcribe_file_verdict`  `StreamPostProcessor`  `light_plus::apply`
//!
//! Two faces, matching the GUI's two faces:
//! - default        = the DELIVERY: one shaped transcript on stdout.
//! - `--stream`     = the LIVE CANVAS view: per-segment text flushed to stdout
//!                    as decoding progresses through the file (lexicon applied
//!                    per segment; Light+ belongs to delivery, not the canvas).
//! - `--raw`        = the Ctrl-hold contract: literal words, no Light+.
//! - `-f/--format`  = the AI-formatted lane (same `ai_formatting` call and
//!                    lane config the GUI uses; requires a configured key).
//!
//! Provenance goes to stderr, GUI-truth style, so stdout stays pipeable.
//! The old `daemon` mode is gone on purpose: the SwiftUI app owns runtime.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "codescribe",
    version,
    about = "Local speech-to-text — the same pipeline the codescribe app delivers"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Transcribe an audio file (wav/mp3/m4a) through the product pipeline
    Transcribe {
        /// Path to the audio file
        file: std::path::PathBuf,
        /// Language code (e.g. pl, en). Default: auto-detect
        #[arg(short, long)]
        language: Option<String>,
        /// Live-canvas view: flush each decoded segment as it lands
        #[arg(long)]
        stream: bool,
        /// Literal words — skip the Light+ deterministic shaping (Ctrl-hold contract)
        #[arg(long)]
        raw: bool,
        /// AI formatting via the configured formatting lane (same as the GUI)
        #[arg(short, long)]
        format: bool,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Transcribe {
            file,
            language,
            stream,
            raw,
            format,
        } => transcribe(&file, language.as_deref(), stream, raw, format),
    }
}

fn transcribe(
    file: &std::path::Path,
    language: Option<&str>,
    stream: bool,
    raw: bool,
    format: bool,
) -> anyhow::Result<()> {
    use std::io::Write as _;

    anyhow::ensure!(file.exists(), "file not found: {}", file.display());

    let started = std::time::Instant::now();
    // The one legal file route — identical to the GUI's stop-path final pass.
    let verdict = codescribe_core::stt::transcribe_file_verdict(file, language)?;
    let decode_secs = started.elapsed().as_secs_f64();

    // Lexicon post-process: the same dictionary pass every GUI delivery gets,
    // in every mode. Per segment in stream view (mirrors the live canvas
    // receiving utterances), whole-text otherwise.
    let mut post = codescribe_core::pipeline::stream_postprocess::StreamPostProcessor::new();
    let stdout = std::io::stdout();

    let cleaned = if stream && !verdict.raw.segments.is_empty() {
        let mut assembled: Vec<String> = Vec::new();
        let mut out = stdout.lock();
        for segment in &verdict.raw.segments {
            if let Some(clean) = post.process_utterance(&segment.text) {
                let clean = clean.trim().to_string();
                if !clean.is_empty() {
                    writeln!(out, "{clean}")?;
                    out.flush()?;
                    assembled.push(clean);
                }
            }
        }
        assembled.join(" ")
    } else {
        post.process_utterance(&verdict.text)
            .unwrap_or_else(|| verdict.text.clone())
    };
    let post_stats = post.stats();

    // Light+ floor — every delivery gets it except the literal contract,
    // mirroring the controller (`--raw` ≙ Ctrl-hold force_raw).
    let shaped = if raw {
        cleaned
    } else {
        codescribe_core::pipeline::light_plus::apply(&cleaned)
    };

    // AI formatting — the same lane call the GUI formatted mode makes.
    let (final_text, ai_status) = if format {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let result = runtime.block_on(codescribe_core::ai_formatting::format_text_with_status(
            &shaped, language, false, None,
        ));
        (result.text, Some(format!("{:?}", result.status)))
    } else {
        (shaped, None)
    };

    // Delivery on stdout (the stream view already printed the canvas; the
    // delivery still follows it so scripts always end with the shaped text).
    if stream {
        eprintln!("--- delivery ---");
    }
    println!("{final_text}");

    // Provenance to stderr, GUI-truth style.
    eprintln!(
        "engine={:?}/{:?} decode_secs={:.2} segments={} chars={} lexicon_rewrites={} avg_logprob={} light_plus={} ai_format={}",
        verdict.engine.engine,
        verdict.engine.mode,
        decode_secs,
        verdict.raw.segments.len(),
        final_text.chars().count(),
        post_stats.lexicon_rewrites,
        verdict
            .raw
            .avg_logprob
            .map(|v| std::format!("{v:.2}"))
            .unwrap_or_else(|| "n/a".into()),
        if raw { "skipped(raw)" } else { "applied" },
        ai_status.as_deref().unwrap_or("off"),
    );
    Ok(())
}
