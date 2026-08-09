//! Codescribe Teacher CLI — standalone proof of the learning triangle.
//!
//! ```text
//! Apple/live under-gen  ×  Whisper over-gen  ×  human
//!            └──────── Needs attention ────────┘
//!                         └─ lexicon hints ─┘
//! ```
//!
//! Examples:
//! ```bash
//! # Built-in proof (e2e 01_no-to-dobra texts)
//! cargo run --bin codescribe-teacher -- proof
//!
//! # Custom files
//! cargo run --bin codescribe-teacher -- compare \
//!   --live live.txt --whisper final.txt --human human.txt \
//!   --html /tmp/teacher.html
//!
//! open /tmp/teacher.html
//! ```

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use codescribe_core::quality::teacher::{TeacherInput, report_to_html, teach};

/// Command-line entry surface; `about` is set explicitly in `#[command(...)]`.
#[derive(Parser, Debug)]
#[command(
    name = "codescribe-teacher",
    about = "Teacher: live×whisper×human → Needs attention → lexicon (proof of mam rację i chuj)"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Command,
}

/// The two ways to feed the teacher: built-in fixtures, or three files.
#[derive(Subcommand, Debug)]
enum Command {
    /// Run the built-in e2e fixture proof (no audio, pure texts from 01_no-to-dobra).
    Proof {
        /// Write HTML report to this path.
        #[arg(long)]
        html: Option<PathBuf>,
        /// Also print JSON report.
        #[arg(long)]
        json: bool,
    },
    /// Compare three transcripts from files (or stdin markers).
    Compare {
        #[arg(long)]
        live: PathBuf,
        #[arg(long)]
        whisper: PathBuf,
        #[arg(long)]
        human: Option<PathBuf>,
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        html: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
}

/// Parse the command line and hand the assembled [`TeacherInput`] to [`run`].
fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Command::Proof { html, json } => {
            let input = TeacherInput {
                live: PROOF_LIVE.into(),
                whisper: PROOF_WHISPER.into(),
                human: Some(PROOF_HUMAN.into()),
                label: Some("proof/01_no-to-dobra e2e candle".into()),
            };
            run(input, html, json)
        }
        Command::Compare {
            live,
            whisper,
            human,
            label,
            html,
            json,
        } => {
            let input = TeacherInput {
                live: std::fs::read_to_string(&live)
                    .with_context(|| format!("read live {}", live.display()))?,
                whisper: std::fs::read_to_string(&whisper)
                    .with_context(|| format!("read whisper {}", whisper.display()))?,
                human: human
                    .as_ref()
                    .map(|p| {
                        std::fs::read_to_string(p)
                            .with_context(|| format!("read human {}", p.display()))
                    })
                    .transpose()?,
                label: label
                    .or_else(|| Some(format!("{} vs {}", live.display(), whisper.display()))),
            };
            run(input, html, json)
        }
    }
}

/// Run the teacher and print the report; optionally emit JSON and an HTML file.
///
/// Exits non-zero only on I/O failure or when both live and whisper are empty —
/// a "weak" thesis is a finding, not a failure, because this is a proof tool
/// rather than a CI gate.
fn run(input: TeacherInput, html: Option<PathBuf>, json: bool) -> Result<()> {
    let report = teach(input);

    println!("═══════════════════════════════════════════════════════════");
    println!("  Codescribe Teacher");
    if let Some(ref l) = report.label {
        println!("  label: {l}");
    }
    println!("═══════════════════════════════════════════════════════════");
    println!(
        "  tokens: live={} whisper={} human={:?}",
        report.live_token_count, report.whisper_token_count, report.human_token_count
    );
    println!(
        "  equal_ops={}  whisper_errors={}  at_live_weak={}  hit_rate={:?}",
        report.equal_ops,
        report.whisper_errors_vs_human,
        report.whisper_errors_at_live_weak,
        report.gap_hallucination_hit_rate
    );
    println!();
    println!("  THESIS: {}", report.thesis_summary);
    println!();
    println!("  ── Needs attention ({}) ──", report.attention.len());
    for (i, span) in report.attention.iter().enumerate() {
        println!(
            "  [{i}] {:?}\n      live={:?}\n      whisper={:?}\n      human={:?}\n      {}",
            span.kind, span.live_tokens, span.whisper_tokens, span.human_tokens, span.note
        );
    }
    println!();
    println!("  ── Lexicon hints ({}) ──", report.lexicon_hints.len());
    for h in &report.lexicon_hints {
        println!("  «{}» → «{}»  ({})", h.from_whisper, h.to_human, h.reason);
    }
    println!("═══════════════════════════════════════════════════════════");

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    }

    if let Some(path) = html {
        let doc = report_to_html(&report);
        std::fs::write(&path, doc).with_context(|| format!("write {}", path.display()))?;
        println!("HTML report: {}", path.display());
    }

    // Exit 0 even when thesis is "weak" — this is a proof tool, not a CI gate.
    // Non-zero only on I/O / empty inputs.
    if report.live_token_count == 0 && report.whisper_token_count == 0 {
        bail!("both live and whisper are empty");
    }
    Ok(())
}

// Fixtures captured from e2e_overlay_delivery_parity (01_no-to-dobra.wav, candle).

/// Apple-live under-gen fixture from the 01_no-to-dobra e2e candle sample.
const PROOF_LIVE: &str = "Teraz po parę słów, korzystając z surowej transkrypcji przez Codescribe mamy już pierwsze słowo do analizy wobec czego chcę, żebyś za chwilę, żebyś wziął ten plik WAV i puść i przychodził go na nasz endpoint. bo muszę mieć pewność czy leksykon działa Więc po prostu po to, aby Duże bazy kodowe przestały być tajemnicą Czarną, dziurą, na agentu veiłej. korzysta z rusta w wersji Toolchain 2024";

/// Whisper over-gen fixture paired with [`PROOF_LIVE`] for the built-in proof.
const PROOF_WHISPER: &str = "No to dobra, teraz generalnie powiem parę słów korzystając z surowej transkrypcji przez Codescribe. Mamy już pierwsze słowo do analizy, wobec czego chcę, żebyś wziął ten blik Wave i puścił go na... nasz endpoint, bo muszę mieć pewność czy leksykon działa, więc po prostu na temu biuz dupy. LOCK3 to aplikacja stworzona po to aby duże bazy kodowe przestały być tajemnicą i czarną dziurą dla agentów AI. Korzystam z Rust w wersji Tooltrain 2024. Dziękuję.";

/// Human reference transcript for the same candle; grounds lexicon-hint scoring.
const PROOF_HUMAN: &str = "No to dobra. Teraz generalnie powiem parę słów korzystając z surowej transkrypcji przez Codescribe (mamy już pierwsze słowo do analizy), do czego chcę, żebyś, nie wiem, żebyś wziął ten plik WAV i puścił go na nasz endpoint, bo muszę mieć pewność, hmmm, czy leksykon działa więc po [(niewyraźnie)]. Loctree to aplikacja stworzona po to, aby duże bazy kodowe przestały być tajemnicą i czarną dziurą dla agentów AI. Korzysta z Rusta, w wersji Toolchain 2024.";
