//! Self-improving loop runner.
//!
//! Usage:
//!   cargo run --bin codescribe-loop -- --date 2026-01-17 --apply
//!   cargo run --bin codescribe-loop -- --daemon   # Run as background daemon (1h interval)
//!
//! Created by M&K (c)2026 VetCoders

use anyhow::Result;
use clap::Parser;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use codescribe::config::Config;
use codescribe::quality_loop::{LexiconSource, QualityLoopConfig, run};
use codescribe::quality_report::{MetricsReference, QualityReport, QualityReportConfig};

/// Global mismatch counter for daemon mode
static PENDING_MISMATCHES: AtomicUsize = AtomicUsize::new(0);

#[derive(Parser, Clone)]
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

    /// Minimum occurrence count for lexicon suggestions (1 = include single occurrences)
    #[arg(long, default_value_t = 2)]
    lexicon_min_count: usize,

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

    /// Run as background daemon checking for new transcriptions every hour
    #[arg(long, default_value_t = false)]
    daemon: bool,

    /// Daemon check interval in seconds (default: 3600 = 1 hour)
    #[arg(long, default_value_t = 3600)]
    daemon_interval: u64,

    /// Mismatch threshold for notification (default: 20)
    #[arg(long, default_value_t = 20)]
    mismatch_threshold: usize,
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

    // Daemon mode - run background loop
    if args.daemon {
        return run_daemon(args).await;
    }

    // Single run mode
    run_single(&args).await
}

/// Run a single quality loop iteration
async fn run_single(args: &Args) -> Result<()> {
    let config_dir = Config::config_dir();
    let input_dir = args
        .input
        .clone()
        .unwrap_or_else(|| config_dir.join("transcriptions"));

    let output_dir = args.out.clone().unwrap_or_else(|| {
        let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
        config_dir.join("reports").join(format!("quality_{ts}"))
    });

    let history_path = args
        .history
        .clone()
        .unwrap_or_else(|| config_dir.join("reports").join("quality_history.jsonl"));

    let debug_mode = args.debug || env_bool("QUALITY_DEBUG_MODE");

    let report_config = QualityReportConfig {
        input_dir,
        output_dir: output_dir.clone(),
        date_filter: args.date.clone(),
        limit: args.limit,
        language: args.language.clone(),
        skip_cloud: args.skip_cloud,
        cloud_concurrency: args.cloud_concurrency,
        skip_formatting: args.skip_formatting,
        debug_mode,
        copy_audio: args.copy_audio,
        metrics_reference: match args.metrics_reference {
            ReferenceSourceArg::Corpus => MetricsReference::Corpus,
            ReferenceSourceArg::Cloud => MetricsReference::Cloud,
        },
    };

    let baseline_report = args.baseline.clone().map(|path| resolve_report_path(&path));

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
        lexicon_min_count: args.lexicon_min_count,
    };

    let out = run(loop_config).await?;
    println!("Quality loop completed: {}", out.display());
    Ok(())
}

/// Run as background daemon checking for new transcriptions
async fn run_daemon(args: Args) -> Result<()> {
    let interval = Duration::from_secs(args.daemon_interval);
    let threshold = args.mismatch_threshold;

    println!(
        "Starting quality daemon (interval: {}s, threshold: {} mismatches)",
        args.daemon_interval, threshold
    );

    // Write daemon state file for tray integration
    let config_dir = Config::config_dir();
    let state_path = config_dir.join("quality_daemon.json");

    loop {
        let now = chrono::Local::now();
        let date_filter = now.format("%Y-%m-%d").to_string();

        println!(
            "[{}] Checking for transcriptions from {}",
            now.format("%H:%M:%S"),
            date_filter
        );

        // Run with today's date filter, comparing local vs cloud
        let check_args = Args {
            date: Some(date_filter),
            skip_cloud: false,
            skip_formatting: true, // Skip AI formatting in daemon mode for speed
            limit: 0,              // No limit
            apply: false,          // Don't apply updates automatically
            ..args.clone()
        };

        match run_single(&check_args).await {
            Ok(()) => {
                // Load the latest report to count mismatches
                let mismatches = count_mismatches_from_latest_report(&config_dir);
                let prev = PENDING_MISMATCHES.swap(mismatches, Ordering::SeqCst);

                // Update state file for tray
                let _ = write_daemon_state(&state_path, mismatches);

                println!("  Mismatches: {} (was: {})", mismatches, prev);

                // Send notification if threshold reached
                if mismatches >= threshold && prev < threshold {
                    send_macos_notification(
                        &format!("{} mismatches detected", mismatches),
                        "Click to review quality report",
                    );
                }
            }
            Err(e) => {
                eprintln!("  Error: {}", e);
            }
        }

        tokio::time::sleep(interval).await;
    }
}

/// Count mismatches from the latest quality report
/// A mismatch is when local transcription differs significantly from cloud reference
fn count_mismatches_from_latest_report(config_dir: &Path) -> usize {
    let history_path = config_dir.join("reports").join("quality_history.jsonl");
    let content = match std::fs::read_to_string(&history_path) {
        Ok(c) => c,
        Err(_) => return 0,
    };

    // Get the last line (latest report)
    let last_line = content.lines().rev().find(|l| !l.trim().is_empty());
    let Some(last_line) = last_line else {
        return 0;
    };

    // Parse history entry to get report path
    #[derive(serde::Deserialize)]
    struct HistoryEntry {
        report_json: String,
    }

    let entry: HistoryEntry = match serde_json::from_str(last_line) {
        Ok(e) => e,
        Err(_) => return 0,
    };

    // Load the full report
    let report_content = match std::fs::read_to_string(&entry.report_json) {
        Ok(c) => c,
        Err(_) => return 0,
    };

    let report: QualityReport = match serde_json::from_str(&report_content) {
        Ok(r) => r,
        Err(_) => return 0,
    };

    // Count entries where local (raw/post) differs significantly from cloud
    // We consider it a mismatch if WER > 0.10 (10%)
    const MISMATCH_WER_THRESHOLD: f32 = 0.10;

    report
        .entries
        .iter()
        .filter(|entry| {
            // Compare post_wer vs cloud_wer, or raw_wer if post not available
            let local_wer = entry.metrics.post_wer.or(entry.metrics.raw_wer);
            let cloud_wer = entry.metrics.cloud_wer;

            match (local_wer, cloud_wer) {
                (Some(local), Some(cloud)) => {
                    // Significant mismatch if local is much worse than cloud
                    (local - cloud).abs() > MISMATCH_WER_THRESHOLD
                }
                _ => false,
            }
        })
        .count()
}

/// Write daemon state for tray integration
fn write_daemon_state(path: &Path, mismatches: usize) -> Result<()> {
    #[derive(serde::Serialize)]
    struct DaemonState {
        pending_mismatches: usize,
        last_check: String,
        latest_report: Option<String>,
    }

    let config_dir = Config::config_dir();
    let history_path = config_dir.join("reports").join("quality_history.jsonl");

    // Get latest report path
    let latest_report = std::fs::read_to_string(&history_path)
        .ok()
        .and_then(|content| {
            content
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .and_then(|line| {
                    #[derive(serde::Deserialize)]
                    struct Entry {
                        report_dir: String,
                    }
                    serde_json::from_str::<Entry>(line)
                        .ok()
                        .map(|e| e.report_dir)
                })
        });

    let state = DaemonState {
        pending_mismatches: mismatches,
        last_check: chrono::Local::now().to_rfc3339(),
        latest_report,
    };

    let json = serde_json::to_string_pretty(&state)?;
    std::fs::write(path, json)?;
    Ok(())
}

/// Send macOS notification using osascript
fn send_macos_notification(message: &str, subtitle: &str) {
    let script = format!(
        r#"display notification "{}" with title "CodeScribe Quality" subtitle "{}""#,
        message.replace('"', r#"\""#),
        subtitle.replace('"', r#"\""#)
    );

    let _ = Command::new("osascript").args(["-e", &script]).spawn();
}

/// Get path to the latest HTML report
pub fn get_latest_report_html() -> Option<PathBuf> {
    let config_dir = Config::config_dir();
    let state_path = config_dir.join("quality_daemon.json");

    let content = std::fs::read_to_string(&state_path).ok()?;

    #[derive(serde::Deserialize)]
    struct DaemonState {
        latest_report: Option<String>,
    }

    let state: DaemonState = serde_json::from_str(&content).ok()?;
    state
        .latest_report
        .map(|dir| PathBuf::from(dir).join("index.html"))
}

/// Get pending mismatch count from daemon state
pub fn get_pending_mismatches() -> usize {
    let config_dir = Config::config_dir();
    let state_path = config_dir.join("quality_daemon.json");

    let content = match std::fs::read_to_string(&state_path) {
        Ok(c) => c,
        Err(_) => return 0,
    };

    #[derive(serde::Deserialize)]
    struct DaemonState {
        pending_mismatches: usize,
    }

    serde_json::from_str::<DaemonState>(&content)
        .map(|s| s.pending_mismatches)
        .unwrap_or(0)
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
