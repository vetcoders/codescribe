//! Unattended corpus census and production-overlay replay.
//!
//! This tool never loads the operator's `settings.json` or `.env`. Matrix
//! profiles run in fresh child processes with an isolated
//! `CODESCRIBE_DATA_DIR`; audio is the only substituted production boundary.
//! Machine reports contain hashes, counts and scores, never filenames or
//! transcript bodies. A separate private Qube HTML contains transcript bodies
//! and opaque audio links for local operator review.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, ExitCode, Stdio};
use std::str::FromStr;
use std::time::Instant;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use clap::{Parser, Subcommand, ValueEnum};
use codescribe::controller::production_replay::{ProductionReplayLane, replay_overlay_recording};
use codescribe::qube_report::{
    LocalTranscriptionMode, MetricsReference, QualityReport, QualityReportConfig, ReportEntry,
    ReportEnvironment, ReportMetrics, ReportSummary, ReportTranscriptSemantics,
    ReportTranscriptState, ReportTranscripts, render_html as render_qube_html,
};
use codescribe_core::config::UserSettings;
use codescribe_core::pipeline::contracts::{EngineEvent, LayerSource};
use codescribe_core::quality::engine_contract::{CORPUS_REPORT_SCHEMA, ENGINE_CONTRACT_ID};
use codescribe_core::quality::seal_atlas_html::{
    SealAtlasPage, SealAtlasStats, render_seal_atlas_html,
};
use codescribe_core::util::safe_path::{safe_open, safe_symlink_or_copy_bounded};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const REPORT_SCHEMA: &str = CORPUS_REPORT_SCHEMA;
const AUDIO_EXTENSIONS: [&str; 3] = ["wav", "m4a", "mp3"];
const CONTROLLED_ENV: [&str; 14] = [
    "CODESCRIBE_STT_ENGINE",
    "CODESCRIBE_LAYERED_TRANSCRIPTION",
    "STT_TAIL_PROVIDER",
    "CODESCRIBE_SILERO_FUSION",
    "CODESCRIBE_SILERO_FUSION_CONTEXT",
    "CODESCRIBE_SPAN_IDEMPOTENCE",
    "CODESCRIBE_INLINE_FORMAT",
    "CODESCRIBE_STT_INITIAL_PROMPT_ENABLED",
    "FINAL_PASS_MODE",
    "CODESCRIBE_FINAL_PASS_MODE",
    "CODESCRIBE_LOCAL_STT_FINAL_PASS",
    "CODESCRIBE_APPLE_STT_ALLOW_DOWNLOAD",
    "CODESCRIBE_APPLE_STT_BRIDGE",
    "CODESCRIBE_BRIDGE_DISCLAIM",
];

#[derive(Debug, Parser)]
#[command(
    name = "codescribe-corpus",
    about = "Private corpus census and production-overlay replay",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Inventory audio and reference classes without running STT.
    Census {
        /// Corpus root. Repeat to combine roots.
        #[arg(long = "root", required = true)]
        roots: Vec<PathBuf>,
        /// JSON report path.
        #[arg(long)]
        out: PathBuf,
        /// Treat same-stem TXT files as historical references.
        #[arg(long)]
        include_historical: bool,
    },
    /// Run one or more isolated replay profiles and write a retained report.
    Run {
        /// Corpus root. Repeat to combine roots.
        #[arg(long = "root", required = true)]
        roots: Vec<PathBuf>,
        /// Durable report directory.
        #[arg(long)]
        out_dir: PathBuf,
        /// Comma-separated profile names.
        #[arg(
            long,
            value_delimiter = ',',
            default_value = "apple-layer0,apple-layer1-inprocess"
        )]
        profiles: Vec<ReplayProfile>,
        /// Independent executions per distinct recording and profile.
        #[arg(long, default_value_t = 1)]
        runs: usize,
        /// Reference selection policy.
        #[arg(long, value_enum, default_value_t = ReferencePolicy::Human)]
        references: ReferencePolicy,
        /// Bound the selected distinct recordings after stable hash sort.
        #[arg(long)]
        max_recordings: Option<usize>,
        /// Recognition language pin.
        #[arg(long, default_value = "pl")]
        language: String,
        /// Exact source commit claimed by this binary invocation.
        #[arg(long)]
        commit: String,
        /// Exact signed Apple STT bridge artifact used by every worker.
        #[arg(long)]
        apple_bridge: PathBuf,
    },
    /// Internal one-profile worker. Fresh process = fresh runtime globals.
    #[command(hide = true)]
    Worker {
        #[arg(long = "root", required = true)]
        roots: Vec<PathBuf>,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        profile: ReplayProfile,
        #[arg(long, default_value_t = 1)]
        runs: usize,
        #[arg(long, value_enum)]
        references: ReferencePolicy,
        #[arg(long)]
        max_recordings: Option<usize>,
        #[arg(long)]
        language: String,
        #[arg(long)]
        commit: String,
        #[arg(long)]
        apple_bridge: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
enum ReferencePolicy {
    /// Only explicit `_human_transcription.txt` siblings are quality truth.
    Human,
    /// Prefer explicit human truth, then admit same-stem historical TXT.
    HumanAndHistorical,
}

impl ReferencePolicy {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::HumanAndHistorical => "human_and_historical",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
enum ReplayProfile {
    /// Apple canvas + lexicon, Layer 1 disarmed.
    AppleLayer0,
    /// Apple canvas + in-process Whisper partials + lexicon.
    AppleLayer1Inprocess,
    /// Apple canvas + sidecar Whisper partials + lexicon.
    AppleLayer1Sidecar,
    /// Apple canvas + remote Whisper partials + lexicon.
    AppleLayer1Remote,
    /// Layer 1 plus Silero utterance identity, utterance-only context.
    AppleLayer1FusionUtterance,
    /// Layer 1 plus Silero identity and bounded left-audio context.
    AppleLayer1FusionLeftPad,
    /// Layer 1 plus Silero identity and stable-text prompt context.
    AppleLayer1FusionStablePrompt,
    /// Fusion plus sealed-span replay/idempotence fence.
    AppleLayer1FusionIdempotent,
    /// Layer 1 live transcript plus the opt-in local final-pass stop lane.
    AppleLayer1LocalFinalPass,
}

impl ReplayProfile {
    const fn token(self) -> &'static str {
        match self {
            Self::AppleLayer0 => "apple-layer0",
            Self::AppleLayer1Inprocess => "apple-layer1-inprocess",
            Self::AppleLayer1Sidecar => "apple-layer1-sidecar",
            Self::AppleLayer1Remote => "apple-layer1-remote",
            Self::AppleLayer1FusionUtterance => "apple-layer1-fusion-utterance",
            Self::AppleLayer1FusionLeftPad => "apple-layer1-fusion-left-pad",
            Self::AppleLayer1FusionStablePrompt => "apple-layer1-fusion-stable-prompt",
            Self::AppleLayer1FusionIdempotent => "apple-layer1-fusion-idempotent",
            Self::AppleLayer1LocalFinalPass => "apple-layer1-local-final-pass",
        }
    }

    const fn layered(self) -> bool {
        !matches!(self, Self::AppleLayer0)
    }

    const fn tail_provider(self) -> &'static str {
        match self {
            Self::AppleLayer1Sidecar => "sidecar",
            Self::AppleLayer1Remote => "remote",
            _ => "inprocess",
        }
    }

    const fn fusion(self) -> bool {
        matches!(
            self,
            Self::AppleLayer1FusionUtterance
                | Self::AppleLayer1FusionLeftPad
                | Self::AppleLayer1FusionStablePrompt
                | Self::AppleLayer1FusionIdempotent
        )
    }

    const fn fusion_context(self) -> &'static str {
        match self {
            Self::AppleLayer1FusionLeftPad => "left_pad",
            Self::AppleLayer1FusionStablePrompt => "stable_prompt",
            _ => "utterance_only",
        }
    }

    const fn idempotence(self) -> bool {
        matches!(self, Self::AppleLayer1FusionIdempotent)
    }

    const fn stop_lane(self) -> ProductionReplayLane {
        match self {
            Self::AppleLayer1LocalFinalPass => ProductionReplayLane::LocalFinalPass,
            _ => ProductionReplayLane::AppleLexicon,
        }
    }
}

impl std::fmt::Display for ReplayProfile {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.token())
    }
}

impl FromStr for ReplayProfile {
    type Err = String;

    fn from_str(raw: &str) -> std::result::Result<Self, Self::Err> {
        let normalized = raw.trim().replace('_', "-");
        [
            Self::AppleLayer0,
            Self::AppleLayer1Inprocess,
            Self::AppleLayer1Sidecar,
            Self::AppleLayer1Remote,
            Self::AppleLayer1FusionUtterance,
            Self::AppleLayer1FusionLeftPad,
            Self::AppleLayer1FusionStablePrompt,
            Self::AppleLayer1FusionIdempotent,
            Self::AppleLayer1LocalFinalPass,
        ]
        .into_iter()
        .find(|profile| profile.token() == normalized)
        .ok_or_else(|| format!("unknown replay profile {raw:?}"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReferenceKind {
    Human,
    HistoricalSameStem,
}

impl ReferenceKind {
    const fn rank(self) -> u8 {
        match self {
            Self::Human => 0,
            Self::HistoricalSameStem => 1,
        }
    }
}

#[derive(Debug, Clone)]
struct Reference {
    path: PathBuf,
    sha256: String,
    kind: ReferenceKind,
}

#[derive(Debug, Clone)]
struct Clip {
    path: PathBuf,
    sha256: String,
    reference: Option<Reference>,
    has_apple_reference: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CorpusCensus {
    schema: String,
    generated_at: String,
    root_count: usize,
    discovered_audio_instances: usize,
    distinct_audio: usize,
    duplicate_instances: usize,
    format_instances: BTreeMap<String, usize>,
    human_reference_instances: usize,
    historical_reference_instances: usize,
    apple_reference_instances: usize,
    distinct_human_paired: usize,
    distinct_historical_paired: usize,
    distinct_apple_referenced: usize,
    distinct_unpaired: usize,
    selected_distinct: usize,
    reference_policy: String,
    privacy: PrivacyContract,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PrivacyContract {
    source_paths_emitted: bool,
    source_filenames_emitted: bool,
    transcript_bodies_emitted: bool,
    opaque_ids_are_hash_prefixes: bool,
}

impl Default for PrivacyContract {
    fn default() -> Self {
        Self {
            source_paths_emitted: false,
            source_filenames_emitted: false,
            transcript_bodies_emitted: false,
            opaque_ids_are_hash_prefixes: true,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct ProfileReport {
    schema: String,
    engine_contract: String,
    generated_at: String,
    commit: String,
    profile: ReplayProfile,
    reference_policy: String,
    corpus: CorpusCensus,
    distinct_recordings: usize,
    requested_runs_per_recording: usize,
    requested_executions: usize,
    successful_executions: usize,
    failed_executions: usize,
    total_audio_seconds_executed: f64,
    total_tail_patches: usize,
    requested_layered: bool,
    observed_layered: bool,
    profile_observation_matches: bool,
    mean_wer: Option<f64>,
    mean_cer: Option<f64>,
    mean_character_parity: Option<f64>,
    input_hashes_unchanged: bool,
    settings_loaded: bool,
    dotenv_loaded: bool,
    keychain_disabled: bool,
    apple_stt_bridge: FileFingerprint,
    quality_html: String,
    rows: Vec<ExecutionRow>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ExecutionRow {
    opaque_id: String,
    run: usize,
    audio_sha256: String,
    reference_sha256: String,
    reference_kind: ReferenceKind,
    duration_seconds: f64,
    sample_rate_hz: u32,
    status: String,
    error_class: Option<String>,
    wall_seconds: f64,
    events: usize,
    previews: usize,
    sealed_finals: usize,
    final_count: usize,
    unique_final_id_count: usize,
    repeated_final_id_count: usize,
    overlapping_final_window_count: usize,
    tail_patches: usize,
    layer1_provider_armed: bool,
    live_chars: usize,
    adjudicated_chars: usize,
    delivered_chars: usize,
    reference_tokens: usize,
    delivered_tokens: usize,
    token_ratio: f64,
    head_present: bool,
    tail_present: bool,
    wer: f64,
    cer: f64,
    character_parity: f64,
    teacher_similarity: f64,
    final_pass_attempted: bool,
    final_pass_skipped: bool,
    lexicon_rewrites: u64,
    gate_drops: u64,
    audio_hash_unchanged: bool,
    reference_hash_unchanged: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct FileFingerprint {
    label: String,
    exists: bool,
    sha256: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ProfileStatus {
    profile: ReplayProfile,
    worker_exit: Option<i32>,
    report_present: bool,
    successful_executions: usize,
    failed_executions: usize,
    observed_layered: Option<bool>,
    mean_wer: Option<f64>,
    mean_cer: Option<f64>,
    mean_character_parity: Option<f64>,
    quality_html: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct MatrixReport {
    schema: String,
    engine_contract: String,
    generated_at: String,
    commit: String,
    corpus: CorpusCensus,
    requested_profiles: usize,
    completed_profiles: usize,
    distinct_recordings: usize,
    requested_runs_per_recording: usize,
    requested_executions: usize,
    successful_executions: usize,
    failed_executions: usize,
    profile_status: Vec<ProfileStatus>,
    configuration_files_before: Vec<FileFingerprint>,
    configuration_files_after: Vec<FileFingerprint>,
    configuration_files_unchanged: bool,
    operator_settings_loaded: bool,
    operator_dotenv_loaded: bool,
    keychain_disabled: bool,
    apple_stt_bridge: FileFingerprint,
    permission_request_apis_called_by_tool: bool,
    tcc_database_inspected: bool,
    permission_state_proven_unchanged: bool,
    quality_gate: &'static str,
    coverage: CoverageContract,
}

#[derive(Debug, Serialize, Deserialize)]
struct CoverageContract {
    production_pcm_session_replay: &'static str,
    production_stop_adjudication: &'static str,
    production_lexicon_delivery: &'static str,
    coreaudio_microphone_capture: &'static str,
    blackhole_loopback_capture: &'static str,
    hold_toggle_hotkey_modes: &'static str,
    clipboard_paste_and_target_app: &'static str,
    cloud_gateway: &'static str,
    inline_llm_formatting: &'static str,
    tcc_permissions: &'static str,
}

impl Default for CoverageContract {
    fn default() -> Self {
        Self {
            production_pcm_session_replay: "covered",
            production_stop_adjudication: "covered",
            production_lexicon_delivery: "covered",
            coreaudio_microphone_capture: "not_covered_by_file_replay",
            blackhole_loopback_capture: "not_covered_by_file_replay",
            hold_toggle_hotkey_modes: "not_covered_by_file_replay",
            clipboard_paste_and_target_app: "not_covered_by_file_replay",
            cloud_gateway: "covered_only_when_apple_layer1_remote_is_requested_and_configured",
            inline_llm_formatting: "not_covered_by_current_replay_seam",
            tcc_permissions: "not_mutated_or_fully_verified",
        }
    }
}

fn main() -> ExitCode {
    // SAFETY: this is the first executable statement, before Clap parsing,
    // runtime construction or thread creation. Corpus tooling must be unable
    // to read, write or prompt for the operator's production Keychain even if
    // a future replay dependency unexpectedly reaches Config/secret code.
    unsafe {
        std::env::set_var("CODESCRIBE_DISABLE_KEYCHAIN", "1");
    }
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("codescribe-corpus: {error:#}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Census {
            roots,
            out,
            include_historical,
        } => {
            let policy = if include_historical {
                ReferencePolicy::HumanAndHistorical
            } else {
                ReferencePolicy::Human
            };
            let discovery = discover_corpus(&roots, policy, None)?;
            atomic_write_json(&out, &discovery.census)?;
            atomic_write(
                &markdown_sibling(&out),
                census_markdown(&discovery.census).as_bytes(),
            )?;
            println!(
                "corpus census: instances={} distinct={} selected={} report={}",
                discovery.census.discovered_audio_instances,
                discovery.census.distinct_audio,
                discovery.census.selected_distinct,
                out.display()
            );
            Ok(())
        }
        Command::Run {
            roots,
            out_dir,
            profiles,
            runs,
            references,
            max_recordings,
            language,
            commit,
            apple_bridge,
        } => run_matrix(MatrixArgs {
            roots,
            out_dir,
            profiles,
            runs,
            references,
            max_recordings,
            language,
            commit,
            apple_bridge,
        }),
        Command::Worker {
            roots,
            out,
            profile,
            runs,
            references,
            max_recordings,
            language,
            commit,
            apple_bridge,
        } => {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("build replay runtime")?;
            runtime.block_on(run_worker(WorkerArgs {
                roots,
                out,
                profile,
                runs,
                references,
                max_recordings,
                language,
                commit,
                apple_bridge,
            }))
        }
    }
}

struct Discovery {
    census: CorpusCensus,
    selected: Vec<Clip>,
}

fn discover_corpus(
    roots: &[PathBuf],
    policy: ReferencePolicy,
    max_recordings: Option<usize>,
) -> Result<Discovery> {
    if roots.is_empty() {
        bail!("at least one corpus root is required");
    }
    let mut instances = Vec::new();
    for root in roots {
        if !root.is_dir() {
            bail!("corpus root is not a directory: {}", root.display());
        }
        walk_audio(root, &mut instances)?;
    }
    instances.sort();

    let mut format_instances = BTreeMap::new();
    let mut human_reference_instances = 0usize;
    let mut historical_reference_instances = 0usize;
    let mut apple_reference_instances = 0usize;
    let mut distinct = BTreeMap::<String, Clip>::new();

    for path in &instances {
        let extension = lower_extension(path).unwrap_or_else(|| "unknown".to_string());
        *format_instances.entry(extension.clone()).or_insert(0) += 1;
        let audio_sha256 = sha256_file(path)?;
        let human_path = reference_path(path, "_human_transcription.txt");
        let historical_path = path.with_extension("txt");
        let apple_path = reference_path(path, "_apple_live_reference.txt");
        let human = human_path.filter(|candidate| candidate.is_file());
        let historical = historical_path.is_file().then_some(historical_path);
        let has_apple_reference = apple_path.is_some_and(|candidate| candidate.is_file());
        human_reference_instances += usize::from(human.is_some());
        historical_reference_instances += usize::from(historical.is_some());
        apple_reference_instances += usize::from(has_apple_reference);

        let reference = if let Some(reference_path) = human {
            Some(Reference {
                sha256: sha256_file(&reference_path)?,
                path: reference_path,
                kind: ReferenceKind::Human,
            })
        } else if matches!(policy, ReferencePolicy::HumanAndHistorical) {
            if let Some(reference_path) = historical {
                Some(Reference {
                    sha256: sha256_file(&reference_path)?,
                    path: reference_path,
                    kind: ReferenceKind::HistoricalSameStem,
                })
            } else {
                None
            }
        } else {
            None
        };

        let incoming = Clip {
            path: path.clone(),
            sha256: audio_sha256.clone(),
            reference,
            has_apple_reference,
        };
        match distinct.get_mut(&audio_sha256) {
            Some(existing) => merge_duplicate(existing, incoming)?,
            None => {
                distinct.insert(audio_sha256, incoming);
            }
        }
    }

    let distinct_audio = distinct.len();
    let distinct_human_paired = distinct
        .values()
        .filter(|clip| {
            matches!(
                clip.reference.as_ref().map(|reference| reference.kind),
                Some(ReferenceKind::Human)
            )
        })
        .count();
    let distinct_historical_paired = distinct
        .values()
        .filter(|clip| {
            matches!(
                clip.reference.as_ref().map(|reference| reference.kind),
                Some(ReferenceKind::HistoricalSameStem)
            )
        })
        .count();
    let distinct_apple_referenced = distinct
        .values()
        .filter(|clip| clip.has_apple_reference)
        .count();
    let distinct_unpaired = distinct
        .values()
        .filter(|clip| clip.reference.is_none())
        .count();
    let mut selected = distinct
        .into_values()
        .filter(|clip| clip.reference.is_some())
        .collect::<Vec<_>>();
    selected.sort_by(|left, right| left.sha256.cmp(&right.sha256));
    if let Some(limit) = max_recordings {
        selected.truncate(limit);
    }

    let census = CorpusCensus {
        schema: REPORT_SCHEMA.to_string(),
        generated_at: Utc::now().to_rfc3339(),
        root_count: roots.len(),
        discovered_audio_instances: instances.len(),
        distinct_audio,
        duplicate_instances: instances.len().saturating_sub(distinct_audio),
        format_instances,
        human_reference_instances,
        historical_reference_instances,
        apple_reference_instances,
        distinct_human_paired,
        distinct_historical_paired,
        distinct_apple_referenced,
        distinct_unpaired,
        selected_distinct: selected.len(),
        reference_policy: policy.as_str().to_string(),
        privacy: PrivacyContract::default(),
    };
    Ok(Discovery { census, selected })
}

fn walk_audio(directory: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries = fs::read_dir(directory)
        .with_context(|| format!("read corpus directory {}", directory.display()))?
        .collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            walk_audio(&path, output)?;
        } else if file_type.is_file() && is_audio(&path) {
            output.push(path);
        }
    }
    Ok(())
}

fn is_audio(path: &Path) -> bool {
    lower_extension(path).is_some_and(|extension| AUDIO_EXTENSIONS.contains(&extension.as_str()))
}

fn lower_extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase)
}

fn reference_path(audio: &Path, suffix: &str) -> Option<PathBuf> {
    let stem = audio.file_stem()?.to_str()?;
    Some(audio.parent()?.join(format!("{stem}{suffix}")))
}

fn merge_duplicate(existing: &mut Clip, incoming: Clip) -> Result<()> {
    existing.has_apple_reference |= incoming.has_apple_reference;
    match (&existing.reference, &incoming.reference) {
        (Some(left), Some(right)) if left.kind == right.kind && left.sha256 != right.sha256 => {
            bail!(
                "duplicate audio hash has conflicting {:?} reference hashes",
                left.kind
            );
        }
        (Some(left), Some(right)) if right.kind.rank() < left.kind.rank() => {
            existing.reference = Some(right.clone());
        }
        (None, Some(reference)) => existing.reference = Some(reference.clone()),
        _ => {}
    }
    Ok(())
}

struct MatrixArgs {
    roots: Vec<PathBuf>,
    out_dir: PathBuf,
    profiles: Vec<ReplayProfile>,
    runs: usize,
    references: ReferencePolicy,
    max_recordings: Option<usize>,
    language: String,
    commit: String,
    apple_bridge: PathBuf,
}

fn run_matrix(args: MatrixArgs) -> Result<()> {
    if args.runs == 0 {
        bail!("--runs must be greater than zero");
    }
    if args.profiles.is_empty() {
        bail!("at least one replay profile is required");
    }
    let apple_stt_bridge = fingerprint_file("apple_stt_bridge", &args.apple_bridge)?;
    if !apple_stt_bridge.exists {
        bail!("--apple-bridge must name an existing file");
    }
    fs::create_dir_all(&args.out_dir)
        .with_context(|| format!("create report directory {}", args.out_dir.display()))?;
    let discovery = discover_corpus(&args.roots, args.references, args.max_recordings)?;
    if discovery.selected.is_empty() {
        bail!(
            "no recordings match reference policy {}",
            args.references.as_str()
        );
    }

    let config_before = operator_configuration_fingerprints()?;
    let current_exe = std::env::current_exe().context("resolve corpus runner executable")?;
    let mut profile_status = Vec::with_capacity(args.profiles.len());
    let mut completed_profiles = 0usize;
    let mut successful_executions = 0usize;
    let mut failed_executions = 0usize;

    for profile in &args.profiles {
        let profile_out = args
            .out_dir
            .join(format!("profile-{}.json", profile.token()));
        let runtime_dir = args.out_dir.join("runtime").join(profile.token());
        fs::create_dir_all(&runtime_dir)?;
        let mut child = ProcessCommand::new(&current_exe);
        child
            .arg("worker")
            .arg("--out")
            .arg(&profile_out)
            .arg("--profile")
            .arg(profile.token())
            .arg("--runs")
            .arg(args.runs.to_string())
            .arg("--references")
            .arg(args.references.as_str().replace('_', "-"))
            .arg("--language")
            .arg(&args.language)
            .arg("--commit")
            .arg(&args.commit)
            .arg("--apple-bridge")
            .arg(&args.apple_bridge)
            .env("CODESCRIBE_DATA_DIR", &runtime_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        if let Some(limit) = args.max_recordings {
            child.arg("--max-recordings").arg(limit.to_string());
        }
        for root in &args.roots {
            child.arg("--root").arg(root);
        }
        configure_profile_environment(&mut child, *profile, &args.apple_bridge);
        let status = child
            .status()
            .with_context(|| format!("launch profile {}", profile.token()))?;
        let report = fs::read(&profile_out)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<ProfileReport>(&bytes).ok());
        if report.is_some() {
            completed_profiles += 1;
        }
        let successful = report
            .as_ref()
            .map_or(0, |profile_report| profile_report.successful_executions);
        let failed = report
            .as_ref()
            .map_or(discovery.selected.len() * args.runs, |profile_report| {
                profile_report.failed_executions
            });
        successful_executions += successful;
        failed_executions += failed;
        profile_status.push(ProfileStatus {
            profile: *profile,
            worker_exit: status.code(),
            report_present: report.is_some(),
            successful_executions: successful,
            failed_executions: failed,
            observed_layered: report
                .as_ref()
                .map(|profile_report| profile_report.observed_layered),
            mean_wer: report
                .as_ref()
                .and_then(|profile_report| profile_report.mean_wer),
            mean_cer: report
                .as_ref()
                .and_then(|profile_report| profile_report.mean_cer),
            mean_character_parity: report
                .as_ref()
                .and_then(|profile_report| profile_report.mean_character_parity),
            quality_html: report
                .as_ref()
                .map(|profile_report| profile_report.quality_html.clone()),
        });
    }

    let config_after = operator_configuration_fingerprints()?;
    let config_unchanged = config_before == config_after;
    let requested_executions = discovery.selected.len() * args.runs * args.profiles.len();
    let matrix = MatrixReport {
        schema: REPORT_SCHEMA.to_string(),
        engine_contract: ENGINE_CONTRACT_ID.to_string(),
        generated_at: Utc::now().to_rfc3339(),
        commit: args.commit,
        corpus: discovery.census,
        requested_profiles: args.profiles.len(),
        completed_profiles,
        distinct_recordings: discovery.selected.len(),
        requested_runs_per_recording: args.runs,
        requested_executions,
        successful_executions,
        failed_executions,
        profile_status,
        configuration_files_before: config_before,
        configuration_files_after: config_after,
        configuration_files_unchanged: config_unchanged,
        operator_settings_loaded: false,
        operator_dotenv_loaded: false,
        keychain_disabled: true,
        apple_stt_bridge,
        permission_request_apis_called_by_tool: false,
        tcc_database_inspected: false,
        permission_state_proven_unchanged: false,
        quality_gate: "measurement_only_operator_decides",
        coverage: CoverageContract::default(),
    };
    let report_path = args.out_dir.join("report.json");
    atomic_write_json(&report_path, &matrix)?;
    atomic_write(
        &args.out_dir.join("report.md"),
        matrix_markdown(&matrix).as_bytes(),
    )?;
    println!(
        "corpus parity: distinct={} executions={}/{} config_unchanged={} report={}",
        matrix.distinct_recordings,
        matrix.successful_executions,
        matrix.requested_executions,
        matrix.configuration_files_unchanged,
        report_path.display()
    );
    if completed_profiles != args.profiles.len() {
        bail!(
            "{} of {} profile workers failed to publish a report",
            args.profiles.len() - completed_profiles,
            args.profiles.len()
        );
    }
    Ok(())
}

fn configure_profile_environment(
    command: &mut ProcessCommand,
    profile: ReplayProfile,
    apple_bridge: &Path,
) {
    for key in CONTROLLED_ENV {
        command.env_remove(key);
    }
    if !matches!(profile, ReplayProfile::AppleLayer1Remote) {
        command.env_remove("STT_API_KEY");
        command.env_remove("STT_ENDPOINT");
        command.env_remove("CODESCRIBE_STT_ENDPOINT");
    }
    command
        .env("CODESCRIBE_DISABLE_KEYCHAIN", "1")
        .env("CODESCRIBE_STT_ENGINE", "apple")
        .env("CODESCRIBE_APPLE_STT_BRIDGE", apple_bridge)
        .env("CODESCRIBE_BRIDGE_DISCLAIM", "1")
        .env(
            "CODESCRIBE_LAYERED_TRANSCRIPTION",
            if profile.layered() { "phase1" } else { "off" },
        )
        .env("STT_TAIL_PROVIDER", profile.tail_provider())
        .env(
            "CODESCRIBE_SILERO_FUSION",
            if profile.fusion() { "on" } else { "off" },
        )
        .env("CODESCRIBE_SILERO_FUSION_CONTEXT", profile.fusion_context())
        .env(
            "CODESCRIBE_SPAN_IDEMPOTENCE",
            if profile.idempotence() { "on" } else { "off" },
        )
        .env("CODESCRIBE_INLINE_FORMAT", "off")
        .env("CODESCRIBE_STT_INITIAL_PROMPT_ENABLED", "off")
        .env("CODESCRIBE_APPLE_STT_ALLOW_DOWNLOAD", "0")
        .env(
            "FINAL_PASS_MODE",
            if matches!(profile, ReplayProfile::AppleLayer1LocalFinalPass) {
                "always"
            } else {
                "off"
            },
        )
        .env(
            "CODESCRIBE_LOCAL_STT_FINAL_PASS",
            if matches!(profile, ReplayProfile::AppleLayer1LocalFinalPass) {
                "1"
            } else {
                "0"
            },
        );
}

struct WorkerArgs {
    roots: Vec<PathBuf>,
    out: PathBuf,
    profile: ReplayProfile,
    runs: usize,
    references: ReferencePolicy,
    max_recordings: Option<usize>,
    language: String,
    commit: String,
    apple_bridge: PathBuf,
}

async fn run_worker(args: WorkerArgs) -> Result<()> {
    if args.runs == 0 {
        bail!("--runs must be greater than zero");
    }
    validate_worker_environment(args.profile, &args.apple_bridge)?;
    codescribe_core::stt::apple_stt::ensure_noninteractive_ready(Some(&args.language))
        .context("noninteractive Apple STT preflight")?;
    let discovery = discover_corpus(&args.roots, args.references, args.max_recordings)?;
    if discovery.selected.is_empty() {
        bail!("worker selected no recordings");
    }

    let mut settings = UserSettings {
        stt_engine: Some("apple".to_string()),
        layered_transcription: Some(if args.profile.layered() {
            "phase1".to_string()
        } else {
            "off".to_string()
        }),
        final_pass_mode: Some(
            if matches!(args.profile, ReplayProfile::AppleLayer1LocalFinalPass) {
                "always"
            } else {
                "off"
            }
            .to_string(),
        ),
        ..UserSettings::default()
    };
    // The cloud product session is a separate gateway surface. Tail-patch
    // provider profiles are controlled by the explicit process environment.
    settings.asr_mode = Some("apple_only".to_string());

    let output_root = args
        .out
        .parent()
        .ok_or_else(|| anyhow!("profile report path has no parent"))?;
    let quality_dir = output_root.join("quality");
    let quality_audio_dir = quality_dir.join("audio");
    fs::create_dir_all(&quality_audio_dir).context("create private quality report directory")?;
    make_private_directory(&quality_dir)?;
    make_private_directory(&quality_audio_dir)?;

    let mut rows = Vec::with_capacity(discovery.selected.len() * args.runs);
    let mut quality_entries = Vec::with_capacity(discovery.selected.len() * args.runs);
    let mut total_audio_seconds_executed = 0.0;
    for clip in &discovery.selected {
        let reference = clip
            .reference
            .as_ref()
            .expect("selected clips have references");
        let truth = fs::read_to_string(&reference.path).context("read paired reference")?;
        let (samples, sample_rate) = codescribe_core::audio::load_audio_file(&clip.path)
            .map_err(|_| anyhow!("decode replay audio failed"))?;
        if samples.is_empty() || sample_rate == 0 {
            bail!("selected replay audio is empty or has zero sample rate");
        }
        let quality_audio_rel = publish_quality_audio(&quality_audio_dir, clip)?;
        let duration_seconds = samples.len() as f64 / f64::from(sample_rate);
        eprintln!(
            "corpus replay selected: profile={} recording={} duration_seconds={duration_seconds:.3} runs={}",
            args.profile.token(),
            opaque_id(&clip.sha256),
            args.runs
        );
        for run in 1..=args.runs {
            let started = Instant::now();
            let replay = replay_overlay_recording(
                &clip.path,
                Some(args.language.clone()),
                &settings,
                args.profile.stop_lane(),
            )
            .await;
            let wall_seconds = started.elapsed().as_secs_f64();
            total_audio_seconds_executed += duration_seconds;
            let execution = ReplayExecutionContext {
                clip,
                reference,
                truth: &truth,
                run,
                profile: args.profile,
                duration_seconds,
                sample_rate,
                wall_seconds,
                audio_rel_path: &quality_audio_rel,
            };
            match replay {
                Ok(replay) => {
                    quality_entries.push(success_quality_entry(&execution, &replay));
                    rows.push(success_row(&execution, replay)?);
                }
                Err(error) => {
                    quality_entries.push(failure_quality_entry(&execution, &format!("{error:#}")));
                    rows.push(failure_row(&execution)?);
                }
            }
            eprintln!(
                "corpus replay: profile={} recording={} run={}/{} status={}",
                args.profile.token(),
                opaque_id(&clip.sha256),
                run,
                args.runs,
                rows.last().map_or("missing", |row| row.status.as_str())
            );
        }
    }

    let successful = rows.iter().filter(|row| row.status == "ok").count();
    let failed = rows.len() - successful;
    let total_tail_patches = rows.iter().map(|row| row.tail_patches).sum();
    let successful_rows = rows
        .iter()
        .filter(|row| row.status == "ok")
        .collect::<Vec<_>>();
    let (observed_layered, profile_observation_matches) = layering_observation(
        args.profile.layered(),
        successful_rows.iter().map(|row| row.layer1_provider_armed),
    );
    let mean_wer = mean(successful_rows.iter().map(|row| row.wer));
    let mean_cer = mean(successful_rows.iter().map(|row| row.cer));
    let mean_character_parity = mean(successful_rows.iter().map(|row| row.character_parity));
    let input_hashes_unchanged = rows
        .iter()
        .all(|row| row.audio_hash_unchanged && row.reference_hash_unchanged);
    let quality_html = format!("quality/seal-atlas.{}.html", args.profile.token());
    let qube_html = format!("quality/qube.{}.html", args.profile.token());
    let quality_report = build_quality_report(
        args.profile,
        &args.language,
        args.references,
        quality_entries,
    );
    let quality_config = QualityReportConfig {
        input_dir: args.roots[0].clone(),
        output_dir: quality_dir.clone(),
        date_filter: None,
        limit: 0,
        language: Some(args.language.clone()),
        skip_cloud: true,
        cloud_concurrency: 0,
        skip_formatting: true,
        debug_mode: true,
        copy_audio: false,
        metrics_reference: MetricsReference::Corpus,
        local_transcription: LocalTranscriptionMode::LocalWhisper,
    };
    let atlas = SealAtlasPage {
        title: format!("Seal Atlas — corpus {}", args.profile.token()),
        lede: format!(
            "Corpus profile {}. One take, one PCM clock. Words from SealedSpan.words when a dump is attached — not from the final string.",
            args.profile.token()
        ),
        stats: SealAtlasStats {
            sealed_spans: quality_report.entries.len().to_string(),
            ..SealAtlasStats::default()
        },
        findings: vec![format!(
            "Qube scores (footnote) live at {}. Avg WER is not the live engine.",
            qube_html
        )],
        dump_present: false,
    };
    atomic_write_private(
        &output_root.join(&quality_html),
        render_seal_atlas_html(&atlas).as_bytes(),
    )?;
    atomic_write_private(
        &output_root.join(&qube_html),
        render_qube_html(&quality_report, &quality_config).as_bytes(),
    )?;

    let report = ProfileReport {
        schema: REPORT_SCHEMA.to_string(),
        engine_contract: ENGINE_CONTRACT_ID.to_string(),
        generated_at: Utc::now().to_rfc3339(),
        commit: args.commit,
        profile: args.profile,
        reference_policy: args.references.as_str().to_string(),
        corpus: discovery.census,
        distinct_recordings: discovery.selected.len(),
        requested_runs_per_recording: args.runs,
        requested_executions: discovery.selected.len() * args.runs,
        successful_executions: successful,
        failed_executions: failed,
        total_audio_seconds_executed,
        total_tail_patches,
        requested_layered: args.profile.layered(),
        observed_layered,
        profile_observation_matches,
        mean_wer,
        mean_cer,
        mean_character_parity,
        input_hashes_unchanged,
        settings_loaded: false,
        dotenv_loaded: false,
        keychain_disabled: true,
        apple_stt_bridge: fingerprint_file("apple_stt_bridge", &args.apple_bridge)?,
        quality_html,
        rows,
    };
    atomic_write_json(&args.out, &report)?;
    Ok(())
}

fn layering_observation(
    requested_layered: bool,
    provider_armed: impl IntoIterator<Item = bool>,
) -> (bool, bool) {
    let mut successful_executions = 0usize;
    let mut observed_layered = false;
    let mut every_execution_matches = true;
    for armed in provider_armed {
        successful_executions += 1;
        observed_layered |= armed;
        every_execution_matches &= armed == requested_layered;
    }
    (
        observed_layered,
        successful_executions > 0 && every_execution_matches,
    )
}

fn publish_quality_audio(quality_audio_dir: &Path, clip: &Clip) -> Result<String> {
    let extension = clip
        .path
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or("wav")
        .to_ascii_lowercase();
    let file_name = format!("{}.{}", opaque_id(&clip.sha256), extension);
    let published = quality_audio_dir.join(&file_name);
    if fs::symlink_metadata(&published).is_ok() {
        let existing = fs::canonicalize(&published)
            .with_context(|| format!("resolve quality audio link {}", published.display()))?;
        let expected = fs::canonicalize(&clip.path)
            .with_context(|| format!("resolve corpus audio {}", clip.path.display()))?;
        if existing != expected {
            bail!(
                "quality audio link collision for {}",
                opaque_id(&clip.sha256)
            );
        }
    } else {
        let source_root = clip
            .path
            .parent()
            .ok_or_else(|| anyhow!("quality audio source has no parent"))?;
        safe_symlink_or_copy_bounded(&clip.path, source_root, &published, quality_audio_dir)
            .with_context(|| format!("publish private quality audio {}", published.display()))?;
    }
    Ok(format!("audio/{file_name}"))
}

struct ReplayExecutionContext<'a> {
    clip: &'a Clip,
    reference: &'a Reference,
    truth: &'a str,
    run: usize,
    profile: ReplayProfile,
    duration_seconds: f64,
    sample_rate: u32,
    wall_seconds: f64,
    audio_rel_path: &'a str,
}

fn success_quality_entry(
    execution: &ReplayExecutionContext<'_>,
    replay: &codescribe::controller::production_replay::ProductionOverlayReplay,
) -> ReportEntry {
    let raw_wer = word_error_rate(execution.truth, &replay.live_text) as f32;
    let raw_cer = character_error_rate(execution.truth, &replay.live_text) as f32;
    let post_wer = word_error_rate(execution.truth, &replay.delivered_text) as f32;
    let post_cer = character_error_rate(execution.truth, &replay.delivered_text) as f32;
    let raw_state = if replay.live_text.trim().is_empty() {
        ReportTranscriptState::EmptyTranscript
    } else {
        ReportTranscriptState::TextCommitted
    };
    ReportEntry {
        id: format!(
            "{}-run{}-{}",
            opaque_id(&execution.clip.sha256),
            execution.run,
            execution.profile.token()
        ),
        audio_path: opaque_id(&execution.clip.sha256),
        audio_rel_path: execution.audio_rel_path.to_string(),
        reference_path: None,
        duration_secs: execution.duration_seconds as f32,
        transcripts: ReportTranscripts {
            raw: Some(replay.live_text.clone()),
            post: Some(replay.delivered_text.clone()),
            ai_formatted: None,
            cloud: None,
            reference: Some(execution.truth.to_string()),
        },
        raw_semantics: Some(ReportTranscriptSemantics {
            state: raw_state,
            reason: None,
        }),
        metrics: ReportMetrics {
            raw_wer: Some(raw_wer),
            raw_cer: Some(raw_cer),
            post_wer: Some(post_wer),
            post_cer: Some(post_cer),
            ..ReportMetrics::default()
        },
        postprocess_stats: Some(replay.postprocess_stats.clone()),
        errors: Vec::new(),
    }
}

fn failure_quality_entry(execution: &ReplayExecutionContext<'_>, error: &str) -> ReportEntry {
    ReportEntry {
        id: format!(
            "{}-run{}-{}",
            opaque_id(&execution.clip.sha256),
            execution.run,
            execution.profile.token()
        ),
        audio_path: opaque_id(&execution.clip.sha256),
        audio_rel_path: execution.audio_rel_path.to_string(),
        reference_path: None,
        duration_secs: execution.duration_seconds as f32,
        transcripts: ReportTranscripts {
            reference: Some(execution.truth.to_string()),
            ..ReportTranscripts::default()
        },
        raw_semantics: None,
        metrics: ReportMetrics::default(),
        postprocess_stats: None,
        errors: vec![error.to_string()],
    }
}

fn build_quality_report(
    profile: ReplayProfile,
    language: &str,
    references: ReferencePolicy,
    entries: Vec<ReportEntry>,
) -> QualityReport {
    let raw_wer = entries
        .iter()
        .filter_map(|entry| entry.metrics.raw_wer)
        .collect::<Vec<_>>();
    let raw_cer = entries
        .iter()
        .filter_map(|entry| entry.metrics.raw_cer)
        .collect::<Vec<_>>();
    let post_wer = entries
        .iter()
        .filter_map(|entry| entry.metrics.post_wer)
        .collect::<Vec<_>>();
    let post_cer = entries
        .iter()
        .filter_map(|entry| entry.metrics.post_cer)
        .collect::<Vec<_>>();
    let raw_text_committed = entries
        .iter()
        .filter(|entry| {
            matches!(
                entry
                    .raw_semantics
                    .as_ref()
                    .map(|semantics| semantics.state),
                Some(ReportTranscriptState::TextCommitted)
            )
        })
        .count();
    let raw_no_speech_detected = entries
        .iter()
        .filter(|entry| {
            matches!(
                entry
                    .raw_semantics
                    .as_ref()
                    .map(|semantics| semantics.state),
                Some(ReportTranscriptState::NoSpeechDetected)
            )
        })
        .count();
    let raw_quality_gate_dropped = entries
        .iter()
        .filter(|entry| {
            matches!(
                entry
                    .raw_semantics
                    .as_ref()
                    .map(|semantics| semantics.state),
                Some(ReportTranscriptState::QualityGateDropped)
            )
        })
        .count();
    QualityReport {
        generated_at: Utc::now().to_rfc3339(),
        environment: ReportEnvironment {
            stt_endpoint: None,
            stt_api_key_present: false,
            llm_formatting_endpoint: None,
            llm_formatting_model: None,
            llm_formatting_key_present: false,
            local_model: None,
            whisper_language: Some(language.to_string()),
            metrics_reference: references.as_str().to_string(),
            local_transcription: format!("production_overlay:{}", profile.token()),
        },
        summary: ReportSummary {
            total_files: entries.len(),
            processed_files: entries
                .iter()
                .filter(|entry| entry.errors.is_empty())
                .count(),
            avg_raw_wer: mean_f32(&raw_wer),
            avg_post_wer: mean_f32(&post_wer),
            avg_raw_cer: mean_f32(&raw_cer),
            avg_post_cer: mean_f32(&post_cer),
            raw_no_speech_detected,
            raw_quality_gate_dropped,
            raw_text_committed,
            ..ReportSummary::default()
        },
        entries,
    }
}

fn mean_f32(values: &[f32]) -> Option<f32> {
    (!values.is_empty()).then(|| values.iter().sum::<f32>() / values.len() as f32)
}

fn make_private_directory(path: &Path) -> Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).with_context(|| {
        format!(
            "set private report directory permissions {}",
            path.display()
        )
    })
}

fn validate_worker_environment(profile: ReplayProfile, apple_bridge: &Path) -> Result<()> {
    let expected = [
        ("CODESCRIBE_STT_ENGINE", "apple"),
        (
            "CODESCRIBE_LAYERED_TRANSCRIPTION",
            if profile.layered() { "phase1" } else { "off" },
        ),
        ("STT_TAIL_PROVIDER", profile.tail_provider()),
        (
            "CODESCRIBE_SILERO_FUSION",
            if profile.fusion() { "on" } else { "off" },
        ),
        (
            "CODESCRIBE_SPAN_IDEMPOTENCE",
            if profile.idempotence() { "on" } else { "off" },
        ),
        ("CODESCRIBE_INLINE_FORMAT", "off"),
        ("CODESCRIBE_DISABLE_KEYCHAIN", "1"),
        ("CODESCRIBE_APPLE_STT_ALLOW_DOWNLOAD", "0"),
        ("CODESCRIBE_BRIDGE_DISCLAIM", "1"),
    ];
    for (key, value) in expected {
        if std::env::var(key).as_deref() != Ok(value) {
            bail!("worker environment pin mismatch for {key}");
        }
    }
    if std::env::var_os("CODESCRIBE_APPLE_STT_BRIDGE").as_deref() != Some(apple_bridge.as_os_str())
    {
        bail!("worker Apple STT bridge pin mismatch");
    }
    let data_dir = std::env::var_os("CODESCRIBE_DATA_DIR")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("worker requires isolated CODESCRIBE_DATA_DIR"))?;
    fs::create_dir_all(PathBuf::from(data_dir)).context("create isolated data directory")?;
    Ok(())
}

fn success_row(
    execution: &ReplayExecutionContext<'_>,
    replay: codescribe::controller::production_replay::ProductionOverlayReplay,
) -> Result<ExecutionRow> {
    let reference_tokens = normalized_words(execution.truth);
    let delivered_tokens = normalized_words(&replay.delivered_text);
    let head_present = reference_tokens
        .iter()
        .take(8)
        .any(|token| delivered_tokens.contains(token));
    let tail_present = reference_tokens
        .iter()
        .rev()
        .take(8)
        .any(|token| delivered_tokens.contains(token));
    let token_ratio = delivered_tokens.len() as f64 / reference_tokens.len().max(1) as f64;
    let previews = replay
        .events
        .iter()
        .filter(|event| matches!(event, EngineEvent::Preview { .. }))
        .count();
    let sealed_finals = replay
        .events
        .iter()
        .filter(|event| matches!(event, EngineEvent::UtteranceFinal { .. }))
        .count();
    let tail_patches = replay
        .events
        .iter()
        .filter(|event| {
            matches!(
                event,
                EngineEvent::ReplaceRange {
                    source: LayerSource::TailPatch,
                    ..
                }
            )
        })
        .count();
    let audio_hash_unchanged = sha256_file(&execution.clip.path)? == execution.clip.sha256;
    let reference_hash_unchanged =
        sha256_file(&execution.reference.path)? == execution.reference.sha256;
    Ok(ExecutionRow {
        opaque_id: opaque_id(&execution.clip.sha256),
        run: execution.run,
        audio_sha256: execution.clip.sha256.clone(),
        reference_sha256: execution.reference.sha256.clone(),
        reference_kind: execution.reference.kind,
        duration_seconds: execution.duration_seconds,
        sample_rate_hz: execution.sample_rate,
        status: "ok".to_string(),
        error_class: None,
        wall_seconds: execution.wall_seconds,
        events: replay.events.len(),
        previews,
        sealed_finals,
        final_count: replay.boundary_evidence.final_count,
        unique_final_id_count: replay.boundary_evidence.unique_final_id_count,
        repeated_final_id_count: replay.boundary_evidence.repeated_final_id_count,
        overlapping_final_window_count: replay.boundary_evidence.overlapping_final_window_count,
        tail_patches,
        layer1_provider_armed: replay.layer1_armed,
        live_chars: replay.live_text.chars().count(),
        adjudicated_chars: replay.adjudicated_text.chars().count(),
        delivered_chars: replay.delivered_text.chars().count(),
        reference_tokens: reference_tokens.len(),
        delivered_tokens: delivered_tokens.len(),
        token_ratio,
        head_present,
        tail_present,
        wer: word_error_rate(execution.truth, &replay.delivered_text),
        cer: character_error_rate(execution.truth, &replay.delivered_text),
        character_parity: normalized_character_parity(execution.truth, &replay.delivered_text),
        teacher_similarity: teacher_similarity(execution.truth, &replay.delivered_text),
        final_pass_attempted: replay.final_pass_attempted,
        final_pass_skipped: replay.final_pass_skipped,
        lexicon_rewrites: replay.postprocess_stats.lexicon_rewrites,
        gate_drops: replay.postprocess_stats.gate_drops,
        audio_hash_unchanged,
        reference_hash_unchanged,
    })
}

fn failure_row(execution: &ReplayExecutionContext<'_>) -> Result<ExecutionRow> {
    Ok(ExecutionRow {
        opaque_id: opaque_id(&execution.clip.sha256),
        run: execution.run,
        audio_sha256: execution.clip.sha256.clone(),
        reference_sha256: execution.reference.sha256.clone(),
        reference_kind: execution.reference.kind,
        duration_seconds: execution.duration_seconds,
        sample_rate_hz: execution.sample_rate,
        status: "error".to_string(),
        error_class: Some("production_replay_failed".to_string()),
        wall_seconds: execution.wall_seconds,
        events: 0,
        previews: 0,
        sealed_finals: 0,
        final_count: 0,
        unique_final_id_count: 0,
        repeated_final_id_count: 0,
        overlapping_final_window_count: 0,
        tail_patches: 0,
        layer1_provider_armed: false,
        live_chars: 0,
        adjudicated_chars: 0,
        delivered_chars: 0,
        reference_tokens: 0,
        delivered_tokens: 0,
        token_ratio: 0.0,
        head_present: false,
        tail_present: false,
        wer: 0.0,
        cer: 0.0,
        character_parity: 0.0,
        teacher_similarity: 0.0,
        final_pass_attempted: false,
        final_pass_skipped: false,
        lexicon_rewrites: 0,
        gate_drops: 0,
        audio_hash_unchanged: sha256_file(&execution.clip.path)? == execution.clip.sha256,
        reference_hash_unchanged: sha256_file(&execution.reference.path)?
            == execution.reference.sha256,
    })
}

fn edit_distance<T: Eq>(reference: &[T], hypothesis: &[T]) -> usize {
    let mut previous = (0..=hypothesis.len()).collect::<Vec<_>>();
    let mut current = vec![0; hypothesis.len() + 1];
    for (row, expected) in reference.iter().enumerate() {
        current[0] = row + 1;
        for (column, actual) in hypothesis.iter().enumerate() {
            current[column + 1] = if expected == actual {
                previous[column]
            } else {
                1 + previous[column]
                    .min(previous[column + 1])
                    .min(current[column])
            };
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[hypothesis.len()]
}

fn normalized_words(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn normalized_characters(text: &str) -> Vec<char> {
    text.chars()
        .flat_map(char::to_lowercase)
        .filter(|character| !character.is_whitespace())
        .collect()
}

fn word_error_rate(reference: &str, hypothesis: &str) -> f64 {
    let reference = normalized_words(reference);
    let hypothesis = normalized_words(hypothesis);
    edit_distance(&reference, &hypothesis) as f64 / reference.len().max(1) as f64
}

fn character_error_rate(reference: &str, hypothesis: &str) -> f64 {
    let reference = normalized_characters(reference);
    let hypothesis = normalized_characters(hypothesis);
    edit_distance(&reference, &hypothesis) as f64 / reference.len().max(1) as f64
}

fn normalized_character_parity(reference: &str, hypothesis: &str) -> f64 {
    let reference = normalized_characters(reference);
    let hypothesis = normalized_characters(hypothesis);
    let denominator = reference.len().max(hypothesis.len()).max(1);
    (1.0 - edit_distance(&reference, &hypothesis) as f64 / denominator as f64).clamp(0.0, 1.0)
}

fn teacher_similarity(reference: &str, hypothesis: &str) -> f64 {
    use codescribe_core::quality::teacher::{AlignOp, align_words, tokenize};

    let normalize = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let reference_tokens = tokenize(&normalize(reference));
    let hypothesis_tokens = tokenize(&normalize(hypothesis));
    let equal = align_words(&reference_tokens, &hypothesis_tokens)
        .iter()
        .filter(|operation| matches!(operation, AlignOp::Equal { .. }))
        .count();
    equal as f64 / reference_tokens.len().max(hypothesis_tokens.len()).max(1) as f64
}

fn mean(values: impl Iterator<Item = f64>) -> Option<f64> {
    let values = values.collect::<Vec<_>>();
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

fn opaque_id(sha256: &str) -> String {
    format!("audio-{}", &sha256[..sha256.len().min(16)])
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = safe_open(path).with_context(|| format!("open input {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn sha256_plist_semantics(path: &Path) -> Result<String> {
    let output = ProcessCommand::new("/usr/bin/plutil")
        .args(["-convert", "xml1", "-o", "-", "--"])
        .arg(path)
        .output()
        .with_context(|| format!("canonicalize preferences plist {}", path.display()))?;
    if !output.status.success() {
        bail!(
            "plutil could not canonicalize preferences plist {} (status={})",
            path.display(),
            output.status
        );
    }
    Ok(format!("{:x}", Sha256::digest(&output.stdout)))
}

fn operator_configuration_fingerprints() -> Result<Vec<FileFingerprint>> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("HOME is unavailable for configuration fingerprinting"))?;
    let mut fingerprints = [
        (
            "settings_json",
            home.join("Library/Application Support/Codescribe/settings.json"),
        ),
        ("dotenv", home.join(".codescribe/.env")),
    ]
    .into_iter()
    .map(|(label, path)| fingerprint_file(label, &path))
    .collect::<Result<Vec<_>>>()?;

    let preferences = home.join("Library/Preferences/com.vetcoders.codescribe.plist");
    let exists = preferences.is_file();
    fingerprints.push(FileFingerprint {
        label: "preferences_plist_semantic".to_string(),
        exists,
        sha256: exists
            .then(|| sha256_plist_semantics(&preferences))
            .transpose()?,
    });
    Ok(fingerprints)
}

fn fingerprint_file(label: &str, path: &Path) -> Result<FileFingerprint> {
    let exists = path.is_file();
    Ok(FileFingerprint {
        label: label.to_string(),
        exists,
        sha256: exists.then(|| sha256_file(path)).transpose()?,
    })
}

fn atomic_write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    atomic_write(path, &bytes)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| anyhow!("report path has no UTF-8 filename"))?;
    let temporary = parent.join(format!(".{file_name}.tmp-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .with_context(|| format!("create temporary report {}", temporary.display()))?;
    io::Write::write_all(&mut file, bytes)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    Ok(())
}

fn atomic_write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_write(path, bytes)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("set private report permissions {}", path.display()))
}

fn markdown_sibling(json: &Path) -> PathBuf {
    json.with_extension("md")
}

fn census_markdown(census: &CorpusCensus) -> String {
    format!(
        "# Codescribe corpus census\n\n- Audio instances: {}\n- Distinct audio: {}\n- Duplicate instances: {}\n- Formats: {}\n- Human-paired distinct: {}\n- Historical-paired distinct: {}\n- Apple-referenced distinct: {}\n- Unpaired distinct: {}\n- Selected distinct: {}\n- Reference policy: `{}`\n- Source paths or filenames emitted: no\n- Transcript bodies emitted: no\n",
        census.discovered_audio_instances,
        census.distinct_audio,
        census.duplicate_instances,
        census
            .format_instances
            .iter()
            .map(|(format, count)| format!("{format}={count}"))
            .collect::<Vec<_>>()
            .join(", "),
        census.distinct_human_paired,
        census.distinct_historical_paired,
        census.distinct_apple_referenced,
        census.distinct_unpaired,
        census.selected_distinct,
        census.reference_policy,
    )
}

fn matrix_markdown(report: &MatrixReport) -> String {
    let mut output = String::new();
    writeln!(output, "# Codescribe corpus parity report\n").unwrap();
    writeln!(output, "- Engine contract: `{}`", report.engine_contract).unwrap();
    writeln!(output, "- Commit: `{}`", report.commit).unwrap();
    writeln!(
        output,
        "- Apple STT bridge SHA-256: `{}`",
        report
            .apple_stt_bridge
            .sha256
            .as_deref()
            .unwrap_or("missing")
    )
    .unwrap();
    writeln!(
        output,
        "- Distinct recordings: {}",
        report.distinct_recordings
    )
    .unwrap();
    writeln!(
        output,
        "- Executions: {}/{} successful",
        report.successful_executions, report.requested_executions
    )
    .unwrap();
    writeln!(
        output,
        "- Operator config hashes unchanged: {}",
        report.configuration_files_unchanged
    )
    .unwrap();
    writeln!(output, "- Operator settings loaded: no").unwrap();
    writeln!(output, "- Operator dotenv loaded: no").unwrap();
    writeln!(
        output,
        "- Keychain access disabled: {}",
        report.keychain_disabled
    )
    .unwrap();
    writeln!(
        output,
        "- Permission request APIs called: {}",
        report.permission_request_apis_called_by_tool
    )
    .unwrap();
    writeln!(
        output,
        "- TCC state fully proven unchanged: {}",
        report.permission_state_proven_unchanged
    )
    .unwrap();
    writeln!(output, "- Quality gate: `{}`\n", report.quality_gate).unwrap();
    writeln!(
        output,
        "| Profile | OK | Failed | Observed L1 | Mean WER | Mean CER | Char parity | Qube quality |"
    )
    .unwrap();
    writeln!(output, "|---|---:|---:|---|---:|---:|---:|---|").unwrap();
    for status in &report.profile_status {
        writeln!(
            output,
            "| `{}` | {} | {} | {} | {} | {} | {} | {} |",
            status.profile.token(),
            status.successful_executions,
            status.failed_executions,
            optional_bool(status.observed_layered),
            optional_score(status.mean_wer),
            optional_score(status.mean_cer),
            optional_score(status.mean_character_parity),
            status
                .quality_html
                .as_deref()
                .map_or_else(|| "n/a".to_string(), |path| format!("[open]({path})")),
        )
        .unwrap();
    }
    output.push_str(
        "\n## Coverage boundary\n\nThis is production PCM-session replay through stop adjudication and lexicon delivery. It does not prove CoreAudio microphone capture, BlackHole loopback, hotkey modes, target-app paste, inline LLM formatting, or TCC continuity. Those surfaces remain explicit in `report.json`.\n",
    );
    output
}

fn optional_bool(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "yes",
        Some(false) => "no",
        None => "n/a",
    }
}

fn optional_score(value: Option<f64>) -> String {
    value.map_or_else(|| "n/a".to_string(), |score| format!("{score:.4}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_tokens_round_trip_without_hidden_defaults() {
        for profile in [
            ReplayProfile::AppleLayer0,
            ReplayProfile::AppleLayer1Inprocess,
            ReplayProfile::AppleLayer1Sidecar,
            ReplayProfile::AppleLayer1Remote,
            ReplayProfile::AppleLayer1FusionUtterance,
            ReplayProfile::AppleLayer1FusionLeftPad,
            ReplayProfile::AppleLayer1FusionStablePrompt,
            ReplayProfile::AppleLayer1FusionIdempotent,
            ReplayProfile::AppleLayer1LocalFinalPass,
        ] {
            assert_eq!(profile.token().parse::<ReplayProfile>(), Ok(profile));
        }
    }

    #[test]
    fn layering_observation_reads_provider_arming_not_tail_patch_count() {
        assert_eq!(layering_observation(true, [true, true]), (true, true));
        assert_eq!(layering_observation(false, [false, false]), (false, true));
        assert_eq!(layering_observation(true, [true, false]), (true, false));
        assert_eq!(layering_observation(false, [false, true]), (true, false));
        assert_eq!(layering_observation(true, []), (false, false));
        assert_eq!(layering_observation(false, []), (false, false));
    }

    #[test]
    fn privacy_contract_is_fail_closed() {
        let contract = PrivacyContract::default();
        assert!(!contract.source_paths_emitted);
        assert!(!contract.source_filenames_emitted);
        assert!(!contract.transcript_bodies_emitted);
        assert!(contract.opaque_ids_are_hash_prefixes);
    }

    #[test]
    fn corpus_schema_carries_the_engine_contract() {
        assert_eq!(REPORT_SCHEMA, CORPUS_REPORT_SCHEMA);
        assert_eq!(REPORT_SCHEMA, "codescribe-corpus-parity/v3");
        assert_eq!(ENGINE_CONTRACT_ID, "the-engine/v1");
    }

    #[test]
    fn edit_metrics_distinguish_loss_from_exact_text() {
        assert_eq!(word_error_rate("alpha beta", "alpha beta"), 0.0);
        assert!(word_error_rate("alpha beta", "alpha") > 0.0);
        assert_eq!(normalized_character_parity("Alpha beta", "alpha beta"), 1.0);
    }

    #[test]
    fn coverage_never_claims_file_replay_is_live_capture() {
        let coverage = CoverageContract::default();
        assert_eq!(coverage.production_pcm_session_replay, "covered");
        assert_eq!(
            coverage.coreaudio_microphone_capture,
            "not_covered_by_file_replay"
        );
        assert_eq!(coverage.tcc_permissions, "not_mutated_or_fully_verified");
    }

    #[test]
    fn isolated_data_dir_is_not_removed_with_profile_overrides() {
        assert!(!CONTROLLED_ENV.contains(&"CODESCRIBE_DATA_DIR"));
        assert!(!CONTROLLED_ENV.contains(&"CODESCRIBE_DISABLE_KEYCHAIN"));
    }

    #[test]
    fn plist_fingerprint_ignores_storage_encoding() {
        let temp = tempfile::tempdir().unwrap();
        let xml_path = temp.path().join("preferences.xml.plist");
        let binary_path = temp.path().join("preferences.binary.plist");
        fs::write(
            &xml_path,
            br#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>agentEnabled</key>
    <true/>
    <key>launchCount</key>
    <integer>7</integer>
</dict>
</plist>
"#,
        )
        .unwrap();
        fs::copy(&xml_path, &binary_path).unwrap();
        let conversion = ProcessCommand::new("/usr/bin/plutil")
            .args(["-convert", "binary1", "--"])
            .arg(&binary_path)
            .status()
            .unwrap();
        assert!(conversion.success());
        assert_ne!(
            sha256_file(&xml_path).unwrap(),
            sha256_file(&binary_path).unwrap()
        );
        assert_eq!(
            sha256_plist_semantics(&xml_path).unwrap(),
            sha256_plist_semantics(&binary_path).unwrap()
        );
    }

    #[test]
    fn production_quality_report_uses_qube_keyboard_surface() {
        let entries = vec![ReportEntry {
            id: "audio-deadbeef-run1-apple-layer0".to_string(),
            audio_path: "audio-deadbeef".to_string(),
            audio_rel_path: "audio/audio-deadbeef.wav".to_string(),
            reference_path: None,
            duration_secs: 1.0,
            transcripts: ReportTranscripts {
                raw: Some("surowy tekst".to_string()),
                post: Some("dostarczony tekst".to_string()),
                reference: Some("tekst człowieka".to_string()),
                ..ReportTranscripts::default()
            },
            raw_semantics: Some(ReportTranscriptSemantics {
                state: ReportTranscriptState::TextCommitted,
                reason: None,
            }),
            metrics: ReportMetrics {
                raw_wer: Some(0.5),
                post_wer: Some(0.25),
                ..ReportMetrics::default()
            },
            postprocess_stats: None,
            errors: Vec::new(),
        }];
        let report = build_quality_report(
            ReplayProfile::AppleLayer0,
            "pl",
            ReferencePolicy::Human,
            entries,
        );
        let config = QualityReportConfig {
            input_dir: PathBuf::from("."),
            output_dir: PathBuf::from("."),
            date_filter: None,
            limit: 0,
            language: Some("pl".to_string()),
            skip_cloud: true,
            cloud_concurrency: 0,
            skip_formatting: true,
            debug_mode: true,
            copy_audio: false,
            metrics_reference: MetricsReference::Corpus,
            local_transcription: LocalTranscriptionMode::LocalWhisper,
        };
        let html = render_qube_html(&report, &config);
        assert!(html.contains("Ctrl+Cmd+Space"));
        assert!(html.contains("event.code === 'ArrowLeft'"));
        assert!(html.contains("surowy tekst"));
        assert!(html.contains("dostarczony tekst"));
        assert!(html.contains("tekst człowieka"));
    }
}
