//! Quality report pipeline for batch audio evaluation.
//!
//! Flow: batch WAV -> single-pass transcription (raw + postprocess) -> AI formatting + cloud ref
//! -> metrics -> artifacts + HTML/JSON/MD reports.

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use tracing::info;

use crate::ai_formatting;
use crate::audio::load_audio_file;
use crate::client;
use crate::config::Config;
use crate::pipeline::contracts::RawTranscript;
use crate::safe_path::{
    safe_canonicalize_bounded, safe_copy_bounded, safe_prepare_path, safe_read_to_string_bounded,
    safe_symlink_or_copy_bounded, safe_write_bounded,
};
use crate::state::conversation::{AiMode, reset_conversation_for_mode};
use crate::stream_postprocess::{StreamPostProcessStats, StreamPostProcessor};
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;

const AI_LOG_PREVIEW_CHARS: usize = 80;

#[derive(Debug, Clone)]
pub struct QualityReportConfig {
    pub input_dir: PathBuf,
    pub output_dir: PathBuf,
    pub date_filter: Option<String>,
    pub limit: usize,
    pub language: Option<String>,
    pub skip_cloud: bool,
    pub cloud_concurrency: usize,
    pub skip_formatting: bool,
    pub debug_mode: bool,
    pub copy_audio: bool,
    pub metrics_reference: MetricsReference,
    pub local_transcription: LocalTranscriptionMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MetricsReference {
    Corpus,
    Cloud,
    /// AI-formatted transcript (Whisper + LLM correction)
    AiFormatted,
}

impl MetricsReference {
    fn as_str(self) -> &'static str {
        match self {
            MetricsReference::Corpus => "corpus",
            MetricsReference::Cloud => "cloud",
            MetricsReference::AiFormatted => "ai",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalTranscriptionMode {
    LocalWhisper,
    CodeScribeIpc,
}

impl LocalTranscriptionMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::LocalWhisper => "local_whisper",
            Self::CodeScribeIpc => "codescribe_ipc",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct QualityReport {
    pub generated_at: String,
    pub environment: ReportEnvironment,
    pub summary: ReportSummary,
    pub entries: Vec<ReportEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReportEnvironment {
    pub stt_endpoint: Option<String>,
    pub stt_api_key_present: bool,
    pub llm_formatting_endpoint: Option<String>,
    pub llm_formatting_model: Option<String>,
    pub llm_formatting_key_present: bool,
    pub local_model: Option<String>,
    pub whisper_language: Option<String>,
    pub metrics_reference: String,
    pub local_transcription: String,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct ReportSummary {
    pub total_files: usize,
    pub processed_files: usize,
    pub avg_raw_wer: Option<f32>,
    pub avg_post_wer: Option<f32>,
    pub avg_ai_wer: Option<f32>,
    pub avg_cloud_wer: Option<f32>,
    pub avg_raw_cer: Option<f32>,
    pub avg_post_cer: Option<f32>,
    pub avg_ai_cer: Option<f32>,
    pub avg_cloud_cer: Option<f32>,
    pub raw_no_speech_detected: usize,
    pub raw_quality_gate_dropped: usize,
    pub raw_text_committed: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportTranscriptState {
    TextCommitted,
    QualityGateDropped,
    NoSpeechDetected,
    EmptyTranscript,
}

impl std::fmt::Display for ReportTranscriptState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TextCommitted => write!(f, "text_committed"),
            Self::QualityGateDropped => write!(f, "quality_gate_dropped"),
            Self::NoSpeechDetected => write!(f, "no_speech_detected"),
            Self::EmptyTranscript => write!(f, "empty_transcript"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportTranscriptSemantics {
    pub state: ReportTranscriptState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReportEntry {
    pub id: String,
    pub audio_path: String,
    pub audio_rel_path: String,
    pub reference_path: Option<String>,
    pub duration_secs: f32,
    pub transcripts: ReportTranscripts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_semantics: Option<ReportTranscriptSemantics>,
    pub metrics: ReportMetrics,
    pub postprocess_stats: Option<StreamPostProcessStats>,
    pub errors: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct ReportTranscripts {
    pub raw: Option<String>,
    pub post: Option<String>,
    pub ai_formatted: Option<String>,
    pub cloud: Option<String>,
    pub reference: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct ReportMetrics {
    pub raw_wer: Option<f32>,
    pub raw_cer: Option<f32>,
    pub post_wer: Option<f32>,
    pub post_cer: Option<f32>,
    pub ai_wer: Option<f32>,
    pub ai_cer: Option<f32>,
    pub cloud_wer: Option<f32>,
    pub cloud_cer: Option<f32>,
}

enum CloudJobSet {
    Disabled,
    Skipped(String),
    Running(HashMap<String, JoinHandle<Result<crate::client::CloudTranscriptionVerdict>>>),
}

impl CloudJobSet {
    async fn take_for(&mut self, id: &str, errors: &mut Vec<String>) -> Option<String> {
        match self {
            CloudJobSet::Disabled => None,
            CloudJobSet::Skipped(reason) => {
                errors.push(reason.clone());
                None
            }
            CloudJobSet::Running(jobs) => match jobs.remove(id) {
                Some(handle) => match handle.await {
                    Ok(Ok(verdict)) => Some(verdict.text),
                    Ok(Err(e)) => {
                        errors.push(format!("Cloud transcription failed: {}", e));
                        None
                    }
                    Err(e) => {
                        errors.push(format!("Cloud transcription task failed: {}", e));
                        None
                    }
                },
                None => {
                    errors.push("Cloud transcription missing for entry".into());
                    None
                }
            },
        }
    }
}

fn classify_raw_semantics(
    transcript: Option<&RawTranscript>,
    no_speech_reason: Option<&str>,
) -> Option<ReportTranscriptSemantics> {
    let transcript = transcript?;
    let trimmed = transcript.text.trim();

    if !trimmed.is_empty() {
        return Some(ReportTranscriptSemantics {
            state: ReportTranscriptState::TextCommitted,
            reason: None,
        });
    }

    if let Some(reason) = no_speech_reason {
        return Some(ReportTranscriptSemantics {
            state: ReportTranscriptState::NoSpeechDetected,
            reason: Some(reason.to_string()),
        });
    }

    if transcript.quality_gate_dropped {
        return Some(ReportTranscriptSemantics {
            state: ReportTranscriptState::QualityGateDropped,
            reason: Some("quality_gate_dropped".to_string()),
        });
    }

    Some(ReportTranscriptSemantics {
        state: ReportTranscriptState::EmptyTranscript,
        reason: None,
    })
}

pub async fn run(config: QualityReportConfig) -> Result<PathBuf> {
    let now: DateTime<Local> = Local::now();
    let generated_at = now.to_rfc3339();

    let env_snapshot = snapshot_environment(config.metrics_reference, config.local_transcription);

    let config_root = Config::config_dir();
    let input_root = resolve_input_root(&config.input_dir, &config_root)?;
    let output_root = resolve_output_root(&config.output_dir, &config_root)?;

    let pairs = collect_pairs(&input_root, config.date_filter.as_deref(), config.limit);
    if pairs.is_empty() {
        bail!("No WAV+TXT pairs found in {}", input_root.display());
    }

    fs::create_dir_all(&output_root)
        .with_context(|| format!("Failed to create {}", output_root.display()))?;
    let artifacts_dir = output_root.join("artifacts");
    let audio_dir = output_root.join("audio");
    fs::create_dir_all(&artifacts_dir)?;
    fs::create_dir_all(&audio_dir)?;

    if config.local_transcription == LocalTranscriptionMode::LocalWhisper {
        crate::stt::init_active_engine()
            .context("Failed to init active STT engine via core::stt")?;
    } else {
        info!("Local Whisper init skipped: quality report uses CodeScribe IPC transcription");
    }

    // Resume: skip pairs that already have artifacts.
    //
    // Note: daemon mode sets `skip_formatting=true`, so `.ai.txt` is not created.
    // Use a `.done` marker (and a raw+reference fallback) so resume works in all modes.
    let total_before = pairs.len();
    let pairs: Vec<_> = pairs
        .into_iter()
        .filter(|pair| {
            let done_artifact = artifacts_dir.join(format!("{}.done", pair.id));
            if done_artifact.exists() {
                info!("[RESUME] Skipping already-processed: {}", pair.id);
                false
            } else {
                let raw_artifact = artifacts_dir.join(format!("{}.raw.txt", pair.id));
                let ref_artifact = artifacts_dir.join(format!("{}.reference.txt", pair.id));
                if raw_artifact.exists() && ref_artifact.exists() {
                    info!("[RESUME] Skipping already-processed (legacy): {}", pair.id);
                    false
                } else {
                    true
                }
            }
        })
        .collect();
    let skipped = total_before - pairs.len();
    if skipped > 0 {
        info!(
            "[RESUME] Processing {} pairs ({} skipped as already done)",
            pairs.len(),
            skipped
        );
    }

    let mut entries = Vec::new();
    let mut totals = Totals::default();
    let mut cloud_jobs = prepare_cloud_jobs(&pairs, &config, &input_root);

    for pair in &pairs {
        let entry = process_pair(
            pair,
            &config,
            &input_root,
            &output_root,
            &artifacts_dir,
            &audio_dir,
            &mut cloud_jobs,
        )
        .await?;
        totals.accumulate(&entry);
        entries.push(entry);
    }

    let summary = totals.finish(entries.len());
    let report = QualityReport {
        generated_at,
        environment: env_snapshot,
        summary,
        entries,
    };

    write_report_files(&report, &config, &artifacts_dir, &output_root)?;

    Ok(output_root)
}

fn prepare_cloud_jobs(
    pairs: &[CorpusPair],
    config: &QualityReportConfig,
    input_root: &Path,
) -> CloudJobSet {
    if config.skip_cloud {
        return CloudJobSet::Disabled;
    }

    let app_config = Config::load();
    let (endpoint, api_key) = match cloud_reference_credentials(&app_config) {
        Some(credentials) => credentials,
        _ => {
            return CloudJobSet::Skipped(
                "Cloud transcription skipped: STT_ENDPOINT/STT_API_KEY missing".into(),
            );
        }
    };

    let total = pairs.len().max(1);
    let max_concurrency = if config.cloud_concurrency == 0 {
        total
    } else {
        config.cloud_concurrency.max(1)
    };
    let semaphore = std::sync::Arc::new(Semaphore::new(max_concurrency));
    let mut jobs = HashMap::new();

    for pair in pairs {
        let id = pair.id.clone();
        let audio_path = pair.audio_path.clone();
        let input_root = input_root.to_path_buf();
        let language = config.language.clone();
        let permitter = semaphore.clone();

        let endpoint = endpoint.clone();
        let api_key = api_key.clone();
        let handle = tokio::spawn(async move {
            let _permit = permitter
                .acquire_owned()
                .await
                .map_err(|e| anyhow!("Cloud concurrency closed: {}", e))?;
            let audio_canon =
                safe_canonicalize_bounded(&audio_path, &input_root).with_context(|| {
                    format!("Audio path escapes input root: {}", audio_path.display())
                })?;
            client::transcribe_cloud(&audio_canon, language.as_deref(), &endpoint, &api_key).await
        });

        jobs.insert(id, handle);
    }

    CloudJobSet::Running(jobs)
}

fn cloud_reference_credentials(app_config: &Config) -> Option<(String, String)> {
    let endpoint = app_config.stt_endpoint.as_deref()?.trim();
    let api_key = app_config.stt_api_key.as_deref()?.trim();

    if endpoint.is_empty() || api_key.is_empty() {
        return None;
    }

    Some((endpoint.to_string(), api_key.to_string()))
}

async fn process_pair(
    pair: &CorpusPair,
    config: &QualityReportConfig,
    input_root: &Path,
    output_root: &Path,
    artifacts_dir: &Path,
    audio_dir: &Path,
    cloud_jobs: &mut CloudJobSet,
) -> Result<ReportEntry> {
    let audio_path = pair.audio_path.clone();
    let reference_path = pair.reference_path.clone();
    let id = pair.id.clone();

    let audio_canon = safe_canonicalize_bounded(&audio_path, input_root)
        .with_context(|| format!("Audio path escapes input root: {}", audio_path.display()))?;
    let reference_canon = if reference_path.exists() {
        Some(
            safe_canonicalize_bounded(&reference_path, input_root).with_context(|| {
                format!(
                    "Reference path escapes input root: {}",
                    reference_path.display()
                )
            })?,
        )
    } else {
        None
    };

    let audio_rel_path = ensure_audio_asset(
        &audio_canon,
        audio_dir,
        &id,
        input_root,
        output_root,
        config.copy_audio,
    )?;

    let mut errors = Vec::new();
    let reference = if let Some(reference_canon) = reference_canon.as_ref() {
        match safe_read_to_string_bounded(reference_canon, input_root) {
            Ok(content) => {
                let trimmed = content.trim().to_string();
                if trimmed.is_empty() {
                    errors.push("Reference transcript is empty".into());
                    None
                } else {
                    Some(trimmed)
                }
            }
            Err(e) => {
                errors.push(format!("Reference read failed: {}", e));
                None
            }
        }
    } else {
        errors.push("Reference transcript missing".into());
        None
    };

    let (samples, sample_rate) = load_audio_file(&audio_canon)
        .with_context(|| format!("Failed to load audio {}", audio_canon.display()))?;
    let duration_secs = samples.len() as f32 / sample_rate as f32;
    let (_speech_only, vad_stats) = crate::vad::extract_speech(&samples, sample_rate);

    let raw_transcript =
        transcribe_raw_for_report(&audio_canon, &samples, sample_rate, config, &mut errors).await;
    let raw_semantics = classify_raw_semantics(
        raw_transcript.as_ref(),
        vad_stats.no_speech_reason.as_deref(),
    );
    let raw = raw_transcript
        .as_ref()
        .map(|transcript| transcript.text.trim().to_string())
        .filter(|text| !text.is_empty());
    if raw_semantics
        .as_ref()
        .is_some_and(|semantics| semantics.state == ReportTranscriptState::EmptyTranscript)
    {
        errors.push("Raw transcript is empty".into());
    }

    // Post = raw + lexicon/cleanup (single pass through postprocessor)
    let mut postprocessor = StreamPostProcessor::new();
    let post = raw
        .as_deref()
        .and_then(|raw_text| postprocessor.process(raw_text));
    if post
        .as_ref()
        .map(|text| text.trim().is_empty())
        .unwrap_or(raw.is_some() && post.is_none())
    {
        errors.push("Postprocess transcript is empty".into());
    }

    let ai_formatted = if config.skip_formatting {
        None
    } else if !ai_formatting::is_formatting_available() {
        errors.push("AI formatting skipped: missing endpoint/model/key".into());
        None
    } else if let Some(post_text) = post.as_deref() {
        // Reset conversation chain — batch mode must NOT chain between files
        reset_conversation_for_mode(AiMode::Formatting);
        let ai_result =
            ai_formatting::format_text(post_text, config.language.as_deref(), false).await;
        info!(
            "[AI_LOG] id={} input_len={} output_len={} input_preview={:?} output_preview={:?}",
            id,
            post_text.len(),
            ai_result.len(),
            preview_for_log(post_text, AI_LOG_PREVIEW_CHARS),
            preview_for_log(&ai_result, AI_LOG_PREVIEW_CHARS)
        );
        Some(ai_result)
    } else {
        None
    };

    // Protected-vocabulary audit: flag operator/tool/agent names that survived
    // the post-lexicon transcript but were dropped or mutated by the AI pass.
    // This makes technical-name corruption visible to the operator instead of
    // silently shipping "plausible prose" that lost the intended terms.
    if let (Some(post_text), Some(ai_text)) = (post.as_deref(), ai_formatted.as_deref()) {
        let lost = crate::stream_postprocess::protected_terms_lost(post_text, ai_text);
        if !lost.is_empty() {
            errors.push(format!(
                "Protected terms lost in AI formatting: {}",
                lost.join(", ")
            ));
        }
    }

    let cloud = cloud_jobs.take_for(&id, &mut errors).await;

    let metrics_reference = match config.metrics_reference {
        MetricsReference::Corpus => reference.as_deref(),
        MetricsReference::Cloud => cloud.as_deref(),
        MetricsReference::AiFormatted => ai_formatted.as_deref(),
    };
    if matches!(config.metrics_reference, MetricsReference::Cloud) && cloud.is_none() {
        errors.push("Metrics reference missing: cloud transcript unavailable".into());
    }
    if matches!(config.metrics_reference, MetricsReference::AiFormatted) && ai_formatted.is_none() {
        errors.push("Metrics reference missing: AI formatted transcript unavailable".into());
    }
    let metrics = compute_metrics(
        metrics_reference,
        raw.as_deref(),
        post.as_deref(),
        ai_formatted.as_deref(),
        cloud.as_deref(),
    );

    let transcripts = ReportTranscripts {
        raw: raw.clone(),
        post: post.clone(),
        ai_formatted: ai_formatted.clone(),
        cloud: cloud.clone(),
        reference: reference.clone(),
    };

    write_artifacts(&id, artifacts_dir, output_root, &transcripts)?;

    Ok(ReportEntry {
        id,
        audio_path: audio_canon.to_string_lossy().to_string(),
        audio_rel_path,
        reference_path: reference_canon
            .as_ref()
            .map(|path| path.to_string_lossy().to_string()),
        duration_secs,
        transcripts,
        raw_semantics,
        metrics,
        postprocess_stats: Some(postprocessor.stats()),
        errors,
    })
}

fn compute_metrics(
    reference: Option<&str>,
    raw: Option<&str>,
    post: Option<&str>,
    ai_formatted: Option<&str>,
    cloud: Option<&str>,
) -> ReportMetrics {
    let Some(reference) = reference else {
        return ReportMetrics::default();
    };

    let (ref_tokens, ref_norm) = normalize_for_eval(reference);

    let (raw_wer, raw_cer) = match raw {
        Some(text) => {
            let (tokens, norm) = normalize_for_eval(text);
            (
                Some(word_error_rate(&ref_tokens, &tokens)),
                Some(char_error_rate(&ref_norm, &norm)),
            )
        }
        None => (None, None),
    };

    let (post_wer, post_cer) = match post {
        Some(text) => {
            let (tokens, norm) = normalize_for_eval(text);
            (
                Some(word_error_rate(&ref_tokens, &tokens)),
                Some(char_error_rate(&ref_norm, &norm)),
            )
        }
        None => (None, None),
    };

    let (ai_wer, ai_cer) = match ai_formatted {
        Some(text) => {
            let (tokens, norm) = normalize_for_eval(text);
            (
                Some(word_error_rate(&ref_tokens, &tokens)),
                Some(char_error_rate(&ref_norm, &norm)),
            )
        }
        None => (None, None),
    };

    let (cloud_wer, cloud_cer) = match cloud {
        Some(text) => {
            let (tokens, norm) = normalize_for_eval(text);
            (
                Some(word_error_rate(&ref_tokens, &tokens)),
                Some(char_error_rate(&ref_norm, &norm)),
            )
        }
        None => (None, None),
    };

    ReportMetrics {
        raw_wer,
        raw_cer,
        post_wer,
        post_cer,
        ai_wer,
        ai_cer,
        cloud_wer,
        cloud_cer,
    }
}

fn write_artifacts(
    id: &str,
    artifacts_dir: &Path,
    output_root: &Path,
    transcripts: &ReportTranscripts,
) -> Result<()> {
    if let Some(text) = transcripts.raw.as_deref() {
        safe_write_bounded(
            &artifacts_dir.join(format!("{id}.raw.txt")),
            output_root,
            text,
        )?;
    }
    if let Some(text) = transcripts.post.as_deref() {
        safe_write_bounded(
            &artifacts_dir.join(format!("{id}.post.txt")),
            output_root,
            text,
        )?;
    }
    if let Some(text) = transcripts.ai_formatted.as_deref() {
        safe_write_bounded(
            &artifacts_dir.join(format!("{id}.ai.txt")),
            output_root,
            text,
        )?;
    }
    if let Some(text) = transcripts.cloud.as_deref() {
        safe_write_bounded(
            &artifacts_dir.join(format!("{id}.cloud.txt")),
            output_root,
            text,
        )?;
    }
    if let Some(text) = transcripts.reference.as_deref() {
        safe_write_bounded(
            &artifacts_dir.join(format!("{id}.reference.txt")),
            output_root,
            text,
        )?;
    }
    // Resume marker: created even when some optional artifacts (AI/cloud) are missing.
    safe_write_bounded(
        &artifacts_dir.join(format!("{id}.done")),
        output_root,
        "done\n",
    )?;
    Ok(())
}

fn write_report_files(
    report: &QualityReport,
    config: &QualityReportConfig,
    artifacts_dir: &Path,
    output_root: &Path,
) -> Result<()> {
    let json_path = output_root.join("report.json");
    let md_path = output_root.join("report.md");
    let html_path = output_root.join("index.html");
    let ingest_path = output_root.join("ingest.jsonl");

    let json = serde_json::to_string_pretty(report)?;
    safe_write_bounded(&json_path, output_root, &json)?;

    let md = render_markdown(report);
    safe_write_bounded(&md_path, output_root, &md)?;

    let html = render_html(report, config);
    safe_write_bounded(&html_path, output_root, &html)?;

    let jsonl = render_ingest_jsonl(report, artifacts_dir)?;
    safe_write_bounded(&ingest_path, output_root, &jsonl)?;

    Ok(())
}

fn render_markdown(report: &QualityReport) -> String {
    let mut out = String::new();
    out.push_str("# CodeScribe Quality Report\n\n");
    out.push_str(&format!("Generated: {}\n\n", report.generated_at));
    out.push_str(&format!(
        "Metrics reference: {}\n\n",
        report.environment.metrics_reference
    ));
    out.push_str("| File | WER raw | WER post | WER ai | WER cloud | CER raw | CER post | CER ai | CER cloud |\n");
    out.push_str("| --- | --- | --- | --- | --- | --- | --- | --- | --- |\n");

    for entry in &report.entries {
        let m = &entry.metrics;
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            entry.id,
            fmt_opt(m.raw_wer),
            fmt_opt(m.post_wer),
            fmt_opt(m.ai_wer),
            fmt_opt(m.cloud_wer),
            fmt_opt(m.raw_cer),
            fmt_opt(m.post_cer),
            fmt_opt(m.ai_cer),
            fmt_opt(m.cloud_cer),
        ));
    }

    out.push_str("\n## Summary\n\n");
    out.push_str(&format!(
        "- Files: {}/{}\n",
        report.summary.processed_files, report.summary.total_files
    ));
    out.push_str(&format!(
        "- Avg WER raw/post/ai/cloud: {}/{}/{}/{}\n",
        fmt_opt(report.summary.avg_raw_wer),
        fmt_opt(report.summary.avg_post_wer),
        fmt_opt(report.summary.avg_ai_wer),
        fmt_opt(report.summary.avg_cloud_wer),
    ));
    out.push_str(&format!(
        "- Avg CER raw/post/ai/cloud: {}/{}/{}/{}\n",
        fmt_opt(report.summary.avg_raw_cer),
        fmt_opt(report.summary.avg_post_cer),
        fmt_opt(report.summary.avg_ai_cer),
        fmt_opt(report.summary.avg_cloud_cer),
    ));
    out.push_str(&format!(
        "- Raw transcript semantics: text_committed={}, quality_gate_dropped={}, no_speech_detected={}\n",
        report.summary.raw_text_committed,
        report.summary.raw_quality_gate_dropped,
        report.summary.raw_no_speech_detected,
    ));

    out
}

fn render_html(report: &QualityReport, config: &QualityReportConfig) -> String {
    let debug = config.debug_mode;
    let mut body = String::new();

    body.push_str(&format!(
        "<h1>CodeScribe Quality Report</h1><p>Generated: {}</p><p>Metrics reference: {}</p><p>Raw semantics: text_committed={} • quality_gate_dropped={} • no_speech_detected={}</p>",
        html_escape(&report.generated_at),
        html_escape(&report.environment.metrics_reference),
        report.summary.raw_text_committed,
        report.summary.raw_quality_gate_dropped,
        report.summary.raw_no_speech_detected
    ));

    body.push_str("<div class=\"toolbar\">");
    body.push_str("<div class=\"mode-toggle\">");
    body.push_str("<button id=\"modeDailyBtn\" type=\"button\" class=\"mode-btn\">Daily</button>");
    body.push_str("<button id=\"modeFullBtn\" type=\"button\" class=\"mode-btn\">Full</button>");
    body.push_str("</div>");
    body.push_str("<div class=\"controls\">");
    body.push_str("<div class=\"control-group\">");
    body.push_str("<button id=\"playPauseBtn\" type=\"button\">Play/Pause</button>");
    body.push_str("<button id=\"backBtn\" type=\"button\">Back 2s</button>");
    body.push_str("<button id=\"fwdBtn\" type=\"button\">Fwd 2s</button>");
    body.push_str("<button id=\"back10Btn\" type=\"button\">Back 10s</button>");
    body.push_str("<button id=\"fwd10Btn\" type=\"button\">Fwd 10s</button>");
    body.push_str("<button id=\"slowerBtn\" type=\"button\">Slower</button>");
    body.push_str("<button id=\"fasterBtn\" type=\"button\">Faster</button>");
    body.push_str("<span id=\"rateLabel\">1.0x</span>");
    body.push_str("</div>");
    body.push_str("<span class=\"active\">Active: <span id=\"activeEntry\">none</span></span>");
    body.push_str("</div>");

    body.push_str("<div class=\"tags\">");
    body.push_str("<label for=\"tagInput\">Tag presets</label>");
    body.push_str("<input id=\"tagInput\" type=\"text\" placeholder=\"Comma-separated tags\"/>");
    body.push_str("<button id=\"saveTagsBtn\" type=\"button\">Save tags</button>");
    body.push_str("<div id=\"tagPalette\" class=\"tag-palette\"></div>");
    body.push_str("</div>");

    body.push_str("<div class=\"hotkeys\">");
    body.push_str("Hotkeys: Ctrl+Cmd+Space play/pause; Ctrl+Cmd+Left/Right seek; Ctrl+Cmd+Shift+Left/Right seek 10s; ");
    body.push_str("Ctrl+Cmd+[ / ] speed; Ctrl+Cmd+1..9 insert tag; Ctrl+Cmd+N/P next/prev entry; Ctrl+Cmd+Enter reveal");
    body.push_str("</div>");

    body.push_str("<div class=\"toolbar-actions\"><button id=\"exportBtn\" type=\"button\">Export annotations</button></div>");
    body.push_str("</div>");

    for entry in &report.entries {
        let t = &entry.transcripts;
        let stats_json = entry
            .postprocess_stats
            .as_ref()
            .and_then(|s| serde_json::to_string(s).ok())
            .unwrap_or_else(|| "{}".to_string());

        body.push_str(&format!(
            "<div class=\"entry\" data-entry=\"{}\" id=\"entry-{}\">",
            html_escape(&entry.id),
            html_escape(&entry.id)
        ));
        body.push_str(&format!(
            "<h2>{}</h2><p class=\"meta\">{:.1}s • {}</p>",
            html_escape(&entry.id),
            entry.duration_secs,
            html_escape(&entry.audio_path)
        ));
        if let Some(semantics) = entry.raw_semantics.as_ref() {
            let reason = semantics.reason.as_deref().unwrap_or("-");
            body.push_str(&format!(
                "<p class=\"meta\">Raw semantics: {} ({})</p>",
                html_escape(&semantics.state.to_string()),
                html_escape(reason)
            ));
        }
        body.push_str(&format!(
            "<audio class=\"entry-audio\" data-entry=\"{}\" controls preload=\"metadata\" src=\"{}\"></audio>",
            html_escape(&entry.id),
            html_escape(&entry.audio_rel_path)
        ));

        body.push_str("<div class=\"metrics\"><table>");
        body.push_str("<tr><th></th><th>WER</th><th>CER</th></tr>");
        body.push_str(&format!(
            "<tr><td>Raw</td><td>{}</td><td>{}</td></tr>",
            fmt_opt(entry.metrics.raw_wer),
            fmt_opt(entry.metrics.raw_cer)
        ));
        body.push_str(&format!(
            "<tr><td>Post</td><td>{}</td><td>{}</td></tr>",
            fmt_opt(entry.metrics.post_wer),
            fmt_opt(entry.metrics.post_cer)
        ));
        body.push_str(&format!(
            "<tr><td>AI</td><td>{}</td><td>{}</td></tr>",
            fmt_opt(entry.metrics.ai_wer),
            fmt_opt(entry.metrics.ai_cer)
        ));
        body.push_str(&format!(
            "<tr><td>Cloud</td><td>{}</td><td>{}</td></tr>",
            fmt_opt(entry.metrics.cloud_wer),
            fmt_opt(entry.metrics.cloud_cer)
        ));
        body.push_str("</table></div>");

        body.push_str("<div class=\"human\">");
        body.push_str(&format!(
            "<label>Human transcript</label><textarea data-entry=\"{}\" spellcheck=\"false\" placeholder=\"Transcribe from audio...\"></textarea>",
            html_escape(&entry.id)
        ));
        body.push_str("</div>");

        body.push_str(&format!(
            "<button class=\"reveal\" type=\"button\" data-entry=\"{}\" {}>Reveal references</button>",
            html_escape(&entry.id),
            if debug { "" } else { "disabled" }
        ));

        body.push_str(&format!(
            "<div class=\"stats\" data-entry=\"{}\" data-stats='{}'><strong>Postprocess stats</strong>: {}</div>",
            html_escape(&entry.id),
            html_escape(&stats_json),
            html_escape(&stats_summary(entry.postprocess_stats.as_ref()))
        ));

        body.push_str("<div class=\"refs\">");
        render_ref_section(&mut body, "Raw (no postprocess)", t.raw.as_deref(), debug);
        render_ref_section(
            &mut body,
            "Postprocess (candidate)",
            t.post.as_deref(),
            debug,
        );
        render_ref_section(
            &mut body,
            "AI formatted (candidate)",
            t.ai_formatted.as_deref(),
            debug,
        );
        render_ref_section(&mut body, "Cloud reference", t.cloud.as_deref(), debug);
        render_ref_section(
            &mut body,
            "Corpus reference (.txt)",
            t.reference.as_deref(),
            debug,
        );
        body.push_str("</div>");

        if !entry.errors.is_empty() {
            body.push_str("<div class=\"errors\"><strong>Errors:</strong><ul>");
            for err in &entry.errors {
                body.push_str(&format!("<li>{}</li>", html_escape(err)));
            }
            body.push_str("</ul></div>");
        }

        body.push_str("</div>");
    }

    let debug_flag = if debug { "true" } else { "false" };
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8"/>
<meta name="viewport" content="width=device-width, initial-scale=1"/>
<title>CodeScribe Quality Report</title>
<style>
body {{ font-family: ui-sans-serif, system-ui, -apple-system, sans-serif; margin: 24px; color: #111; }}
h1 {{ margin-bottom: 8px; }}
.mode-toggle {{ display: flex; gap: 4px; margin-bottom: 6px; }}
.mode-btn {{ padding: 6px 16px; border: 1px solid #ccc; border-radius: 6px; cursor: pointer; background: #fff; font-weight: 500; }}
.mode-btn.active {{ background: #111; color: #fff; border-color: #111; }}
.toolbar {{ border: 1px solid #ddd; border-radius: 12px; padding: 12px; margin: 16px 0; display: flex; flex-direction: column; gap: 10px; }}
.controls {{ display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: 10px; }}
.control-group {{ display: flex; flex-wrap: wrap; gap: 6px; }}
.tags {{ display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }}
.tags input {{ min-width: 240px; padding: 6px 8px; }}
.tag-palette {{ display: flex; flex-wrap: wrap; gap: 6px; }}
.tag {{ border: 1px solid #ccc; border-radius: 999px; padding: 4px 10px; font-size: 0.85rem; cursor: pointer; }}
.hotkeys {{ font-size: 0.85rem; color: #555; }}
.toolbar-actions {{ display: flex; justify-content: flex-end; }}
.entry {{ border: 1px solid #ddd; border-radius: 10px; padding: 16px; margin-bottom: 18px; }}
.entry.active {{ border-color: #111; box-shadow: 0 0 0 2px rgba(0,0,0,0.08); }}
.meta {{ color: #555; font-size: 0.9rem; }}
audio {{ width: 100%; margin: 8px 0; }}
.metrics table {{ width: 100%; border-collapse: collapse; margin: 8px 0; }}
.metrics th, .metrics td {{ border-bottom: 1px solid #eee; padding: 4px 6px; text-align: left; }}
.stats {{ font-size: 0.85rem; color: #444; margin-top: 6px; }}
.human textarea {{ width: 100%; min-height: 140px; padding: 8px; font-size: 1rem; }}
.reveal {{ margin: 8px 0; }}
.refs {{ margin-top: 12px; }}
.ref {{ margin-top: 8px; }}
.ref pre {{ background: #f6f6f6; padding: 10px; border-radius: 6px; white-space: pre-wrap; }}
.ref.hidden {{ display: none; }}
.errors {{ margin-top: 10px; color: #a00; }}
</style>
</head>
<body>
{body}
<script>
const INITIAL_DEBUG = {debug_flag};
const MIN_LEN = 40;
const MODE_STORAGE_KEY = 'codescribe:mode';
const TAG_STORAGE_KEY = 'codescribe:tags';
const DEFAULT_TAGS = ['NIEWYRAZNE', 'NIESLYSZALNE', 'BELKOT', 'PRZERWA', 'SZUM'];
const TAG_COLORS = ['#e3f2fd', '#e8f5e9', '#fff8e1', '#fce4ec', '#ede7f6', '#f3e5f5', '#e0f7fa'];
const RATE_STEPS = [0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0];

let activeEntryId = null;
let activeTextarea = null;
let activeAudio = null;
let currentTags = [];
let currentMode = localStorage.getItem(MODE_STORAGE_KEY) || (INITIAL_DEBUG ? 'daily' : 'full');

function setMode(mode) {{
  currentMode = mode;
  localStorage.setItem(MODE_STORAGE_KEY, mode);
  document.getElementById('modeDailyBtn')?.classList.toggle('active', mode === 'daily');
  document.getElementById('modeFullBtn')?.classList.toggle('active', mode === 'full');
  document.querySelectorAll('.entry').forEach(entry => {{
    const refs = entry.querySelectorAll('.ref');
    const revealBtn = entry.querySelector('button.reveal');
    if (mode === 'daily') {{
      refs.forEach(r => r.classList.remove('hidden'));
      if (revealBtn) revealBtn.style.display = 'none';
    }} else {{
      refs.forEach(r => r.classList.add('hidden'));
      if (revealBtn) {{
        revealBtn.style.display = '';
        const entryId = revealBtn.dataset.entry;
        updateReveal(entryId);
      }}
    }}
  }});
}}

function entryElements() {{
  return Array.from(document.querySelectorAll('.entry'));
}}

function findEntry(entryId) {{
  return document.querySelector('.entry[data-entry=\"' + entryId + '\"]');
}}

function setActiveEntry(entryId) {{
  const entry = findEntry(entryId);
  if (!entry) return;
  entryElements().forEach(el => el.classList.remove('active'));
  entry.classList.add('active');
  activeEntryId = entryId;
  activeTextarea = entry.querySelector('textarea[data-entry]');
  activeAudio = entry.querySelector('audio');
  const label = document.getElementById('activeEntry');
  if (label) label.textContent = entryId;
  updateRateLabel(activeAudio);
}}

function setActiveFromElement(el) {{
  const entry = el?.closest?.('.entry');
  if (entry) setActiveEntry(entry.dataset.entry);
}}

function getActiveAudio() {{
  if (activeAudio) return activeAudio;
  const first = document.querySelector('audio');
  return first || null;
}}

function updateRateLabel(audio) {{
  const label = document.getElementById('rateLabel');
  if (!label) return;
  const rate = audio ? audio.playbackRate : 1.0;
  label.textContent = rate.toFixed(2) + 'x';
}}

function togglePlayPause() {{
  const audio = getActiveAudio();
  if (!audio) return;
  if (audio.paused) {{
    audio.play();
  }} else {{
    audio.pause();
  }}
}}

function seek(delta) {{
  const audio = getActiveAudio();
  if (!audio) return;
  const next = Math.min(Math.max(audio.currentTime + delta, 0), audio.duration || audio.currentTime + delta);
  audio.currentTime = next;
}}

function changeRate(step) {{
  const audio = getActiveAudio();
  if (!audio) return;
  const idx = RATE_STEPS.findIndex(r => Math.abs(r - audio.playbackRate) < 0.01);
  const nextIdx = Math.min(Math.max((idx < 0 ? 2 : idx) + step, 0), RATE_STEPS.length - 1);
  audio.playbackRate = RATE_STEPS[nextIdx];
  updateRateLabel(audio);
}}

function insertAtCursor(area, text) {{
  if (!area) return;
  const start = area.selectionStart ?? area.value.length;
  const end = area.selectionEnd ?? area.value.length;
  const before = area.value.slice(0, start);
  const after = area.value.slice(end);
  let insert = text;
  if (before && !before.endsWith(' ')) insert = ' ' + insert;
  if (after && !after.startsWith(' ')) insert = insert + ' ';
  area.value = before + insert + after;
  const nextPos = (before + insert).length;
  area.selectionStart = nextPos;
  area.selectionEnd = nextPos;
  area.dispatchEvent(new Event('input'));
}}

function insertTag(tag) {{
  if (!activeTextarea) {{
    const first = document.querySelector('textarea[data-entry]');
    if (first) {{
      first.focus();
      activeTextarea = first;
    }}
  }}
  if (!activeTextarea) return;
  insertAtCursor(activeTextarea, '[' + tag + ']');
}}

function parseTags(value) {{
  return value
    .split(',')
    .map(t => t.trim())
    .filter(Boolean);
}}

function loadTags() {{
  const stored = localStorage.getItem(TAG_STORAGE_KEY);
  if (!stored) return [...DEFAULT_TAGS];
  try {{
    const parsed = JSON.parse(stored);
    if (Array.isArray(parsed) && parsed.length > 0) return parsed;
  }} catch (_) {{
    return parseTags(stored);
  }}
  return [...DEFAULT_TAGS];
}}

function saveTags(tags) {{
  localStorage.setItem(TAG_STORAGE_KEY, JSON.stringify(tags));
}}

function renderTags(tags) {{
  const palette = document.getElementById('tagPalette');
  if (!palette) return;
  palette.innerHTML = '';
  tags.forEach((tag, idx) => {{
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'tag';
    btn.textContent = tag;
    btn.style.background = TAG_COLORS[idx % TAG_COLORS.length];
    btn.addEventListener('click', () => insertTag(tag));
    palette.appendChild(btn);
  }});
}}

function loadAnnotations() {{
  document.querySelectorAll('textarea[data-entry]').forEach(area => {{
    const key = 'codescribe:human:' + area.dataset.entry;
    const saved = localStorage.getItem(key);
    if (saved) area.value = saved;
    area.addEventListener('input', () => {{
      localStorage.setItem(key, area.value);
      updateReveal(area.dataset.entry);
    }});
    area.addEventListener('focus', () => {{
      setActiveEntry(area.dataset.entry);
    }});
  }});
  const first = document.querySelector('textarea[data-entry]');
  if (first) setActiveEntry(first.dataset.entry);
}}

function updateReveal(entryId) {{
  const area = document.querySelector('textarea[data-entry=\"' + entryId + '\"]');
  const button = document.querySelector('button.reveal[data-entry=\"' + entryId + '\"]');
  if (!area || !button) return;
  if (currentMode === 'daily') {{
    button.disabled = false;
    button.style.display = 'none';
    reveal(entryId);
    return;
  }}
  button.style.display = '';
  button.disabled = area.value.trim().length < MIN_LEN;
}}

function reveal(entryId) {{
  if (!canReveal(entryId)) return;
  document.querySelectorAll('.entry').forEach(entry => {{
    if (entry.querySelector('textarea[data-entry]')?.dataset.entry !== entryId) return;
    entry.querySelectorAll('.ref').forEach(ref => ref.classList.remove('hidden'));
  }});
}}

function canReveal(entryId) {{
  if (currentMode === 'daily') return true;
  const button = document.querySelector('button.reveal[data-entry=\"' + entryId + '\"]');
  return button && !button.disabled;
}}

function attachRevealButtons() {{
  document.querySelectorAll('button.reveal').forEach(btn => {{
    btn.addEventListener('click', () => reveal(btn.dataset.entry));
  }});
}}

function exportAnnotations() {{
  const items = [];
  document.querySelectorAll('textarea[data-entry]').forEach(area => {{
    items.push({{
      id: area.dataset.entry,
      human: area.value.trim()
    }});
  }});
  const blob = new Blob([JSON.stringify(items, null, 2)], {{type: 'application/json'}});
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = 'annotations.json';
  a.click();
  URL.revokeObjectURL(url);
}}

function bindAudio() {{
  document.querySelectorAll('audio.entry-audio').forEach(audio => {{
    audio.addEventListener('play', () => setActiveEntry(audio.dataset.entry));
    audio.addEventListener('click', () => setActiveEntry(audio.dataset.entry));
    audio.addEventListener('ratechange', () => updateRateLabel(audio));
  }});
}}

function focusEntry(offset) {{
  const entries = entryElements();
  if (!entries.length) return;
  const current = activeEntryId ? entries.findIndex(e => e.dataset.entry === activeEntryId) : -1;
  const nextIdx = Math.min(Math.max(current + offset, 0), entries.length - 1);
  const nextEntry = entries[nextIdx];
  if (!nextEntry) return;
  const area = nextEntry.querySelector('textarea[data-entry]');
  if (area) {{
    area.focus();
  }} else {{
    setActiveEntry(nextEntry.dataset.entry);
  }}
  nextEntry.scrollIntoView({{behavior: 'smooth', block: 'center'}});
}}

function handleHotkeys(event) {{
  if (!(event.ctrlKey && event.metaKey)) return;
  const key = event.key.toLowerCase();
  if (event.code === 'Space') {{
    event.preventDefault();
    togglePlayPause();
    return;
  }}
  if (event.code === 'ArrowLeft') {{
    event.preventDefault();
    seek(event.shiftKey ? -10 : -2);
    return;
  }}
  if (event.code === 'ArrowRight') {{
    event.preventDefault();
    seek(event.shiftKey ? 10 : 2);
    return;
  }}
  if (event.code === 'BracketLeft') {{
    event.preventDefault();
    changeRate(-1);
    return;
  }}
  if (event.code === 'BracketRight') {{
    event.preventDefault();
    changeRate(1);
    return;
  }}
  if (event.code === 'Enter') {{
    event.preventDefault();
    if (activeEntryId) reveal(activeEntryId);
    return;
  }}
  if (key >= '1' && key <= '9') {{
    event.preventDefault();
    const idx = Number(key) - 1;
    if (currentTags[idx]) insertTag(currentTags[idx]);
    return;
  }}
  if (key === 'n') {{
    event.preventDefault();
    focusEntry(1);
    return;
  }}
  if (key === 'p') {{
    event.preventDefault();
    focusEntry(-1);
    return;
  }}
}}

document.getElementById('modeDailyBtn')?.addEventListener('click', () => setMode('daily'));
document.getElementById('modeFullBtn')?.addEventListener('click', () => setMode('full'));
document.getElementById('exportBtn')?.addEventListener('click', exportAnnotations);
document.getElementById('playPauseBtn')?.addEventListener('click', togglePlayPause);
document.getElementById('backBtn')?.addEventListener('click', () => seek(-2));
document.getElementById('fwdBtn')?.addEventListener('click', () => seek(2));
document.getElementById('back10Btn')?.addEventListener('click', () => seek(-10));
document.getElementById('fwd10Btn')?.addEventListener('click', () => seek(10));
document.getElementById('slowerBtn')?.addEventListener('click', () => changeRate(-1));
document.getElementById('fasterBtn')?.addEventListener('click', () => changeRate(1));

const tagInput = document.getElementById('tagInput');
const saveTagsBtn = document.getElementById('saveTagsBtn');
if (tagInput && saveTagsBtn) {{
  saveTagsBtn.addEventListener('click', () => {{
    currentTags = parseTags(tagInput.value);
    if (currentTags.length === 0) currentTags = [...DEFAULT_TAGS];
    saveTags(currentTags);
    renderTags(currentTags);
  }});
  tagInput.addEventListener('keydown', (event) => {{
    if (event.key === 'Enter') {{
      event.preventDefault();
      saveTagsBtn.click();
    }}
  }});
}}

currentTags = loadTags();
if (tagInput) tagInput.value = currentTags.join(', ');
renderTags(currentTags);
loadAnnotations();
attachRevealButtons();
setMode(currentMode);
bindAudio();
document.addEventListener('keydown', handleHotkeys);
</script>
</body>
</html>"#
    )
}

fn render_ref_section(body: &mut String, label: &str, text: Option<&str>, debug: bool) {
    if let Some(text) = text {
        let hidden_class = if debug { "" } else { " hidden" };
        body.push_str(&format!(
            "<div class=\"ref{hidden_class}\"><h3>{}</h3><pre>{}</pre></div>",
            html_escape(label),
            html_escape(text)
        ));
    }
}

fn stats_summary(stats: Option<&StreamPostProcessStats>) -> String {
    let Some(stats) = stats else {
        return "n/a".to_string();
    };

    format!(
        "in={}, out={}, drop={}, gate={}, lexicon={}, repeat={}, suspicious={}",
        stats.input_chunks,
        stats.output_chunks,
        stats.dropped_chunks,
        stats.gate_drops,
        stats.lexicon_rewrites,
        stats.repetition_cleanups,
        stats.suspicious_chunks
    )
}

fn render_ingest_jsonl(report: &QualityReport, artifacts_dir: &Path) -> Result<String> {
    let mut out = String::new();
    for entry in &report.entries {
        let base = artifacts_dir.join(&entry.id);
        let lang = report.environment.whisper_language.clone();
        for (suffix, text_opt, source) in [
            ("raw.txt", entry.transcripts.raw.as_deref(), "raw"),
            ("post.txt", entry.transcripts.post.as_deref(), "postprocess"),
            (
                "ai.txt",
                entry.transcripts.ai_formatted.as_deref(),
                "ai_formatted",
            ),
            ("cloud.txt", entry.transcripts.cloud.as_deref(), "cloud"),
            (
                "reference.txt",
                entry.transcripts.reference.as_deref(),
                "reference",
            ),
        ] {
            let Some(text) = text_opt else { continue };
            let record = serde_json::json!({
                "id": entry.id,
                "audio_path": entry.audio_path,
                "text": text,
                "source": source,
                "language": lang,
                "artifact_path": base.with_extension(suffix).to_string_lossy(),
            });
            out.push_str(&record.to_string());
            out.push('\n');
        }
    }
    Ok(out)
}

async fn transcribe_raw_for_report(
    audio_path: &Path,
    samples: &[f32],
    sample_rate: u32,
    config: &QualityReportConfig,
    errors: &mut Vec<String>,
) -> Option<RawTranscript> {
    match config.local_transcription {
        LocalTranscriptionMode::LocalWhisper => {
            // Single-pass transcription: engine handles 25s/5s chunking internally.
            match crate::stt::transcribe_long_with_segments(
                samples,
                sample_rate,
                config.language.as_deref(),
            ) {
                Ok(transcript) => Some(transcript),
                Err(e) => {
                    errors.push(format!("Raw transcription failed: {}", e));
                    None
                }
            }
        }
        LocalTranscriptionMode::CodeScribeIpc => {
            match crate::ipc::transcribe_file(audio_path).await {
                Ok(text) => {
                    let text = text.trim().to_string();
                    if text.is_empty() {
                        errors.push(
                            "Raw transcription skipped: CodeScribe IPC returned empty transcript"
                                .into(),
                        );
                        None
                    } else {
                        Some(RawTranscript {
                            text,
                            segments: Vec::new(),
                            avg_logprob: None,
                            compression_ratio: None,
                            quality_gate_dropped: false,
                        })
                    }
                }
                Err(e) => {
                    errors.push(format!(
                        "Raw transcription skipped: CodeScribe IPC unavailable/degraded: {}",
                        e
                    ));
                    None
                }
            }
        }
    }
}

fn snapshot_environment(
    metrics_reference: MetricsReference,
    local_transcription: LocalTranscriptionMode,
) -> ReportEnvironment {
    let config = Config::load();
    ReportEnvironment {
        stt_endpoint: config.stt_endpoint.clone(),
        stt_api_key_present: config
            .stt_api_key
            .as_ref()
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false),
        llm_formatting_endpoint: std::env::var("LLM_FORMATTING_ENDPOINT").ok(),
        llm_formatting_model: std::env::var("LLM_FORMATTING_MODEL").ok(),
        llm_formatting_key_present: std::env::var("LLM_FORMATTING_API_KEY")
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false),
        local_model: Some(config.local_model),
        whisper_language: Some(config.whisper_language.as_str().to_string()),
        metrics_reference: metrics_reference.as_str().to_string(),
        local_transcription: local_transcription.as_str().to_string(),
    }
}

fn ensure_audio_asset(
    audio_path: &Path,
    audio_dir: &Path,
    asset_id: &str,
    input_root: &Path,
    output_root: &Path,
    copy_audio: bool,
) -> Result<String> {
    let ext = audio_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("wav");
    let filename = format!("{asset_id}.{ext}");
    let dest = audio_dir.join(&filename);
    if dest.exists() {
        return Ok(format!("audio/{}", filename));
    }

    if copy_audio {
        safe_copy_bounded(audio_path, input_root, &dest, output_root)?;
    } else {
        #[cfg(target_family = "unix")]
        {
            safe_symlink_or_copy_bounded(audio_path, input_root, &dest, output_root)?;
        }
        #[cfg(not(target_family = "unix"))]
        {
            safe_copy_bounded(audio_path, input_root, &dest, output_root)?;
        }
    }

    Ok(format!("audio/{}", filename))
}

#[derive(Debug, Clone)]
struct CorpusPair {
    id: String,
    audio_path: PathBuf,
    reference_path: PathBuf,
}

fn collect_pairs(root: &Path, date_filter: Option<&str>, limit: usize) -> Vec<CorpusPair> {
    let mut pairs = Vec::new();
    if !root.exists() {
        return pairs;
    }

    let mut subdirs = Vec::new();
    if let Some(date) = date_filter {
        let dir = root.join(date);
        if dir.exists() {
            subdirs.push(dir);
        }
    } else if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                subdirs.push(path);
            }
        }
    }

    subdirs.sort();

    for dir in subdirs {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut wavs = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("wav") {
                wavs.push(path);
            }
        }

        wavs.sort();
        for wav in wavs {
            let stem = match wav.file_stem().and_then(|s| s.to_str()) {
                Some(stem) => stem,
                None => continue,
            };
            let txt = wav.with_file_name(format!("{stem}.txt"));
            if txt.exists() {
                let id = make_entry_id(&wav);
                pairs.push(CorpusPair {
                    id,
                    audio_path: wav,
                    reference_path: txt,
                });
            }
        }
    }

    if limit > 0 && pairs.len() > limit {
        let start = pairs.len() - limit;
        pairs = pairs[start..].to_vec();
    }

    pairs
}

fn make_entry_id(audio_path: &Path) -> String {
    let stem = audio_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("recording");
    let date = audio_path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str());
    let raw_id = match date {
        Some(date) => format!("{date}__{stem}"),
        None => stem.to_string(),
    };
    raw_id.replace(['/', '\\'], "_")
}

fn resolve_input_root(path: &Path, root: &Path) -> Result<PathBuf> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    safe_canonicalize_bounded(&candidate, root)
        .with_context(|| format!("Input dir must stay within {}", root.display()))
}

fn resolve_output_root(path: &Path, root: &Path) -> Result<PathBuf> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let prepared = safe_prepare_path(&candidate, root)
        .with_context(|| format!("Output dir must stay within {}", root.display()))?;
    fs::create_dir_all(&prepared)?;
    safe_canonicalize_bounded(&prepared, root)
        .with_context(|| format!("Output dir must stay within {}", root.display()))
}

fn preview_for_log(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

fn normalize_for_eval(text: &str) -> (Vec<String>, String) {
    let mut normalized = String::with_capacity(text.len());
    for ch in text.to_lowercase().chars() {
        if ch.is_alphanumeric() || ch.is_whitespace() {
            normalized.push(ch);
        } else {
            normalized.push(' ');
        }
    }
    let tokens: Vec<String> = normalized
        .split_whitespace()
        .map(|t| t.to_string())
        .collect();
    let normalized = tokens.join(" ");
    (tokens, normalized)
}

fn word_error_rate(reference: &[String], hypothesis: &[String]) -> f32 {
    let dist = levenshtein(reference, hypothesis);
    let denom = reference.len().max(1) as f32;
    dist as f32 / denom
}

fn char_error_rate(reference: &str, hypothesis: &str) -> f32 {
    let ref_chars: Vec<char> = reference.chars().collect();
    let hyp_chars: Vec<char> = hypothesis.chars().collect();
    let dist = levenshtein(&ref_chars, &hyp_chars);
    let denom = ref_chars.len().max(1) as f32;
    dist as f32 / denom
}

fn levenshtein<T: Eq>(a: &[T], b: &[T]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];

    for (i, item_a) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, item_b) in b.iter().enumerate() {
            let cost = if item_a == item_b { 0 } else { 1 };
            cur[j + 1] = std::cmp::min(std::cmp::min(prev[j + 1] + 1, cur[j] + 1), prev[j] + cost);
        }
        prev.clone_from(&cur);
    }

    prev[b.len()]
}

fn html_escape(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn fmt_opt(value: Option<f32>) -> String {
    value
        .map(|v| format!("{:.3}", v))
        .unwrap_or_else(|| "-".to_string())
}

#[derive(Default)]
struct Totals {
    raw_wer: Vec<f32>,
    post_wer: Vec<f32>,
    ai_wer: Vec<f32>,
    cloud_wer: Vec<f32>,
    raw_cer: Vec<f32>,
    post_cer: Vec<f32>,
    ai_cer: Vec<f32>,
    cloud_cer: Vec<f32>,
    raw_no_speech_detected: usize,
    raw_quality_gate_dropped: usize,
    raw_text_committed: usize,
    processed: usize,
}

impl Totals {
    fn accumulate(&mut self, entry: &ReportEntry) {
        self.processed += 1;
        let metrics = &entry.metrics;
        if let Some(v) = metrics.raw_wer {
            self.raw_wer.push(v);
        }
        if let Some(v) = metrics.post_wer {
            self.post_wer.push(v);
        }
        if let Some(v) = metrics.ai_wer {
            self.ai_wer.push(v);
        }
        if let Some(v) = metrics.cloud_wer {
            self.cloud_wer.push(v);
        }
        if let Some(v) = metrics.raw_cer {
            self.raw_cer.push(v);
        }
        if let Some(v) = metrics.post_cer {
            self.post_cer.push(v);
        }
        if let Some(v) = metrics.ai_cer {
            self.ai_cer.push(v);
        }
        if let Some(v) = metrics.cloud_cer {
            self.cloud_cer.push(v);
        }

        if let Some(semantics) = entry.raw_semantics.as_ref() {
            match semantics.state {
                ReportTranscriptState::NoSpeechDetected => self.raw_no_speech_detected += 1,
                ReportTranscriptState::QualityGateDropped => self.raw_quality_gate_dropped += 1,
                ReportTranscriptState::TextCommitted => self.raw_text_committed += 1,
                ReportTranscriptState::EmptyTranscript => {}
            }
        }
    }

    fn finish(self, total_files: usize) -> ReportSummary {
        ReportSummary {
            total_files,
            processed_files: self.processed,
            avg_raw_wer: avg(&self.raw_wer),
            avg_post_wer: avg(&self.post_wer),
            avg_ai_wer: avg(&self.ai_wer),
            avg_cloud_wer: avg(&self.cloud_wer),
            avg_raw_cer: avg(&self.raw_cer),
            avg_post_cer: avg(&self.post_cer),
            avg_ai_cer: avg(&self.ai_cer),
            avg_cloud_cer: avg(&self.cloud_cer),
            raw_no_speech_detected: self.raw_no_speech_detected,
            raw_quality_gate_dropped: self.raw_quality_gate_dropped,
            raw_text_committed: self.raw_text_committed,
        }
    }
}

fn avg(values: &[f32]) -> Option<f32> {
    if values.is_empty() {
        None
    } else {
        Some(values.iter().sum::<f32>() / values.len() as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_raw_semantics_distinguishes_no_speech_and_gate_drop() {
        let no_speech = classify_raw_semantics(
            Some(&RawTranscript::default()),
            Some("vad_no_speech_detected"),
        )
        .expect("semantics");
        assert_eq!(no_speech.state, ReportTranscriptState::NoSpeechDetected);
        assert_eq!(no_speech.reason.as_deref(), Some("vad_no_speech_detected"));

        let quality_gate = classify_raw_semantics(
            Some(&RawTranscript {
                quality_gate_dropped: true,
                ..Default::default()
            }),
            None,
        )
        .expect("semantics");
        assert_eq!(
            quality_gate.state,
            ReportTranscriptState::QualityGateDropped
        );
        assert_eq!(quality_gate.reason.as_deref(), Some("quality_gate_dropped"));

        let committed = classify_raw_semantics(
            Some(&RawTranscript {
                text: "hello".to_string(),
                ..Default::default()
            }),
            None,
        )
        .expect("semantics");
        assert_eq!(committed.state, ReportTranscriptState::TextCommitted);
    }

    #[test]
    fn totals_finish_counts_raw_semantics_separately() {
        let mut totals = Totals::default();
        let mk_entry = |state: ReportTranscriptState, reason: Option<&str>| ReportEntry {
            id: state.to_string(),
            audio_path: "a.wav".to_string(),
            audio_rel_path: "audio/a.wav".to_string(),
            reference_path: None,
            duration_secs: 1.0,
            transcripts: ReportTranscripts::default(),
            raw_semantics: Some(ReportTranscriptSemantics {
                state,
                reason: reason.map(str::to_string),
            }),
            metrics: ReportMetrics::default(),
            postprocess_stats: None,
            errors: Vec::new(),
        };

        totals.accumulate(&mk_entry(ReportTranscriptState::TextCommitted, None));
        totals.accumulate(&mk_entry(ReportTranscriptState::QualityGateDropped, None));
        totals.accumulate(&mk_entry(
            ReportTranscriptState::NoSpeechDetected,
            Some("vad_no_speech_detected"),
        ));

        let summary = totals.finish(3);
        assert_eq!(summary.raw_text_committed, 1);
        assert_eq!(summary.raw_quality_gate_dropped, 1);
        assert_eq!(summary.raw_no_speech_detected, 1);
    }

    #[test]
    fn cloud_reference_credentials_ignore_local_committed_transcript_mode() {
        let mut config = Config {
            use_local_stt: true,
            stt_endpoint: Some(" https://api.example.test/v1/audio/transcriptions ".into()),
            stt_api_key: Some(" test-token ".into()),
            ..Default::default()
        };

        assert_eq!(
            cloud_reference_credentials(&config),
            Some((
                "https://api.example.test/v1/audio/transcriptions".into(),
                "test-token".into(),
            ))
        );

        config.stt_api_key = Some("   ".into());
        assert_eq!(cloud_reference_credentials(&config), None);
    }

    #[tokio::test]
    async fn codescribe_ipc_transcription_failure_is_degraded_not_local_fallback() {
        let temp = tempfile::tempdir().expect("create temp dir for ipc fallback test");
        let config = QualityReportConfig {
            input_dir: temp.path().join("input"),
            output_dir: temp.path().join("output"),
            date_filter: None,
            limit: 0,
            language: Some("pl".to_string()),
            skip_cloud: true,
            cloud_concurrency: 1,
            skip_formatting: true,
            debug_mode: false,
            copy_audio: false,
            metrics_reference: MetricsReference::Corpus,
            local_transcription: LocalTranscriptionMode::CodeScribeIpc,
        };
        let mut errors = Vec::new();
        let missing_audio = temp.path().join("missing.wav");

        let transcript =
            transcribe_raw_for_report(&missing_audio, &[], 16_000, &config, &mut errors).await;

        assert!(
            transcript.is_none(),
            "IPC failure must not fall back to in-daemon local Whisper"
        );
        assert!(
            errors
                .iter()
                .any(|error| error.starts_with("Raw transcription skipped: CodeScribe IPC")),
            "expected degraded IPC error, got: {errors:?}"
        );
    }

    #[test]
    fn test_word_error_rate() {
        let (ref_tokens, _) = normalize_for_eval("ala ma kota");
        let (hyp_tokens, _) = normalize_for_eval("ala ma psa");
        let wer = word_error_rate(&ref_tokens, &hyp_tokens);
        assert!((wer - 0.333).abs() < 0.01);
    }

    #[test]
    fn test_html_escape() {
        let input = "<tag>&\"'";
        let escaped = html_escape(input);
        assert_eq!(escaped, "&lt;tag&gt;&amp;&quot;&#39;");
    }
}
