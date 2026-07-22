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
        let table = replay_corrections_through_extractor(&path, args.apply)?;
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
