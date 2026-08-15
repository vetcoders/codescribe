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
//! - `--stream` = the LIVE CANVAS view: per-segment text flushed to stdout
//!   as decoding progresses through the file (lexicon applied per segment;
//!   Light+ belongs to delivery, not the canvas).
//! - `--raw` = the Ctrl-hold contract: literal words, no Light+.
//! - `-f/--format` = the AI-formatted lane (same `ai_formatting` call and
//!   lane config the GUI uses; requires a configured key).
//! - `transcribe live` = follow the app-owned clean transcript bus and flush
//!   newly created utterance drafts to stdout one line at a time. Revisions and
//!   the final product seal remain explicit bus events. It never opens a
//!   second microphone or reconstructs text from UI previews.
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
    /// Transcribe a file or follow the app-owned live transcript bus
    Transcribe {
        /// Path to the audio file (omit when using `transcribe live`)
        file: Option<std::path::PathBuf>,
        /// File language; live accepts it for compatibility but app settings own capture
        #[arg(short, long, global = true)]
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
        #[command(subcommand)]
        mode: Option<TranscribeMode>,
    },
}

#[derive(Subcommand)]
enum TranscribeMode {
    /// Follow the app's transcript draft/seal bus; Ctrl-C closes the reader
    Live,
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
            mode,
        } => match mode {
            Some(TranscribeMode::Live) => {
                anyhow::ensure!(
                    file.is_none() && !stream && !raw && !format,
                    "`transcribe live` does not accept a file, --stream, --raw, or --format"
                );
                transcribe_live(language)
            }
            None => {
                let file = file.ok_or_else(|| {
                    anyhow::anyhow!("missing <FILE> (or use `codescribe transcribe live`)")
                })?;
                transcribe(&file, language.as_deref(), stream, raw, format)
            }
        },
    }
}

fn transcribe_live(language: Option<String>) -> anyhow::Result<()> {
    use codescribe::presentation::transcript_bus::{CleanTranscriptEvent, transcript_bus_path};
    use std::io::{Read, Seek, SeekFrom, Write as _};

    let path = transcript_bus_path();
    let mut offset = std::fs::metadata(&path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let mut pending = Vec::<u8>::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async move {
        eprintln!("codescribe live: app transcript bus -> live draft stdout");
        eprintln!("bus={} start=end stop=Ctrl-C", path.display());
        eprintln!(
            "language_hint={} owner=Codescribe.app",
            language.as_deref().unwrap_or("auto")
        );

        loop {
            tokio::select! {
                signal = tokio::signal::ctrl_c() => {
                    signal?;
                    eprintln!("codescribe live: stopped");
                    return Ok(());
                }
                () = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
            }

            let mut file = match std::fs::File::open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            let file_len = file.metadata()?.len();
            if file_len < offset {
                offset = 0;
                pending.clear();
            }
            file.seek(SeekFrom::Start(offset))?;
            let mut chunk = Vec::new();
            file.read_to_end(&mut chunk)?;
            offset = offset.saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
            pending.extend_from_slice(&chunk);

            while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = pending.drain(..=newline).collect();
                let line = &line[..line.len().saturating_sub(1)];
                if line.is_empty() {
                    continue;
                }
                let event: CleanTranscriptEvent = match serde_json::from_slice(line) {
                    Ok(event) => event,
                    Err(error) => {
                        eprintln!("codescribe live: invalid transcript event: {error}");
                        continue;
                    }
                };
                if let Some(text) = live_event_text(&event.status, &event.text) {
                    let stdout = std::io::stdout();
                    let mut out = stdout.lock();
                    writeln!(out, "{text}")?;
                    out.flush()?;
                } else if event.status == "utterance_revised" {
                    eprintln!(
                        "codescribe live: revision available session={} utterance={}",
                        event.session_id,
                        event
                            .utterance_id
                            .map(|id| id.to_string())
                            .unwrap_or_else(|| "unknown".to_string())
                    );
                } else if event.status == "transcript_sealed" {
                    eprintln!(
                        "codescribe live: transcript sealed session={} chars={}",
                        event.session_id,
                        event.text.chars().count()
                    );
                }
            }
        }
    })
}

/// Plain stdout is intentionally append-only and therefore shows each new draft
/// slot once. Revisions and the final seal remain machine-readable in the
/// canonical NDJSON bus and are announced on stderr without transcript content.
fn live_event_text<'a>(status: &str, text: &'a str) -> Option<&'a str> {
    if status != "utterance_draft" {
        return None;
    }
    let text = text.trim();
    (!text.is_empty()).then_some(text)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_command_is_a_subcommand_not_a_file_named_live() {
        let cli = Cli::try_parse_from(["codescribe", "transcribe", "live", "--language", "pl"])
            .expect("live command should parse");
        let Command::Transcribe {
            file,
            language,
            mode,
            ..
        } = cli.command;
        assert!(file.is_none());
        assert_eq!(language.as_deref(), Some("pl"));
        assert!(matches!(mode, Some(TranscribeMode::Live)));
    }

    #[test]
    fn live_plain_text_emits_only_nonempty_new_drafts() {
        assert_eq!(
            live_event_text("utterance_draft", "  instrukcja  "),
            Some("instrukcja")
        );
        assert_eq!(live_event_text("utterance_draft", "  "), None);
        assert_eq!(live_event_text("utterance_revised", "poprawka"), None);
        assert_eq!(live_event_text("transcript_sealed", "całość"), None);
    }
}
