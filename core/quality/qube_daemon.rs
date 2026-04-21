//! Self-improving quality loop for batch transcription evaluation.
//!
//! Flow: batch -> report -> regression analysis -> tuning updates -> re-run later.
//!
//! Created by M&K (c)2026 VetCoders

use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::qube_report::{QualityReport, QualityReportConfig, ReportSummary};
use crate::safe_path::{
    safe_append_line_bounded, safe_canonicalize_bounded, safe_prepare_path,
    safe_read_to_string_bounded,
};

const DEFAULT_REGRESSION_THRESHOLD: f32 = 0.02;
const DEFAULT_SIMILARITY: f32 = 0.93;
const DEFAULT_NOVELTY: f32 = 0.12;

#[derive(Debug, Clone)]
pub struct QubeDaemonConfig {
    pub report_config: QualityReportConfig,
    pub baseline_report: Option<PathBuf>,
    pub history_path: PathBuf,
    pub regression_threshold: f32,
    pub apply_updates: bool,
    pub update_lexicon: bool,
    pub lexicon_source: LexiconSource,
    pub update_gate: bool,
    pub update_prompts: bool,
    pub update_embeddings: bool,
    pub max_lexicon_updates: usize,
    /// Minimum occurrence count for lexicon suggestions (default: 2)
    pub lexicon_min_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LexiconSource {
    /// Reference from corpus .txt files (human-written)
    Corpus,
    /// Reference from cloud STT (Google/Deepgram)
    Cloud,
    /// Reference from AI-formatted transcript (Whisper + LLM correction)
    AiFormatted,
}

impl LexiconSource {
    fn as_str(self) -> &'static str {
        match self {
            LexiconSource::Corpus => "corpus",
            LexiconSource::Cloud => "cloud",
            LexiconSource::AiFormatted => "ai",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoopAnalysis {
    pub generated_at: String,
    pub current_report: String,
    pub baseline_report: Option<String>,
    pub summary: LoopSummary,
    pub regressions: Vec<RegressionFinding>,
    pub updates: Vec<UpdateAction>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoopSummary {
    pub total_entries: usize,
    pub compared_entries: usize,
    pub regression_count: usize,
    pub improvement_count: usize,
    pub post_worse_ratio: Option<f32>,
    pub ai_worse_ratio: Option<f32>,
    pub gate_drop_rate: Option<f32>,
    pub suspicious_rate: Option<f32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RegressionFinding {
    pub id: String,
    pub metric: String,
    pub current: f32,
    pub baseline: f32,
    pub delta: f32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateAction {
    pub kind: String,
    pub detail: String,
    pub applied: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct LoopHistoryEntry {
    generated_at: String,
    report_dir: String,
    report_json: String,
    summary: ReportSummary,
}

pub async fn run(config: QubeDaemonConfig) -> Result<PathBuf> {
    let config_root = Config::config_dir();
    let report_config = normalize_report_config(&config.report_config, &config_root)?;
    let output_dir = crate::qube_report::run(report_config).await?;
    let output_root = safe_canonicalize_bounded(&output_dir, &config_root)?;
    let report_path = output_root.join("report.json");
    let report = load_report(&report_path, &config_root)?;

    let history_path = resolve_history_path(&config.history_path, &config_root)?;
    let baseline_path = resolve_baseline(&config, &output_root, &config_root, &history_path)?;
    let baseline_report = baseline_path
        .as_ref()
        .and_then(|path| load_report(path, &config_root).ok());

    let (regressions, regression_summary) = analyze_regressions(
        &report,
        baseline_report.as_ref(),
        config.regression_threshold,
    );

    let mut updates = Vec::new();
    let signals = QualitySignals::from_report(&report, config.regression_threshold);
    let postprocess_stats = PostprocessStats::from_report(&report);

    if config.update_gate
        && let Some(update) =
            propose_gate_update(&signals, &postprocess_stats, config.apply_updates)?
    {
        updates.push(update);
    }

    if config.update_embeddings
        && let Some(update) =
            propose_embedding_update(&signals, &postprocess_stats, config.apply_updates)?
    {
        updates.push(update);
    }

    if config.update_prompts
        && let Some(update) = propose_prompt_tuning(&signals, &report, config.apply_updates)?
    {
        updates.push(update);
    }

    if config.update_lexicon
        && let Some(update) = propose_lexicon_updates(
            &report,
            config.max_lexicon_updates,
            config.lexicon_min_count,
            config.apply_updates,
            config.lexicon_source,
        )?
    {
        updates.push(update);
    }

    let analysis = LoopAnalysis {
        generated_at: Local::now().to_rfc3339(),
        current_report: report_path.to_string_lossy().to_string(),
        baseline_report: baseline_path.map(|p| p.to_string_lossy().to_string()),
        summary: regression_summary,
        regressions,
        updates,
    };

    write_analysis_files(&output_root, &analysis)?;
    append_history(&history_path, &config_root, &report, &output_root)?;

    Ok(output_root)
}

fn load_report(path: &Path, root: &Path) -> Result<QualityReport> {
    let data = safe_read_to_string_bounded(path, root)
        .with_context(|| format!("Failed to read report {}", path.display()))?;
    serde_json::from_str(&data).context("Failed to parse report.json")
}

fn normalize_report_config(
    config: &QualityReportConfig,
    root: &Path,
) -> Result<QualityReportConfig> {
    let mut normalized = config.clone();
    let input_candidate = safe_prepare_path(&normalized.input_dir, root)?;
    if !input_candidate.exists() {
        anyhow::bail!(
            "Input directory does not exist: {}",
            input_candidate.display()
        );
    }
    normalized.input_dir = safe_canonicalize_bounded(&input_candidate, root)?;
    normalized.output_dir = safe_prepare_path(&normalized.output_dir, root)?;
    Ok(normalized)
}

fn resolve_history_path(path: &Path, root: &Path) -> Result<PathBuf> {
    safe_prepare_path(path, root)
}

fn resolve_baseline(
    config: &QubeDaemonConfig,
    output_dir: &Path,
    root: &Path,
    history_path: &Path,
) -> Result<Option<PathBuf>> {
    if let Some(path) = config.baseline_report.as_ref() {
        let resolved = resolve_report_path(path);
        let bounded = safe_canonicalize_bounded(&resolved, root)
            .with_context(|| format!("Baseline report must stay within {}", root.display()))?;
        return Ok(Some(bounded));
    }

    let history = read_last_history(history_path, root)?;
    let Some(history) = history else {
        return Ok(None);
    };
    let history_path = PathBuf::from(&history.report_json);
    if history_path.exists() && history_path != output_dir.join("report.json") {
        let bounded = safe_canonicalize_bounded(&history_path, root)
            .with_context(|| format!("Baseline report must stay within {}", root.display()))?;
        return Ok(Some(bounded));
    }

    Ok(None)
}

fn resolve_report_path(path: &Path) -> PathBuf {
    if path.is_dir() {
        path.join("report.json")
    } else {
        path.to_path_buf()
    }
}

fn read_last_history(path: &Path, root: &Path) -> Result<Option<LoopHistoryEntry>> {
    let content = safe_read_to_string_bounded(path, root).ok();
    let content = match content {
        Some(content) => content,
        None => return Ok(None),
    };
    for line in content.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<LoopHistoryEntry>(trimmed) {
            return Ok(Some(entry));
        }
    }
    Ok(None)
}

fn append_history(
    path: &Path,
    root: &Path,
    report: &QualityReport,
    output_dir: &Path,
) -> Result<()> {
    let entry = LoopHistoryEntry {
        generated_at: report.generated_at.clone(),
        report_dir: output_dir.to_string_lossy().to_string(),
        report_json: output_dir.join("report.json").to_string_lossy().to_string(),
        summary: report.summary.clone(),
    };

    let line = serde_json::to_string(&entry)?;
    safe_append_line_bounded(path, root, &line)
}

fn write_analysis_files(output_dir: &Path, analysis: &LoopAnalysis) -> Result<()> {
    let json_path = output_dir.join("analysis.json");
    let md_path = output_dir.join("analysis.md");

    let json = serde_json::to_string_pretty(analysis)?;
    crate::safe_path::safe_write_bounded(&json_path, output_dir, &json)?;

    let md = render_analysis_markdown(analysis);
    crate::safe_path::safe_write_bounded(&md_path, output_dir, &md)?;

    Ok(())
}

fn render_analysis_markdown(analysis: &LoopAnalysis) -> String {
    let mut out = String::new();
    out.push_str("# CodeScribe Quality Loop Analysis\n\n");
    out.push_str(&format!("Generated: {}\n\n", analysis.generated_at));
    out.push_str(&format!("- Current report: {}\n", analysis.current_report));
    if let Some(baseline) = &analysis.baseline_report {
        out.push_str(&format!("- Baseline report: {}\n", baseline));
    }

    out.push_str("\n## Summary\n\n");
    out.push_str(&format!(
        "- Entries compared: {}/{}\n",
        analysis.summary.compared_entries, analysis.summary.total_entries
    ));
    out.push_str(&format!(
        "- Regressions: {}, Improvements: {}\n",
        analysis.summary.regression_count, analysis.summary.improvement_count
    ));

    if let Some(rate) = analysis.summary.post_worse_ratio {
        out.push_str(&format!("- Post worse ratio: {:.2}\n", rate));
    }
    if let Some(rate) = analysis.summary.ai_worse_ratio {
        out.push_str(&format!("- AI worse ratio: {:.2}\n", rate));
    }
    if let Some(rate) = analysis.summary.gate_drop_rate {
        out.push_str(&format!("- Gate drop rate: {:.2}\n", rate));
    }
    if let Some(rate) = analysis.summary.suspicious_rate {
        out.push_str(&format!("- Suspicious rate: {:.2}\n", rate));
    }

    if !analysis.regressions.is_empty() {
        out.push_str("\n## Regressions\n\n");
        out.push_str("| ID | Metric | Current | Baseline | Delta |\n");
        out.push_str("| --- | --- | --- | --- | --- |\n");
        for reg in analysis.regressions.iter().take(50) {
            out.push_str(&format!(
                "| {} | {} | {:.3} | {:.3} | {:.3} |\n",
                reg.id, reg.metric, reg.current, reg.baseline, reg.delta
            ));
        }
    }

    if !analysis.updates.is_empty() {
        out.push_str("\n## Updates\n\n");
        for update in &analysis.updates {
            out.push_str(&format!(
                "- {}: {} (applied={})\n",
                update.kind, update.detail, update.applied
            ));
        }
    }

    out
}

fn analyze_regressions(
    report: &QualityReport,
    baseline: Option<&QualityReport>,
    threshold: f32,
) -> (Vec<RegressionFinding>, LoopSummary) {
    let mut regressions = Vec::new();
    let mut improvements = 0usize;
    let mut compared = 0usize;

    if let Some(base) = baseline {
        let base_map = base
            .entries
            .iter()
            .map(|entry| (entry.id.clone(), entry))
            .collect::<HashMap<_, _>>();

        for entry in &report.entries {
            let Some(base_entry) = base_map.get(&entry.id) else {
                continue;
            };
            compared += 1;
            compare_metric(
                &entry.id,
                "raw_wer",
                entry.metrics.raw_wer,
                base_entry.metrics.raw_wer,
                threshold,
                &mut regressions,
                &mut improvements,
            );
            compare_metric(
                &entry.id,
                "post_wer",
                entry.metrics.post_wer,
                base_entry.metrics.post_wer,
                threshold,
                &mut regressions,
                &mut improvements,
            );
            compare_metric(
                &entry.id,
                "ai_wer",
                entry.metrics.ai_wer,
                base_entry.metrics.ai_wer,
                threshold,
                &mut regressions,
                &mut improvements,
            );
            compare_metric(
                &entry.id,
                "cloud_wer",
                entry.metrics.cloud_wer,
                base_entry.metrics.cloud_wer,
                threshold,
                &mut regressions,
                &mut improvements,
            );
            compare_metric(
                &entry.id,
                "raw_cer",
                entry.metrics.raw_cer,
                base_entry.metrics.raw_cer,
                threshold,
                &mut regressions,
                &mut improvements,
            );
            compare_metric(
                &entry.id,
                "post_cer",
                entry.metrics.post_cer,
                base_entry.metrics.post_cer,
                threshold,
                &mut regressions,
                &mut improvements,
            );
            compare_metric(
                &entry.id,
                "ai_cer",
                entry.metrics.ai_cer,
                base_entry.metrics.ai_cer,
                threshold,
                &mut regressions,
                &mut improvements,
            );
            compare_metric(
                &entry.id,
                "cloud_cer",
                entry.metrics.cloud_cer,
                base_entry.metrics.cloud_cer,
                threshold,
                &mut regressions,
                &mut improvements,
            );
        }
    }

    let signals = QualitySignals::from_report(report, threshold);
    let post_stats = PostprocessStats::from_report(report);

    let summary = LoopSummary {
        total_entries: report.entries.len(),
        compared_entries: compared,
        regression_count: regressions.len(),
        improvement_count: improvements,
        post_worse_ratio: signals.post_worse_ratio,
        ai_worse_ratio: signals.ai_worse_ratio,
        gate_drop_rate: post_stats.gate_drop_rate(),
        suspicious_rate: post_stats.suspicious_rate(),
    };

    (regressions, summary)
}

fn compare_metric(
    id: &str,
    metric: &str,
    current: Option<f32>,
    baseline: Option<f32>,
    threshold: f32,
    regressions: &mut Vec<RegressionFinding>,
    improvements: &mut usize,
) {
    let (Some(current), Some(baseline)) = (current, baseline) else {
        return;
    };
    let delta = current - baseline;
    if delta > threshold {
        regressions.push(RegressionFinding {
            id: id.to_string(),
            metric: metric.to_string(),
            current,
            baseline,
            delta,
        });
    } else if delta < -threshold {
        *improvements += 1;
    }
}

struct QualitySignals {
    post_worse_ratio: Option<f32>,
    ai_worse_ratio: Option<f32>,
    avg_raw_wer: Option<f32>,
    avg_post_wer: Option<f32>,
    avg_ai_wer: Option<f32>,
}

impl QualitySignals {
    fn from_report(report: &QualityReport, threshold: f32) -> Self {
        let mut post_worse = 0usize;
        let mut post_total = 0usize;
        let mut ai_worse = 0usize;
        let mut ai_total = 0usize;

        for entry in &report.entries {
            if let (Some(raw), Some(post)) = (entry.metrics.raw_wer, entry.metrics.post_wer) {
                post_total += 1;
                if post > raw + threshold {
                    post_worse += 1;
                }
            }
            if let (Some(post), Some(ai)) = (entry.metrics.post_wer, entry.metrics.ai_wer) {
                ai_total += 1;
                if ai > post + threshold {
                    ai_worse += 1;
                }
            }
        }

        Self {
            post_worse_ratio: ratio(post_worse, post_total),
            ai_worse_ratio: ratio(ai_worse, ai_total),
            avg_raw_wer: report.summary.avg_raw_wer,
            avg_post_wer: report.summary.avg_post_wer,
            avg_ai_wer: report.summary.avg_ai_wer,
        }
    }
}

fn ratio(numer: usize, denom: usize) -> Option<f32> {
    if denom == 0 {
        None
    } else {
        Some(numer as f32 / denom as f32)
    }
}

struct PostprocessStats {
    input_chunks: u64,
    gate_drops: u64,
    suspicious: u64,
    embeddings_enabled: Option<bool>,
}

impl PostprocessStats {
    fn from_report(report: &QualityReport) -> Self {
        let mut input = 0u64;
        let mut gate = 0u64;
        let mut suspicious = 0u64;
        let mut embeddings = None;

        for entry in &report.entries {
            let Some(stats) = entry.postprocess_stats.as_ref() else {
                continue;
            };
            input += stats.input_chunks;
            gate += stats.gate_drops;
            suspicious += stats.suspicious_chunks;
            embeddings = match embeddings {
                None => Some(stats.embeddings_enabled),
                Some(value) if value == stats.embeddings_enabled => Some(value),
                Some(_) => None,
            };
        }

        Self {
            input_chunks: input,
            gate_drops: gate,
            suspicious,
            embeddings_enabled: embeddings,
        }
    }

    fn gate_drop_rate(&self) -> Option<f32> {
        if self.input_chunks == 0 {
            None
        } else {
            Some(self.gate_drops as f32 / self.input_chunks as f32)
        }
    }

    fn suspicious_rate(&self) -> Option<f32> {
        if self.input_chunks == 0 {
            None
        } else {
            Some(self.suspicious as f32 / self.input_chunks as f32)
        }
    }
}

fn propose_gate_update(
    signals: &QualitySignals,
    stats: &PostprocessStats,
    apply: bool,
) -> Result<Option<UpdateAction>> {
    let Some(post_worse_ratio) = signals.post_worse_ratio else {
        return Ok(None);
    };
    let config_root = Config::config_dir();
    let env_path = config_root.join(".env");
    let similarity = read_env_f32(
        &env_path,
        &config_root,
        "CODESCRIBE_STREAM_SIMILARITY",
        DEFAULT_SIMILARITY,
    );
    let novelty = read_env_f32(
        &env_path,
        &config_root,
        "CODESCRIBE_STREAM_NOVELTY",
        DEFAULT_NOVELTY,
    );

    let mut new_similarity = similarity;
    let mut new_novelty = novelty;
    let mut reason = None;

    let avg_regression = match (signals.avg_post_wer, signals.avg_raw_wer) {
        (Some(post), Some(raw)) => post > raw + DEFAULT_REGRESSION_THRESHOLD,
        _ => false,
    };

    if post_worse_ratio >= 0.30 || avg_regression {
        new_similarity = (similarity + 0.01).min(0.98);
        new_novelty = (novelty - 0.01).max(0.05);
        reason = Some("postprocess regressions detected, relaxing gate".to_string());
    } else if post_worse_ratio < 0.10
        && let Some(suspicious_rate) = stats.suspicious_rate()
        && suspicious_rate > 0.25
    {
        new_similarity = (similarity - 0.01).max(0.85);
        new_novelty = (novelty + 0.01).min(0.30);
        reason = Some("high suspicious rate, tightening gate".to_string());
    }

    if new_similarity == similarity && new_novelty == novelty {
        return Ok(None);
    }

    let mut applied = false;
    if apply {
        applied |= update_env_var(
            &env_path,
            &config_root,
            "CODESCRIBE_STREAM_SIMILARITY",
            &format!("{:.3}", new_similarity),
        )?;
        applied |= update_env_var(
            &env_path,
            &config_root,
            "CODESCRIBE_STREAM_NOVELTY",
            &format!("{:.3}", new_novelty),
        )?;
    }

    let detail = format!(
        "CODESCRIBE_STREAM_SIMILARITY {:.3} -> {:.3}, CODESCRIBE_STREAM_NOVELTY {:.3} -> {:.3} ({})",
        similarity,
        new_similarity,
        novelty,
        new_novelty,
        reason.unwrap_or_else(|| "tuned".into())
    );

    Ok(Some(UpdateAction {
        kind: "gate_thresholds".into(),
        detail,
        applied,
    }))
}

fn propose_embedding_update(
    signals: &QualitySignals,
    stats: &PostprocessStats,
    apply: bool,
) -> Result<Option<UpdateAction>> {
    let Some(embeddings_enabled) = stats.embeddings_enabled else {
        return Ok(None);
    };
    let config_root = Config::config_dir();
    let env_path = config_root.join(".env");

    if !embeddings_enabled {
        if let Some(suspicious_rate) = stats.suspicious_rate()
            && suspicious_rate > 0.20
        {
            let applied = if apply {
                update_env_var(
                    &env_path,
                    &config_root,
                    "CODESCRIBE_STREAM_DISABLE_EMBEDDINGS",
                    "0",
                )?
            } else {
                false
            };
            return Ok(Some(UpdateAction {
                kind: "embeddings".into(),
                detail: "Enable embeddings (suspicious rate high)".into(),
                applied,
            }));
        }
    } else if let Some(post_worse_ratio) = signals.post_worse_ratio
        && post_worse_ratio > 0.40
        && let Some(gate_rate) = stats.gate_drop_rate()
        && gate_rate > 0.40
    {
        let applied = if apply {
            update_env_var(
                &env_path,
                &config_root,
                "CODESCRIBE_STREAM_DISABLE_EMBEDDINGS",
                "1",
            )?
        } else {
            false
        };
        return Ok(Some(UpdateAction {
            kind: "embeddings".into(),
            detail: "Disable embeddings (gate too aggressive)".into(),
            applied,
        }));
    }

    Ok(None)
}

fn propose_prompt_tuning(
    signals: &QualitySignals,
    report: &QualityReport,
    apply: bool,
) -> Result<Option<UpdateAction>> {
    let Some(ai_worse_ratio) = signals.ai_worse_ratio else {
        return Ok(None);
    };
    let Some(avg_ai) = signals.avg_ai_wer else {
        return Ok(None);
    };
    let Some(avg_post) = signals.avg_post_wer else {
        return Ok(None);
    };

    if ai_worse_ratio < 0.30 && avg_ai <= avg_post + DEFAULT_REGRESSION_THRESHOLD {
        return Ok(None);
    }

    let now: DateTime<Local> = Local::now();
    let tuning = format!(
        "# AUTO-TUNING {}\n\
- Preserve original wording; do not paraphrase.\n\
- Keep technical terms and identifiers verbatim.\n\
- If unsure, keep the word as-is.\n\
- Keep bracketed tags like [NIEWYRAZNE] unchanged.\n",
        now.format("%Y-%m-%d %H:%M:%S")
    );

    let config_root = Config::config_dir();
    let prompts_dir = safe_prepare_path(&config_root.join("prompts"), &config_root)?;
    fs::create_dir_all(&prompts_dir)?;
    let path = prompts_dir.join("formatting_tuning.txt");

    let applied = if apply {
        let existing = safe_read_to_string_bounded(&path, &config_root).unwrap_or_default();
        if existing.trim() != tuning.trim() {
            crate::safe_path::safe_write_bounded(&path, &config_root, &tuning)?;
            true
        } else {
            false
        }
    } else {
        false
    };

    Ok(Some(UpdateAction {
        kind: "prompt_tuning".into(),
        detail: format!(
            "formatting_tuning.txt updated (ai_worse_ratio={:.2}, avg_ai_wer={:.3}, avg_post_wer={:.3}, entries={})",
            ai_worse_ratio,
            avg_ai,
            avg_post,
            report.entries.len()
        ),
        applied,
    }))
}

fn propose_lexicon_updates(
    report: &QualityReport,
    max_updates: usize,
    min_count: usize,
    apply: bool,
    source: LexiconSource,
) -> Result<Option<UpdateAction>> {
    let suggestions = extract_lexicon_suggestions(report, max_updates, min_count, source);
    if suggestions.is_empty() {
        return Ok(None);
    }

    let config_root = Config::config_dir();
    let path = safe_prepare_path(&config_root.join("lexicon.custom.jsonl"), &config_root)?;
    let applied = if apply {
        apply_lexicon_suggestions(&path, &config_root, &suggestions)?
    } else {
        false
    };

    let detail = format!(
        "lexicon.custom.jsonl suggestions={} source={} (top: {})",
        suggestions.len(),
        source.as_str(),
        suggestions
            .iter()
            .take(3)
            .map(|s| format!("{}<-{}", s.term, s.mis))
            .collect::<Vec<_>>()
            .join(", ")
    );

    Ok(Some(UpdateAction {
        kind: "lexicon".into(),
        detail,
        applied,
    }))
}

#[derive(Debug)]
struct LexiconSuggestion {
    term: String,
    mis: String,
    count: usize,
}

fn extract_lexicon_suggestions(
    report: &QualityReport,
    max_updates: usize,
    min_count: usize,
    source: LexiconSource,
) -> Vec<LexiconSuggestion> {
    let mut counts: HashMap<(String, String), usize> = HashMap::new();

    for entry in &report.entries {
        let reference = match source {
            LexiconSource::Corpus => entry.transcripts.reference.as_deref(),
            LexiconSource::Cloud => entry.transcripts.cloud.as_deref(),
            LexiconSource::AiFormatted => entry.transcripts.ai_formatted.as_deref(),
        };
        let Some(reference) = reference else {
            continue;
        };
        let Some(raw) = entry.transcripts.raw.as_deref() else {
            continue;
        };

        let ref_tokens = normalize_tokens(reference);
        let raw_tokens = normalize_tokens(raw);
        let subs = align_tokens(&ref_tokens, &raw_tokens);

        for (term, mis) in subs {
            if !token_eligible(&term) || !token_eligible(&mis) {
                continue;
            }
            if term.eq_ignore_ascii_case(&mis) {
                continue;
            }
            // Allow higher distance for Polish morphology (odmiana)
            // dist 4 catches: odpowiedział↔odpowiadał, remote'a↔remontu
            if word_distance(&term, &mis) > 4 {
                continue;
            }

            let key = (term.clone(), mis.clone());
            *counts.entry(key).or_insert(0) += 1;
        }
    }

    let mut suggestions: Vec<LexiconSuggestion> = counts
        .into_iter()
        .filter(|(_, count)| *count >= min_count)
        .map(|((term, mis), count)| LexiconSuggestion { term, mis, count })
        .collect();

    suggestions.sort_by_key(|b| std::cmp::Reverse(b.count));
    if max_updates > 0 && suggestions.len() > max_updates {
        suggestions.truncate(max_updates);
    }
    suggestions
}

#[derive(Debug, Serialize, Deserialize)]
struct LexiconEntry {
    term: String,
    mispronunciations: Vec<String>,
}

fn apply_lexicon_suggestions(
    path: &Path,
    root: &Path,
    suggestions: &[LexiconSuggestion],
) -> Result<bool> {
    let mut entries = read_custom_lexicon(path, root);
    let mut changed = false;

    for suggestion in suggestions {
        let bucket = entries.entry(suggestion.term.clone()).or_default();
        if bucket.insert(suggestion.mis.clone()) {
            changed = true;
        }
    }

    if changed {
        let mut out = String::new();
        let mut keys: Vec<_> = entries.keys().cloned().collect();
        keys.sort();
        for key in keys {
            let mut mis: Vec<_> = entries[&key].iter().cloned().collect();
            mis.sort();
            let entry = LexiconEntry {
                term: key,
                mispronunciations: mis,
            };
            out.push_str(&serde_json::to_string(&entry)?);
            out.push('\n');
        }
        crate::safe_path::safe_write_bounded(path, root, &out)?;
    }

    Ok(changed)
}

fn read_custom_lexicon(path: &Path, root: &Path) -> HashMap<String, HashSet<String>> {
    let mut map: HashMap<String, HashSet<String>> = HashMap::new();
    let content = safe_read_to_string_bounded(path, root).unwrap_or_default();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<LexiconEntry>(trimmed) {
            let bucket = map.entry(entry.term).or_default();
            for mis in entry.mispronunciations {
                bucket.insert(mis);
            }
        }
    }
    map
}

fn normalize_tokens(text: &str) -> Vec<String> {
    let mut normalized = String::with_capacity(text.len());
    for ch in text.to_lowercase().chars() {
        if ch.is_alphanumeric() || ch.is_whitespace() {
            normalized.push(ch);
        } else {
            normalized.push(' ');
        }
    }
    normalized
        .split_whitespace()
        .map(|t| t.to_string())
        .collect()
}

fn token_eligible(token: &str) -> bool {
    if token.len() < 3 {
        return false;
    }
    if token.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    true
}

fn align_tokens(reference: &[String], hypothesis: &[String]) -> Vec<(String, String)> {
    let n = reference.len();
    let m = hypothesis.len();
    let mut dp = vec![vec![0usize; m + 1]; n + 1];

    for (i, row) in dp.iter_mut().enumerate().take(n + 1) {
        row[0] = i;
    }
    for (j, value) in dp[0].iter_mut().enumerate().take(m + 1) {
        *value = j;
    }

    for i in 1..=n {
        for j in 1..=m {
            let cost = if reference[i - 1] == hypothesis[j - 1] {
                0
            } else {
                1
            };
            dp[i][j] = (dp[i - 1][j] + 1)
                .min(dp[i][j - 1] + 1)
                .min(dp[i - 1][j - 1] + cost);
        }
    }

    let mut subs = Vec::new();
    let mut i = n;
    let mut j = m;
    while i > 0 || j > 0 {
        if i > 0 && j > 0 {
            let cost = if reference[i - 1] == hypothesis[j - 1] {
                0
            } else {
                1
            };
            if dp[i][j] == dp[i - 1][j - 1] + cost {
                if cost == 1 {
                    subs.push((reference[i - 1].clone(), hypothesis[j - 1].clone()));
                }
                i -= 1;
                j -= 1;
                continue;
            }
        }
        if i > 0 && dp[i][j] == dp[i - 1][j] + 1 {
            i -= 1;
        } else if j > 0 {
            j -= 1;
        } else {
            break;
        }
    }

    subs
}

fn word_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    levenshtein(&a_chars, &b_chars)
}

fn levenshtein<T: Eq>(a: &[T], b: &[T]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];

    for (i, item_a) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, item_b) in b.iter().enumerate() {
            let cost = if item_a == item_b { 0 } else { 1 };
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        prev.clone_from(&cur);
    }

    prev[b.len()]
}

fn read_env_f32(path: &Path, root: &Path, key: &str, default: f32) -> f32 {
    if let Ok(value) = std::env::var(key)
        && let Ok(parsed) = value.parse::<f32>()
    {
        return parsed;
    }

    if let Some(value) = read_env_value(path, root, key)
        && let Ok(parsed) = value.parse::<f32>()
    {
        return parsed;
    }

    default
}

fn read_env_value(path: &Path, root: &Path, key: &str) -> Option<String> {
    let content = safe_read_to_string_bounded(path, root).ok()?;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }
        let Some((k, v)) = trimmed.split_once('=') else {
            continue;
        };
        if k.trim() == key {
            return Some(v.trim().to_string());
        }
    }
    None
}

fn update_env_var(path: &Path, root: &Path, key: &str, value: &str) -> Result<bool> {
    let mut lines = Vec::new();
    let mut found = false;
    let mut changed = false;
    let target = format!("{}={}", key, value);

    if path.exists() {
        let content = safe_read_to_string_bounded(path, root)?;
        for line in content.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with(&format!("{}=", key)) {
                found = true;
                if line != target {
                    changed = true;
                }
                lines.push(target.clone());
            } else {
                lines.push(line.to_string());
            }
        }
    }

    if !found {
        lines.push(target.clone());
        changed = true;
    }

    if changed {
        let mut output = lines.join("\n");
        output.push('\n');
        crate::safe_path::safe_write_bounded(path, root, &output)?;
    }
    Ok(changed)
}

// ============================================================================
// Quality Daemon State (for tray integration)
// ============================================================================

/// Daemon state stored in qube_daemon.json
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QubeDaemonState {
    pub pending_mismatches: usize,
    #[serde(default)]
    pub last_check: String,
    pub latest_report: Option<String>,
    #[serde(default = "default_daemon_available")]
    pub available: bool,
}

fn default_daemon_available() -> bool {
    true
}

/// Get path to daemon state file
pub fn daemon_state_path() -> PathBuf {
    Config::config_dir().join("qube_daemon.json")
}

fn daemon_history_path() -> PathBuf {
    Config::config_dir()
        .join("reports")
        .join("quality_history.jsonl")
}

fn read_latest_report_from_history(path: &Path, root: &Path) -> Option<String> {
    #[derive(Deserialize)]
    struct DaemonHistoryEntry {
        report_dir: String,
    }

    let content = safe_read_to_string_bounded(path, root).ok()?;
    content
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .and_then(|line| serde_json::from_str::<DaemonHistoryEntry>(line).ok())
        .map(|entry| entry.report_dir)
}

fn write_daemon_state_file(path: &Path, root: &Path, state: &QubeDaemonState) -> Result<()> {
    fs::create_dir_all(root)
        .with_context(|| format!("Failed to create config directory {}", root.display()))?;
    let root_canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let target_path = path
        .strip_prefix(root)
        .map(|relative| root_canon.join(relative))
        .unwrap_or_else(|_| path.to_path_buf());
    let json = serde_json::to_string_pretty(state)?;
    crate::safe_path::safe_write_bounded(&target_path, &root_canon, &json)
        .with_context(|| format!("Failed to write daemon state {}", target_path.display()))
}

fn write_daemon_state_with_paths(
    state_path: &Path,
    history_path: &Path,
    config_root: &Path,
    pending_mismatches: usize,
    available: bool,
) -> Result<QubeDaemonState> {
    let state = QubeDaemonState {
        pending_mismatches,
        last_check: Local::now().to_rfc3339(),
        latest_report: read_latest_report_from_history(history_path, config_root),
        available,
    };

    write_daemon_state_file(state_path, config_root, &state)?;
    Ok(state)
}

/// Write daemon state using the canonical quality_history.jsonl contract.
pub fn write_daemon_state(pending_mismatches: usize) -> Result<QubeDaemonState> {
    let config_root = Config::config_dir();
    let state_path = daemon_state_path();
    let history_path = daemon_history_path();
    write_daemon_state_with_paths(
        &state_path,
        &history_path,
        &config_root,
        pending_mismatches,
        true,
    )
}

/// Read daemon state from file
pub fn read_daemon_state() -> QubeDaemonState {
    let path = daemon_state_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return QubeDaemonState::default(),
    };

    serde_json::from_str(&content).unwrap_or_default()
}

/// Get pending mismatch count from daemon state
pub fn get_pending_mismatches() -> usize {
    read_daemon_state().pending_mismatches
}

/// Get path to the latest HTML report
pub fn get_latest_report_html() -> Option<PathBuf> {
    let state = read_daemon_state();
    state
        .latest_report
        .map(|dir| PathBuf::from(dir).join("index.html"))
}

/// Open the latest quality report in default browser
pub fn open_latest_report() -> bool {
    if let Some(html_path) = get_latest_report_html()
        && html_path.exists()
    {
        return std::process::Command::new("open")
            .arg(&html_path)
            .spawn()
            .is_ok();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::qube_report::{
        ReportEntry, ReportEnvironment, ReportMetrics, ReportSummary, ReportTranscripts,
    };
    use crate::stream_postprocess::StreamPostProcessStats;

    fn mock_environment() -> ReportEnvironment {
        ReportEnvironment {
            stt_endpoint: None,
            stt_api_key_present: false,
            llm_formatting_endpoint: None,
            llm_formatting_model: None,
            llm_formatting_key_present: false,
            local_model: None,
            whisper_language: None,
            metrics_reference: "corpus".into(),
        }
    }

    fn mock_entry(id: &str) -> ReportEntry {
        ReportEntry {
            id: id.to_string(),
            audio_path: format!("/tmp/{}.wav", id),
            audio_rel_path: format!("{}.wav", id),
            reference_path: None,
            duration_secs: 5.0,
            transcripts: ReportTranscripts::default(),
            raw_semantics: None,
            metrics: ReportMetrics::default(),
            postprocess_stats: None,
            errors: vec![],
        }
    }

    fn mock_report(entries: Vec<ReportEntry>) -> QualityReport {
        QualityReport {
            generated_at: "2026-01-23T12:00:00+01:00".into(),
            environment: mock_environment(),
            summary: ReportSummary::default(),
            entries,
        }
    }

    #[test]
    fn test_write_daemon_state_with_paths_uses_latest_history_entry() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let root = tmp.path().canonicalize().expect("canonical root");
        let history_path = root.join("reports").join("quality_history.jsonl");
        let state_path = root.join("qube_daemon.json");
        std::fs::create_dir_all(history_path.parent().expect("history parent"))
            .expect("create reports dir");

        let older = serde_json::json!({
            "report_dir": "/tmp/quality_old",
            "report_json": "/tmp/quality_old/report.json"
        });
        let latest = serde_json::json!({
            "report_dir": "/tmp/quality_latest",
            "report_json": "/tmp/quality_latest/report.json"
        });
        std::fs::write(&history_path, format!("{older}\n{latest}\n")).expect("write history");

        let state = write_daemon_state_with_paths(&state_path, &history_path, &root, 7, true)
            .expect("write daemon state");

        assert_eq!(state.pending_mismatches, 7);
        assert_eq!(state.latest_report.as_deref(), Some("/tmp/quality_latest"));
        assert!(state.available);
        assert!(!state.last_check.is_empty());

        let persisted = std::fs::read_to_string(&state_path).expect("read daemon state file");
        let loaded: QubeDaemonState =
            serde_json::from_str(&persisted).expect("parse daemon state file");
        assert_eq!(loaded.pending_mismatches, 7);
        assert_eq!(loaded.latest_report.as_deref(), Some("/tmp/quality_latest"));
        assert!(loaded.available);
    }

    #[test]
    fn test_write_daemon_state_with_paths_tolerates_invalid_history() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let root = tmp.path().canonicalize().expect("canonical root");
        let history_path = root.join("reports").join("quality_history.jsonl");
        let state_path = root.join("qube_daemon.json");
        std::fs::create_dir_all(history_path.parent().expect("history parent"))
            .expect("create reports dir");
        std::fs::write(&history_path, "{invalid json}\n").expect("write invalid history");

        let state = write_daemon_state_with_paths(&state_path, &history_path, &root, 2, true)
            .expect("write daemon state");
        assert_eq!(state.pending_mismatches, 2);
        assert_eq!(state.latest_report, None);
        assert!(state.available);
    }

    #[test]
    fn test_quality_daemon_state_backward_compatible_defaults() {
        let raw = r#"{
            "pending_mismatches": 4,
            "last_check": "2026-02-01T10:00:00+01:00",
            "latest_report": "/tmp/quality_latest"
        }"#;

        let state: QubeDaemonState = serde_json::from_str(raw).expect("parse daemon state");
        assert_eq!(state.pending_mismatches, 4);
        assert_eq!(state.latest_report.as_deref(), Some("/tmp/quality_latest"));
        assert!(state.available);
    }

    // ─── normalize_tokens ────────────────────────────────────────────

    #[test]
    fn test_normalize_tokens_basic() {
        let tokens = normalize_tokens("Hello World");
        assert_eq!(tokens, vec!["hello", "world"]);
    }

    #[test]
    fn test_normalize_tokens_punctuation_to_space() {
        let tokens = normalize_tokens("CodeScribe's test-case, version 2.0!");
        assert_eq!(
            tokens,
            vec!["codescribe", "s", "test", "case", "version", "2", "0"]
        );
    }

    #[test]
    fn test_normalize_tokens_polish_diacritics() {
        let tokens = normalize_tokens("Źródło działania systemu");
        assert_eq!(tokens, vec!["źródło", "działania", "systemu"]);
    }

    #[test]
    fn test_normalize_tokens_extra_whitespace() {
        let tokens = normalize_tokens("  foo   bar  \n baz  ");
        assert_eq!(tokens, vec!["foo", "bar", "baz"]);
    }

    // ─── token_eligible ──────────────────────────────────────────────

    #[test]
    fn test_token_eligible_too_short() {
        assert!(!token_eligible("ab"));
        assert!(!token_eligible("x"));
    }

    #[test]
    fn test_token_eligible_all_digits() {
        assert!(!token_eligible("123"));
        assert!(!token_eligible("007"));
    }

    #[test]
    fn test_token_eligible_valid() {
        assert!(token_eligible("foo"));
        assert!(token_eligible("abc123"));
        assert!(token_eligible("źródło"));
    }

    // ─── word_distance / levenshtein ─────────────────────────────────

    #[test]
    fn test_word_distance_identical() {
        assert_eq!(word_distance("hello", "hello"), 0);
    }

    #[test]
    fn test_word_distance_one_sub() {
        assert_eq!(word_distance("cat", "bat"), 1);
    }

    #[test]
    fn test_word_distance_insertion_deletion() {
        assert_eq!(word_distance("kitten", "sitting"), 3);
    }

    #[test]
    fn test_word_distance_polish_morphology() {
        // odpowiedział vs odpowiadał - should be within 4
        assert!(word_distance("odpowiedział", "odpowiadał") <= 4);
        // remote'a vs remontu - within 4
        assert!(word_distance("remontu", "remotea") <= 4);
    }

    #[test]
    fn test_word_distance_completely_different() {
        assert!(word_distance("python", "javascript") > 4);
    }

    // ─── align_tokens ────────────────────────────────────────────────

    #[test]
    fn test_align_tokens_identical() {
        let ref_tokens = vec!["ala".into(), "ma".into(), "kota".into()];
        let hyp_tokens = vec!["ala".into(), "ma".into(), "kota".into()];
        let subs = align_tokens(&ref_tokens, &hyp_tokens);
        assert!(subs.is_empty());
    }

    #[test]
    fn test_align_tokens_single_substitution() {
        let ref_tokens = vec!["ala".into(), "ma".into(), "kota".into()];
        let hyp_tokens = vec!["ala".into(), "ma".into(), "psa".into()];
        let subs = align_tokens(&ref_tokens, &hyp_tokens);
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0], ("kota".to_string(), "psa".to_string()));
    }

    #[test]
    fn test_align_tokens_multiple_substitutions() {
        let ref_tokens = vec!["system".into(), "działa".into(), "poprawnie".into()];
        let hyp_tokens = vec!["system".into(), "działa".into(), "niepoprawne".into()];
        let subs = align_tokens(&ref_tokens, &hyp_tokens);
        assert_eq!(subs.len(), 1);
        assert_eq!(
            subs[0],
            ("poprawnie".to_string(), "niepoprawne".to_string())
        );
    }

    #[test]
    fn test_align_tokens_insertion_not_a_substitution() {
        // Insertion: hypothesis has extra word - NOT counted as substitution
        let ref_tokens = vec!["ala".into(), "kota".into()];
        let hyp_tokens = vec!["ala".into(), "ma".into(), "kota".into()];
        let subs = align_tokens(&ref_tokens, &hyp_tokens);
        assert!(subs.is_empty());
    }

    // ─── compare_metric ──────────────────────────────────────────────

    #[test]
    fn test_compare_metric_regression() {
        let mut regressions = Vec::new();
        let mut improvements = 0usize;
        compare_metric(
            "e1",
            "raw_wer",
            Some(0.15),
            Some(0.10),
            0.02,
            &mut regressions,
            &mut improvements,
        );
        assert_eq!(regressions.len(), 1);
        assert_eq!(regressions[0].metric, "raw_wer");
        assert_eq!(improvements, 0);
    }

    #[test]
    fn test_compare_metric_improvement() {
        let mut regressions = Vec::new();
        let mut improvements = 0usize;
        compare_metric(
            "e1",
            "raw_wer",
            Some(0.05),
            Some(0.15),
            0.02,
            &mut regressions,
            &mut improvements,
        );
        assert!(regressions.is_empty());
        assert_eq!(improvements, 1);
    }

    #[test]
    fn test_compare_metric_within_threshold() {
        let mut regressions = Vec::new();
        let mut improvements = 0usize;
        compare_metric(
            "e1",
            "raw_wer",
            Some(0.11),
            Some(0.10),
            0.02,
            &mut regressions,
            &mut improvements,
        );
        assert!(regressions.is_empty());
        assert_eq!(improvements, 0);
    }

    #[test]
    fn test_compare_metric_none_values_skip() {
        let mut regressions = Vec::new();
        let mut improvements = 0usize;
        compare_metric(
            "e1",
            "raw_wer",
            None,
            Some(0.10),
            0.02,
            &mut regressions,
            &mut improvements,
        );
        compare_metric(
            "e2",
            "raw_wer",
            Some(0.10),
            None,
            0.02,
            &mut regressions,
            &mut improvements,
        );
        assert!(regressions.is_empty());
        assert_eq!(improvements, 0);
    }

    // ─── QualitySignals::from_report ─────────────────────────────────

    #[test]
    fn test_quality_signals_post_worse_ratio() {
        let mut entries = vec![];
        // 3 entries: post worse in 2 of them
        for i in 0..3 {
            let mut entry = mock_entry(&format!("e{}", i));
            entry.metrics.raw_wer = Some(0.10);
            entry.metrics.post_wer = if i < 2 { Some(0.15) } else { Some(0.08) };
            entries.push(entry);
        }
        let report = mock_report(entries);
        let signals = QualitySignals::from_report(&report, 0.02);
        // 2 out of 3 are worse
        let ratio = signals.post_worse_ratio.unwrap();
        assert!((ratio - 2.0 / 3.0).abs() < 0.01);
    }

    #[test]
    fn test_quality_signals_empty_report() {
        let report = mock_report(vec![]);
        let signals = QualitySignals::from_report(&report, 0.02);
        assert!(signals.post_worse_ratio.is_none());
        assert!(signals.ai_worse_ratio.is_none());
    }

    // ─── PostprocessStats::from_report ───────────────────────────────

    #[test]
    fn test_postprocess_stats_aggregation() {
        let mut entries = vec![];
        for i in 0..3 {
            let mut entry = mock_entry(&format!("e{}", i));
            entry.postprocess_stats = Some(StreamPostProcessStats {
                input_chunks: 10,
                output_chunks: 8,
                dropped_chunks: 2,
                gate_drops: 1,
                suspicious_chunks: 2,
                lexicon_rewrites: 3,
                repetition_cleanups: 1,
                embeddings_enabled: true,
            });
            entries.push(entry);
        }
        let report = mock_report(entries);
        let stats = PostprocessStats::from_report(&report);
        assert_eq!(stats.input_chunks, 30);
        assert_eq!(stats.gate_drops, 3);
        assert_eq!(stats.suspicious, 6);
        assert_eq!(stats.embeddings_enabled, Some(true));
    }

    #[test]
    fn test_postprocess_stats_gate_drop_rate() {
        let mut entry = mock_entry("e1");
        entry.postprocess_stats = Some(StreamPostProcessStats {
            input_chunks: 100,
            gate_drops: 25,
            ..Default::default()
        });
        let report = mock_report(vec![entry]);
        let stats = PostprocessStats::from_report(&report);
        let rate = stats.gate_drop_rate().unwrap();
        assert!((rate - 0.25).abs() < 0.001);
    }

    #[test]
    fn test_postprocess_stats_no_entries() {
        let report = mock_report(vec![]);
        let stats = PostprocessStats::from_report(&report);
        assert!(stats.gate_drop_rate().is_none());
        assert!(stats.suspicious_rate().is_none());
    }

    // ─── extract_lexicon_suggestions ─────────────────────────────────

    #[test]
    fn test_extract_lexicon_suggestions_finds_mismatches() {
        let mut entries = vec![];
        // Same substitution in 3 entries → count=3, passes min_count=2
        for i in 0..3 {
            let mut entry = mock_entry(&format!("e{}", i));
            entry.transcripts.reference = Some("system działa poprawnie".into());
            entry.transcripts.raw = Some("system działa paprawnie".into());
            entries.push(entry);
        }
        let report = mock_report(entries);
        let suggestions = extract_lexicon_suggestions(&report, 10, 2, LexiconSource::Corpus);
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].term, "poprawnie");
        assert_eq!(suggestions[0].mis, "paprawnie");
        assert_eq!(suggestions[0].count, 3);
    }

    #[test]
    fn test_extract_lexicon_suggestions_respects_min_count() {
        let mut entry = mock_entry("e1");
        entry.transcripts.reference = Some("system działa poprawnie".into());
        entry.transcripts.raw = Some("system działa paprawnie".into());
        let report = mock_report(vec![entry]);
        // min_count=2, but only 1 occurrence
        let suggestions = extract_lexicon_suggestions(&report, 10, 2, LexiconSource::Corpus);
        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_extract_lexicon_suggestions_respects_max_updates() {
        let mut entries = vec![];
        for i in 0..5 {
            let mut entry = mock_entry(&format!("e{}", i));
            entry.transcripts.reference = Some("alfa beta gamma delta".into());
            entry.transcripts.raw = Some("alfe bete gamme delte".into());
            entries.push(entry);
        }
        let report = mock_report(entries);
        let suggestions = extract_lexicon_suggestions(&report, 2, 1, LexiconSource::Corpus);
        assert_eq!(suggestions.len(), 2);
    }

    #[test]
    fn test_extract_lexicon_suggestions_filters_high_distance() {
        let mut entries = vec![];
        for i in 0..3 {
            let mut entry = mock_entry(&format!("e{}", i));
            // "python" vs "javascript" - distance > 4, should be filtered
            entry.transcripts.reference = Some("programuję python codziennie".into());
            entry.transcripts.raw = Some("programuję javascript codziennie".into());
            entries.push(entry);
        }
        let report = mock_report(entries);
        let suggestions = extract_lexicon_suggestions(&report, 10, 1, LexiconSource::Corpus);
        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_extract_lexicon_suggestions_cloud_source() {
        let mut entries = vec![];
        for i in 0..3 {
            let mut entry = mock_entry(&format!("e{}", i));
            // "transkrypcja" vs "transkrypsja" - distance=1, well within 4
            entry.transcripts.cloud = Some("dokładna transkrypcja audio".into());
            entry.transcripts.raw = Some("dokładna transkrypsja audio".into());
            entries.push(entry);
        }
        let report = mock_report(entries);
        let suggestions = extract_lexicon_suggestions(&report, 10, 2, LexiconSource::Cloud);
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].term, "transkrypcja");
        assert_eq!(suggestions[0].mis, "transkrypsja");
    }

    #[test]
    fn test_extract_lexicon_case_insensitive_skips_same_word() {
        let mut entries = vec![];
        for i in 0..3 {
            let mut entry = mock_entry(&format!("e{}", i));
            entry.transcripts.reference = Some("System działa".into());
            entry.transcripts.raw = Some("system działa".into());
            entries.push(entry);
        }
        let report = mock_report(entries);
        let suggestions = extract_lexicon_suggestions(&report, 10, 1, LexiconSource::Corpus);
        // "system" vs "system" after normalize should be identical, no substitution
        assert!(suggestions.is_empty());
    }

    // ─── analyze_regressions ─────────────────────────────────────────

    #[test]
    fn test_analyze_regressions_no_baseline() {
        let report = mock_report(vec![mock_entry("e1")]);
        let (regressions, summary) = analyze_regressions(&report, None, 0.02);
        assert!(regressions.is_empty());
        assert_eq!(summary.compared_entries, 0);
    }

    #[test]
    fn test_analyze_regressions_detects_wer_regression() {
        let mut current_entry = mock_entry("e1");
        current_entry.metrics.raw_wer = Some(0.20);
        let current = mock_report(vec![current_entry]);

        let mut base_entry = mock_entry("e1");
        base_entry.metrics.raw_wer = Some(0.10);
        let baseline = mock_report(vec![base_entry]);

        let (regressions, summary) = analyze_regressions(&current, Some(&baseline), 0.02);
        assert!(!regressions.is_empty());
        assert_eq!(summary.compared_entries, 1);
        assert_eq!(summary.regression_count, regressions.len());
    }

    // ─── LexiconSource ──────────────────────────────────────────────

    #[test]
    fn test_lexicon_source_as_str() {
        assert_eq!(LexiconSource::Corpus.as_str(), "corpus");
        assert_eq!(LexiconSource::Cloud.as_str(), "cloud");
        assert_eq!(LexiconSource::AiFormatted.as_str(), "ai");
    }

    // ─── render_analysis_markdown ────────────────────────────────────

    #[test]
    fn test_render_analysis_markdown_contains_sections() {
        let analysis = LoopAnalysis {
            generated_at: "2026-01-23T12:00:00".into(),
            current_report: "/tmp/report.json".into(),
            baseline_report: Some("/tmp/baseline.json".into()),
            summary: LoopSummary {
                total_entries: 10,
                compared_entries: 8,
                regression_count: 2,
                improvement_count: 3,
                post_worse_ratio: Some(0.25),
                ai_worse_ratio: None,
                gate_drop_rate: Some(0.10),
                suspicious_rate: None,
            },
            regressions: vec![RegressionFinding {
                id: "e1".into(),
                metric: "raw_wer".into(),
                current: 0.20,
                baseline: 0.10,
                delta: 0.10,
            }],
            updates: vec![UpdateAction {
                kind: "lexicon".into(),
                detail: "added 3 rules".into(),
                applied: true,
            }],
        };
        let md = render_analysis_markdown(&analysis);
        assert!(md.contains("# CodeScribe Quality Loop Analysis"));
        assert!(md.contains("Baseline report"));
        assert!(md.contains("## Regressions"));
        assert!(md.contains("## Updates"));
        assert!(md.contains("raw_wer"));
        assert!(md.contains("lexicon"));
    }
}
