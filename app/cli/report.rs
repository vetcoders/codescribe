//! Batch quality report generator.
//!
//! Reached as `codescribe report` and, for compatibility, as the standalone
//! `qube-report` binary. The surface lives here rather than in a bin so both
//! entry points share one flag set and one dispatch.

use anyhow::Result;
use std::path::PathBuf;

use crate::config::Config;
use crate::qube_report::{
    LocalTranscriptionMode, MetricsReference, QualityReportConfig, compare_truth_dirs,
    render_truth_comparison, run as run_quality_report,
};
use codescribe_core::quality::lexicon_replay::{ReplayOutcome, run_lexicon_replay};

/// Command-line surface of the report generator. Two distinct jobs share this
/// binary: the default batch quality report, and the `--replay-corrections`
/// lexicon extraction pass, which ignores the report flags entirely.
#[derive(clap::Args, Debug)]
pub struct ReportArgs {
    /// Input directory with date subfolders containing WAV+TXT pairs
    #[arg(long)]
    input: Option<PathBuf>,

    /// Output directory (default: ~/.codescribe/reports/quality_<timestamp>)
    #[arg(long, visible_alias = "output")]
    out: Option<PathBuf>,

    /// Baseline archive directory of `*.truth.json` sidecars (requires `--candidate-dir`)
    #[arg(long, requires = "candidate_dir")]
    baseline_dir: Option<PathBuf>,

    /// Fresh-run directory of `*.truth.json` sidecars (requires `--baseline-dir`)
    #[arg(long, requires = "baseline_dir")]
    candidate_dir: Option<PathBuf>,

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

/// Which transcript metrics are scored against: the corpus `.txt` beside the
/// audio, or the cloud transcription produced during the run.
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum ReferenceSourceArg {
    Corpus,
    Cloud,
}

/// Dispatch to one of the two jobs and print where the output landed.
///
/// `--replay-corrections` returns early: it writes a proposed JSONL plus a
/// markdown table and leaves the live lexicon untouched unless `--apply` was
/// given, so the extraction can be reviewed before it changes anything.
/// Run the report job: truth-dir comparison, corrections replay, or the batch
/// quality report, in that order of precedence.
pub async fn run(args: ReportArgs) -> Result<()> {
    if let (Some(baseline_dir), Some(candidate_dir)) = (&args.baseline_dir, &args.candidate_dir) {
        let comparison = compare_truth_dirs(baseline_dir, candidate_dir)?;
        print!("{}", render_truth_comparison(&comparison));
        let out_dir = args.out.clone().unwrap_or_else(|| candidate_dir.clone());
        std::fs::create_dir_all(&out_dir)?;
        let path = out_dir.join("comparison.json");
        std::fs::write(&path, serde_json::to_vec_pretty(&comparison)?)?;
        println!("comparison: {}", path.display());
        return Ok(());
    }

    if args.replay_corrections {
        let config_dir = Config::config_dir();
        let path = args
            .corrections_path
            .unwrap_or_else(|| config_dir.join("quality").join("corrections.jsonl"));
        let out_dir = args.out.unwrap_or_else(|| config_dir.clone());
        // Same library call `codescribe lexicon replay` makes, so both entry
        // points produce byte-identical artifacts.
        let outcome = run_lexicon_replay(&path, &out_dir, args.apply)?;
        print_replay_outcome(&outcome, &path, args.apply);
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

    let out = run_quality_report(report_config).await?;
    println!("Quality report generated: {}", out.display());
    Ok(())
}

/// Read a boolean environment flag, accepting `1` or a case-insensitive
/// `true`; anything else (including unset) reads as `false`.
fn env_bool(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Print a replay outcome the way the legacy binary always has: one TSV row per
/// pair, then the tier histogram and the artifact paths.
fn print_replay_outcome(outcome: &ReplayOutcome, source: &std::path::Path, applied: bool) {
    println!(
        "line\tcorrection_id\tvariant\tcanonical\tverdict\tapplied\t(source={})",
        source.display()
    );
    for row in &outcome.table {
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}",
            row.line, row.correction_id, row.variant, row.canonical, row.verdict, row.applied
        );
    }
    println!(
        "replay: {} candidate pair(s), {} accepted{} from {}",
        outcome.table.len(),
        outcome.accepted(),
        if applied {
            " and applied"
        } else {
            " (dry-run)"
        },
        source.display()
    );
    for (tier, count) in &outcome.tier_counts {
        println!("  {tier}: {count}");
    }
    println!("accepted: {}", outcome.accepted_path.display());
    println!("review: {}", outcome.review_path.display());
    println!("rejected: {}", outcome.rejected_path.display());
    println!("report: {}", outcome.report_path.display());
}
