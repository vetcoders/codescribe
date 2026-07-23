//! Batch quality report generator.
//!
//! Usage:
//!   cargo run --bin qube-report -- --date 2026-01-17 --limit 5
//!   cargo run --bin qube-report -- --replay-corrections [--apply]

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

use codescribe::config::Config;
use codescribe::qube_report::{LocalTranscriptionMode, MetricsReference, QualityReportConfig, run};
use codescribe_core::quality::overlay_quality::replay_corrections_through_extractor;

#[derive(Parser)]
#[command(name = "qube-report")]
#[command(version)]
#[command(about = "Generate a quality report for Codescribe transcriptions")]
struct Args {
    /// Input directory with date subfolders containing WAV+TXT pairs
    #[arg(long)]
    input: Option<PathBuf>,

    /// Output directory (default: ~/.codescribe/reports/quality_<timestamp>)
    #[arg(long)]
    out: Option<PathBuf>,

    /// Filter by date folder (e.g., 2026-01-17)
    #[arg(long)]
    date: Option<String>,

    /// Limit to last N pairs (0 = no limit)
    #[arg(long, default_value_t = 3)]
    limit: usize,

    /// Force language (e.g., pl, en)
    #[arg(long)]
    language: Option<String>,

    /// Skip cloud reference transcription
    #[arg(long, default_value_t = false)]
    skip_cloud: bool,

    /// Max concurrent cloud STT requests (0 = unlimited)
    #[arg(long, default_value_t = 0)]
    cloud_concurrency: usize,

    /// Skip AI formatting
    #[arg(long, default_value_t = false)]
    skip_formatting: bool,

    /// Show references immediately in HTML (debug mode)
    #[arg(long, default_value_t = false)]
    debug: bool,

    /// Copy audio into report (instead of symlink)
    #[arg(long, default_value_t = false)]
    copy_audio: bool,

    /// Disable embedding gate for postprocess (faster, less strict)
    #[arg(long, default_value_t = false)]
    no_embeddings: bool,

    /// Reference source for metrics (corpus .txt or cloud transcript)
    #[arg(long, value_enum, default_value = "corpus")]
    metrics_reference: ReferenceSourceArg,

    /// Replay historical quality/corrections.jsonl through the current
    /// word-level lexicon extractor (dry-run table by default).
    #[arg(long, default_value_t = false)]
    replay_corrections: bool,

    /// Optional path to corrections.jsonl (default: $CODESCRIBE_DATA_DIR/quality/corrections.jsonl)
    #[arg(long)]
    corrections_path: Option<PathBuf>,

    /// With --replay-corrections: upsert extracted pairs into lexicon.custom.jsonl
    /// (backs up existing file to .bak-replay-<ts> first).
    #[arg(long, default_value_t = false)]
    apply: bool,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
enum ReferenceSourceArg {
    Corpus,
    Cloud,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    if args.replay_corrections {
        let config_dir = Config::config_dir();
        let path = args
            .corrections_path
            .unwrap_or_else(|| config_dir.join("quality").join("corrections.jsonl"));
        // Live lexicon is untouched unless --apply. Dry-run always writes a proposed
        // JSONL + human-readable report under the config dir (or --out).
        let table = replay_corrections_through_extractor(&path, args.apply)?;
        let out_dir = args.out.unwrap_or_else(|| config_dir.clone());
        std::fs::create_dir_all(&out_dir)?;
        let proposed_path = out_dir.join("lexicon.custom.proposed.jsonl");
        let report_path = out_dir.join("lexicon_replay_report.md");

        // Proposed rows: one JSON object per pair (not applied).
        {
            use std::io::Write;
            let mut proposed = std::fs::File::create(&proposed_path)?;
            for row in &table {
                let line = serde_json::json!({
                    "term": row.canonical,
                    "mispronunciations": [row.variant],
                    "source": "correction",
                    "correction_id": row.correction_id,
                    "source_line": row.line,
                });
                writeln!(proposed, "{line}")?;
            }
        }

        {
            use std::io::Write;
            let mut report = std::fs::File::create(&report_path)?;
            writeln!(report, "# Lexicon corrections replay")?;
            writeln!(report)?;
            writeln!(report, "- source: `{}`", path.display())?;
            writeln!(report, "- candidate pairs: {}", table.len())?;
            writeln!(
                report,
                "- mode: {}",
                if args.apply {
                    "apply (live lexicon updated after backup)"
                } else {
                    "dry-run (live lexicon untouched)"
                }
            )?;
            writeln!(report, "- proposed file: `{}`", proposed_path.display())?;
            writeln!(report)?;
            writeln!(report, "| line | correction_id | variant | canonical |")?;
            writeln!(report, "| --- | --- | --- | --- |")?;
            for row in &table {
                writeln!(
                    report,
                    "| {} | `{}` | {} | {} |",
                    row.line, row.correction_id, row.variant, row.canonical
                )?;
            }
        }

        println!(
            "line\tcorrection_id\tvariant\tcanonical\tapplied\t(source={})",
            path.display()
        );
        for row in &table {
            println!(
                "{}\t{}\t{}\t{}\t{}",
                row.line, row.correction_id, row.variant, row.canonical, row.applied
            );
        }
        println!(
            "replay: {} candidate pair(s){} from {}",
            table.len(),
            if args.apply { " applied" } else { " (dry-run)" },
            path.display()
        );
        println!("proposed: {}", proposed_path.display());
        println!("report: {}", report_path.display());
        return Ok(());
    }

    if args.no_embeddings {
        // SAFETY: this is a single-process CLI before any threads start.
        unsafe {
            std::env::set_var("CODESCRIBE_STREAM_DISABLE_EMBEDDINGS", "1");
        }
    }

    let config_dir = Config::config_dir();
    let input_dir = args
        .input
        .unwrap_or_else(|| config_dir.join("transcriptions"));

    let output_dir = args.out.unwrap_or_else(|| {
        let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
        config_dir.join("reports").join(format!("quality_{ts}"))
    });

    let debug_mode = args.debug || env_bool("QUALITY_DEBUG_MODE");

    let report_config = QualityReportConfig {
        input_dir,
        output_dir: output_dir.clone(),
        date_filter: args.date,
        limit: args.limit,
        language: args.language,
        skip_cloud: args.skip_cloud,
        cloud_concurrency: args.cloud_concurrency,
        skip_formatting: args.skip_formatting,
        debug_mode,
        copy_audio: args.copy_audio,
        metrics_reference: match args.metrics_reference {
            ReferenceSourceArg::Corpus => MetricsReference::Corpus,
            ReferenceSourceArg::Cloud => MetricsReference::Cloud,
        },
        local_transcription: LocalTranscriptionMode::LocalWhisper,
    };

    let out = run(report_config).await?;
    println!("Quality report generated: {}", out.display());
    Ok(())
}

fn env_bool(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}
