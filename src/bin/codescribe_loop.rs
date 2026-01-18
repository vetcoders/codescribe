//! Self-improving loop runner.
//!
//! Usage:
//!   cargo run --bin codescribe-loop -- --date 2026-01-17 --apply
//!
//! Created by M&K (c)2026 VetCoders

use anyhow::Result;
use clap::Parser;
use std::path::{Path, PathBuf};

use codescribe::config::Config;
use codescribe::quality_loop::{LexiconSource, QualityLoopConfig, run};
use codescribe::quality_report::{MetricsReference, QualityReportConfig};

#[derive(Parser)]
#[command(name = "codescribe-loop")]
#[command(version)]
#[command(about = "Run the self-improving quality loop (report + regression + tuning)")]
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

    /// Baseline report.json or report directory to compare against
    #[arg(long)]
    baseline: Option<PathBuf>,

    /// History file path (default: ~/.codescribe/reports/quality_history.jsonl)
    #[arg(long)]
    history: Option<PathBuf>,

    /// Apply updates to lexicon/prompts/env tuning
    #[arg(long, default_value_t = false)]
    apply: bool,

    /// Regression threshold (delta in WER/CER)
    #[arg(long, default_value_t = 0.02)]
    regression_threshold: f32,

    /// Max lexicon updates to add per run
    #[arg(long, default_value_t = 50)]
    lexicon_max: usize,

    /// Disable lexicon auto-updates
    #[arg(long, default_value_t = false)]
    no_lexicon: bool,

    /// Disable gate threshold tuning
    #[arg(long, default_value_t = false)]
    no_gate: bool,

    /// Disable prompt tuning updates
    #[arg(long, default_value_t = false)]
    no_prompt: bool,

    /// Disable embeddings tuning updates
    #[arg(long, default_value_t = false)]
    no_embeddings_tuning: bool,

    /// Reference source for metrics (corpus .txt or cloud transcript)
    #[arg(long, value_enum, default_value = "corpus")]
    metrics_reference: ReferenceSourceArg,

    /// Lexicon source (corpus .txt or cloud transcript)
    #[arg(long, value_enum, default_value = "corpus")]
    lexicon_source: ReferenceSourceArg,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
enum ReferenceSourceArg {
    Corpus,
    Cloud,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

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

    let history_path = args
        .history
        .unwrap_or_else(|| config_dir.join("reports").join("quality_history.jsonl"));

    let debug_mode = args.debug || env_bool("QUALITY_DEBUG_MODE");

    let report_config = QualityReportConfig {
        input_dir,
        output_dir: output_dir.clone(),
        date_filter: args.date,
        limit: args.limit,
        language: args.language,
        skip_cloud: args.skip_cloud,
        skip_formatting: args.skip_formatting,
        debug_mode,
        copy_audio: args.copy_audio,
        metrics_reference: match args.metrics_reference {
            ReferenceSourceArg::Corpus => MetricsReference::Corpus,
            ReferenceSourceArg::Cloud => MetricsReference::Cloud,
        },
    };

    let baseline_report = args.baseline.map(|path| resolve_report_path(&path));

    let loop_config = QualityLoopConfig {
        report_config,
        baseline_report,
        history_path,
        regression_threshold: args.regression_threshold,
        apply_updates: args.apply,
        update_lexicon: !args.no_lexicon,
        lexicon_source: match args.lexicon_source {
            ReferenceSourceArg::Corpus => LexiconSource::Corpus,
            ReferenceSourceArg::Cloud => LexiconSource::Cloud,
        },
        update_gate: !args.no_gate,
        update_prompts: !args.no_prompt,
        update_embeddings: !args.no_embeddings_tuning,
        max_lexicon_updates: args.lexicon_max,
    };

    let out = run(loop_config).await?;
    println!("Quality loop completed: {}", out.display());
    Ok(())
}

fn env_bool(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn resolve_report_path(path: &Path) -> PathBuf {
    if path.is_dir() {
        path.join("report.json")
    } else {
        path.to_path_buf()
    }
}
