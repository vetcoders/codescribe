//! CodeScribe CLI - Local speech-to-text transcription
//!
//! Lightweight CLI for direct audio file transcription.
//! For the tray app + overlay, use CodeScribe.app.
//!
//! Created by M&K (c)2026 VetCoders

use anyhow::Result;
use clap::{Parser, Subcommand};
use codescribe::os::hotkeys;
use codescribe::{ai_formatting, audio, whisper};
use codescribe_core::pipeline::contracts::{EngineEvent, EventSink, TranscriptDelta};
use codescribe_core::vad;
use std::borrow::Cow;
use std::env;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tracing::info;

/// CodeScribe CLI - Local speech-to-text transcription
///
/// For the full app with tray icon and hotkeys, run CodeScribe.app
#[derive(Parser)]
#[command(name = "codescribe")]
#[command(version)]
#[command(author = "VetCoders <hello@vetcoders.io>")]
#[command(about = "Local speech-to-text transcription", long_about = None)]
struct Cli {
    /// Open config file in editor (creates default if missing)
    #[arg(long)]
    config: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Transcribe an audio file using local Whisper
    Transcribe(TranscribeArgs),

    /// Run as daemon with tray icon (default when no args)
    Daemon,

    /// Query or drive the native app automation surface over IPC
    App {
        #[command(subcommand)]
        command: AppCommand,
    },

    /// Migrate transcript/audio filenames to ASCII + suffix naming
    MigrateHistory {
        /// Only print planned changes without renaming files
        #[arg(long)]
        dry_run: bool,

        /// Assume kind for files without suffix
        #[arg(long, value_enum, default_value = "raw")]
        assume_kind: MigrateKind,
    },
}

#[derive(clap::Args)]
struct TranscribeArgs {
    /// Path to audio file (wav, mp3, m4a)
    file: Option<PathBuf>,

    /// Language code (e.g., pl, en). Default: auto-detect
    #[arg(short, long, global = true)]
    language: Option<String>,

    /// Stream transcription to stdout (chunked, with flush)
    #[arg(long)]
    stream: bool,

    /// Format output using AI (Ollama)
    #[arg(short, long)]
    format: bool,

    /// LLM model for formatting (overrides LLM_FORMATTING_MODEL for this run)
    #[arg(long)]
    llm: Option<String>,

    /// Skip Silero VAD speech pre-filtering
    #[arg(long)]
    no_vad: bool,

    #[command(subcommand)]
    mode: Option<TranscribeMode>,
}

#[derive(Subcommand)]
enum TranscribeMode {
    /// Live transcription from microphone to stdout
    Live,
}

#[derive(Subcommand)]
enum AppCommand {
    /// Print the current native app automation state as JSON
    State,
    /// Run one native app automation action and print the resulting state as JSON
    Action {
        #[arg(value_enum)]
        action: AppAutomationCliAction,
    },
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
enum AppAutomationCliAction {
    ResetUi,
    ShowCreator,
    HideCreator,
    ShowVoiceChat,
    HideVoiceChat,
    ShowTranscriptionOverlay,
    HideTranscriptionOverlay,
    TriggerTrayShowAgent,
    TriggerTrayOpenCreator,
    TriggerTrayCompleteSetup,
    TriggerTrayRunOnboarding,
    TriggerDockReopen,
}

impl From<AppAutomationCliAction> for codescribe::ipc::AppAutomationAction {
    fn from(value: AppAutomationCliAction) -> Self {
        match value {
            AppAutomationCliAction::ResetUi => Self::ResetUi,
            AppAutomationCliAction::ShowCreator => Self::ShowCreator,
            AppAutomationCliAction::HideCreator => Self::HideCreator,
            AppAutomationCliAction::ShowVoiceChat => Self::ShowVoiceChat,
            AppAutomationCliAction::HideVoiceChat => Self::HideVoiceChat,
            AppAutomationCliAction::ShowTranscriptionOverlay => Self::ShowTranscriptionOverlay,
            AppAutomationCliAction::HideTranscriptionOverlay => Self::HideTranscriptionOverlay,
            AppAutomationCliAction::TriggerTrayShowAgent => Self::TriggerTrayShowAgent,
            AppAutomationCliAction::TriggerTrayOpenCreator => Self::TriggerTrayOpenCreator,
            AppAutomationCliAction::TriggerTrayCompleteSetup => Self::TriggerTrayCompleteSetup,
            AppAutomationCliAction::TriggerTrayRunOnboarding => Self::TriggerTrayRunOnboarding,
            AppAutomationCliAction::TriggerDockReopen => Self::TriggerDockReopen,
        }
    }
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
enum MigrateKind {
    Raw,
    Cloud,
    Ai,
    AiFailed,
    Failed,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();

    let cli = Cli::parse();

    // Handle --config flag
    if cli.config {
        return handle_config_command();
    }

    // Handle subcommands
    match cli.command {
        Some(Commands::Transcribe(args)) => handle_transcribe_command(args).await,
        Some(Commands::App { command }) => handle_app_command(command),
        Some(Commands::MigrateHistory {
            dry_run,
            assume_kind,
        }) => handle_migrate_history_command(dry_run, assume_kind),
        Some(Commands::Daemon) | None => run_daemon().await,
    }
}

fn handle_app_command(command: AppCommand) -> Result<()> {
    use anyhow::bail;
    use codescribe::ipc::{IpcCommand, IpcResponse, send_command_blocking};

    let response = match command {
        AppCommand::State => send_command_blocking(&IpcCommand::GetAppAutomationState),
        AppCommand::Action { action } => send_command_blocking(&IpcCommand::RunAppAutomation {
            action: action.into(),
        }),
    }
    .map_err(anyhow::Error::msg)?;

    match response {
        IpcResponse::AppAutomationState(state) => {
            println!("{}", serde_json::to_string_pretty(&state)?);
            Ok(())
        }
        IpcResponse::Error(message) => bail!(message),
        other => bail!("Unexpected IPC response for app command: {:?}", other),
    }
}

fn init_tracing() {
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::{EnvFilter, fmt, fmt::writer::BoxMakeWriter};

    // Prefer `RUST_LOG`, fall back to legacy `LOG_LEVEL`.
    let filter = match env::var("RUST_LOG") {
        Ok(v) => v,
        Err(_) => match env::var("LOG_LEVEL") {
            Ok(v) => v.to_lowercase(),
            Err(_) => "info".to_string(),
        },
    };

    let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let log_dir = PathBuf::from(home).join(".codescribe").join("logs");
    let _ = std::fs::create_dir_all(&log_dir);
    let log_path = log_dir.join("codescribe.log");

    let stderr_layer = fmt::layer()
        .with_ansi(true)
        .with_target(true)
        .with_thread_ids(true);

    let filter_layer = EnvFilter::try_new(filter).unwrap_or_else(|_| EnvFilter::new("info"));

    let file = open_append_log_file(&log_path);

    if let Ok(file) = file {
        let file = std::sync::Arc::new(file);
        let log_path = log_path.clone();
        let writer = BoxMakeWriter::new(move || -> Box<dyn std::io::Write> {
            match clone_or_reopen_log_file(&file, &log_path, "runtime log sink") {
                Ok(writer) => Box::new(writer),
                Err(error) => {
                    eprintln!(
                        "[logging] Failed to access {}: {}. Falling back to sink.",
                        log_path.display(),
                        error
                    );
                    Box::new(std::io::sink())
                }
            }
        });
        let file_layer = fmt::layer()
            .with_ansi(false)
            .with_target(true)
            .with_thread_ids(true)
            .with_writer(writer);

        let _ = tracing_subscriber::registry()
            .with(filter_layer)
            .with(stderr_layer)
            .with(file_layer)
            .try_init();
    } else {
        let _ = tracing_subscriber::registry()
            .with(filter_layer)
            .with(stderr_layer)
            .try_init();
    }
}

fn open_append_log_file(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
}

fn clone_or_reopen_log_file(
    file: &std::fs::File,
    path: &std::path::Path,
    context: &str,
) -> std::io::Result<std::fs::File> {
    file.try_clone().or_else(|clone_error| {
        open_append_log_file(path).map_err(|open_error| {
            std::io::Error::new(
                open_error.kind(),
                format!("{context}: clone failed ({clone_error}); reopen failed ({open_error})"),
            )
        })
    })
}

/// Handle --config flag: create default config and open in editor
fn handle_config_command() -> Result<()> {
    use std::fs;
    use std::process::Command;

    let config_dir = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string()))
        .join(".codescribe");
    let config_path = config_dir.join(".env");

    // Create directory if needed
    fs::create_dir_all(&config_dir)?;

    // Create default config if missing
    if !config_path.exists() {
        let default_config = include_str!("../core/config/default_env.txt");
        fs::write(&config_path, default_config)?;
        println!("Created default config: {}", config_path.display());
    } else {
        println!("Config exists: {}", config_path.display());
    }

    // Open in editor
    #[cfg(target_os = "macos")]
    {
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() {
            println!("Opening in default text editor");
            Command::new("open").arg("-t").arg(&config_path).status()?;
            return Ok(());
        }
    }

    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| {
            for editor in &["code", "nvim", "vim", "nano"] {
                if Command::new("which")
                    .arg(editor)
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false)
                {
                    return editor.to_string();
                }
            }
            "nano".to_string()
        });

    println!("Opening in: {}", editor);
    Command::new(&editor).arg(&config_path).status()?;

    Ok(())
}

fn handle_migrate_history_command(dry_run: bool, assume_kind: MigrateKind) -> Result<()> {
    let kind = match assume_kind {
        MigrateKind::Raw => codescribe::state::history::TranscriptKind::Raw,
        MigrateKind::Cloud => codescribe::state::history::TranscriptKind::Cloud,
        MigrateKind::Ai => codescribe::state::history::TranscriptKind::Ai,
        MigrateKind::AiFailed => codescribe::state::history::TranscriptKind::AiFailed,
        MigrateKind::Failed => codescribe::state::history::TranscriptKind::Failed,
    };

    let report = codescribe::state::history::migrate_transcriptions(kind, dry_run)?;

    println!(
        "Migration summary: {} transcripts renamed, {} audio renamed, {} skipped, {} errors",
        report.renamed_text, report.renamed_audio, report.skipped, report.errors
    );

    Ok(())
}

/// Handle `codescribe transcribe <file>` command
async fn handle_transcribe_command(args: TranscribeArgs) -> Result<()> {
    match args.mode {
        Some(TranscribeMode::Live) => handle_transcribe_live(args.language).await,
        None => {
            let file = args.file.ok_or_else(|| {
                anyhow::anyhow!("Missing <FILE> (or use `codescribe transcribe live`)")
            })?;
            handle_transcribe_file(
                file,
                args.language,
                args.format,
                args.llm,
                args.stream,
                args.no_vad,
            )
            .await
        }
    }
}

async fn handle_transcribe_file(
    file: PathBuf,
    language: Option<String>,
    format: bool,
    llm_model: Option<String>,
    stream: bool,
    no_vad: bool,
) -> Result<()> {
    use std::time::Instant;

    // Check file exists
    if !file.exists() {
        anyhow::bail!("File not found: {}", file.display());
    }

    eprintln!("CodeScribe Local Transcription");
    eprintln!("Audio: {}", file.display());

    // Initialize Whisper
    eprintln!("Loading Whisper model...");
    let start = Instant::now();
    whisper::init()?;

    if whisper::embedded::is_embedded_available() {
        eprintln!("Model: embedded (zero I/O)");
    } else if let Ok(path) = whisper::get_model_path() {
        eprintln!("Model: {}", path.display());
    }
    eprintln!("Language: {}", language.as_deref().unwrap_or("auto-detect"));
    eprintln!("Model loaded in {:?}", start.elapsed());

    // Always load audio (needed for VAD pre-filter + language detection)
    let (samples, sample_rate) = audio::load_audio_file(&file)?;
    let total_sec = samples.len() as f32 / sample_rate as f32;

    // ── Silero VAD pre-filter: extract speech-only regions ──
    let speech_samples = if no_vad {
        eprintln!("VAD: skipped (--no-vad)");
        Cow::Borrowed(samples.as_slice())
    } else {
        let (filtered_samples, vad_stats) = vad::extract_speech(&samples, sample_rate);
        let speech_sec = filtered_samples.len() as f32 / sample_rate as f32;
        eprintln!(
            "Silero VAD: {:.1}s speech / {:.1}s total ({:.0}% speech) | {}",
            speech_sec, total_sec, vad_stats.speech_pct, vad_stats.sparkline
        );
        Cow::Owned(filtered_samples)
    };

    if !no_vad && speech_samples.is_empty() {
        eprintln!("No speech detected by Silero VAD. Skipping Whisper transcription.");
        return Ok(());
    }

    // Detect language if not specified
    let lang = if let Some(l) = language {
        l
    } else {
        eprintln!("Detecting language...");
        let start = Instant::now();
        let detected = whisper::detect_language(speech_samples.as_ref(), sample_rate)?;
        eprintln!("Detected: {} ({:?})", detected, start.elapsed());
        detected
    };

    if stream {
        if format || llm_model.is_some() {
            eprintln!("Warning: --stream ignores --format/--llm (raw streaming only)");
        }

        eprintln!("Transcribing (streaming)...");
        let start = Instant::now();
        let emitter = StreamEmitter::new();
        let callback = {
            let emitter = Arc::clone(&emitter);
            move |cumulative: &str| {
                emitter.emit_cumulative(cumulative);
            }
        };
        let _raw_text = whisper::transcribe_streaming(
            speech_samples.as_ref(),
            sample_rate,
            Some(&lang),
            Some(&callback),
        )?;
        eprintln!("Transcription time: {:?}", start.elapsed());
        emitter.finish();
        return Ok(());
    }

    // Transcribe (non-streaming) — speech-only samples
    eprintln!("Transcribing...");
    let start = Instant::now();
    let raw_text = whisper::transcribe(speech_samples.as_ref(), sample_rate, Some(&lang))?;
    eprintln!("Transcription time: {:?}", start.elapsed());

    // Format with AI if requested
    let final_text = if format {
        let mut prev_model: Option<String> = None;
        if let Some(model) = llm_model.as_ref() {
            prev_model = std::env::var("LLM_FORMATTING_MODEL").ok();
            // SAFETY: CLI is single-process; scoped override for this run only.
            unsafe { std::env::set_var("LLM_FORMATTING_MODEL", model) };
            eprintln!("Formatting with AI (model: {})...", model);
        } else {
            eprintln!("Formatting with AI...");
        }

        let start = Instant::now();
        let result =
            ai_formatting::format_text_with_status(&raw_text, Some(&lang), false, None).await;

        let formatted = match result.status {
            ai_formatting::AiFormatStatus::Applied => {
                eprintln!("Formatted in {:?}", start.elapsed());
                result.text
            }
            ai_formatting::AiFormatStatus::Failed => {
                eprintln!("Formatting failed - using raw text");
                raw_text
            }
            ai_formatting::AiFormatStatus::Skipped => raw_text,
        };

        if llm_model.is_some() {
            match prev_model {
                Some(prev) => unsafe { std::env::set_var("LLM_FORMATTING_MODEL", prev) },
                None => unsafe { std::env::remove_var("LLM_FORMATTING_MODEL") },
            }
        }

        formatted
    } else {
        raw_text
    };

    eprintln!();

    // Output transcription to stdout (pipeable)
    emit_stdout(&final_text)?;
    emit_stdout("\n")?;

    Ok(())
}

async fn handle_transcribe_live(language: Option<String>) -> Result<()> {
    use std::io::Write;

    // CLI flag takes priority; fall back to settings.json speech.language.
    let language =
        language.or_else(|| codescribe_core::config::UserSettings::load().whisper_language);

    eprintln!("CodeScribe Live Transcription");
    eprintln!("Press Ctrl+C to stop.");

    whisper::init()?;

    // Create transcript log file — clean text only, one utterance per line.
    let log_dir = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".codescribe/logs");
    std::fs::create_dir_all(&log_dir)?;
    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let log_path = log_dir.join(format!("live_{timestamp}.log"));
    let log_file = Arc::new(Mutex::new(
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?,
    ));
    eprintln!("Transcript log: {}", log_path.display());

    // Auto-open log in Console.app for live tailing.
    let _ = std::process::Command::new("open")
        .arg("-a")
        .arg("Console")
        .arg(&log_path)
        .spawn();

    let mut recorder = codescribe::audio::streaming_recorder::StreamingRecorder::new()?;
    // Disable auto-silence stop — live mode runs until Ctrl+C.
    // Silero VAD still acts as supervisor for utterance segmentation
    // inside the streaming pipeline (same as the daemon app).
    recorder.recorder.config.auto_silence = false;

    let emitter = StreamEmitter::new();
    let sink = LiveCliEventSink::new(Arc::clone(&emitter));
    let log_sink = LiveLogEventSink {
        inner: sink,
        log_file: log_file.clone(),
    };
    recorder.set_event_sink(Some(Arc::new(log_sink) as Arc<dyn EventSink>));
    recorder.start_event_session(language).await?;

    tokio::signal::ctrl_c().await.ok();
    eprintln!("\nStopping live transcription...");

    let _ = recorder.stop().await?;
    emitter.finish();

    // Write session footer.
    if let Ok(mut f) = log_file.lock() {
        let _ = writeln!(
            f,
            "\n--- session ended {} ---",
            chrono::Local::now().format("%H:%M:%S")
        );
    }

    eprintln!("Transcript saved: {}", log_path.display());
    Ok(())
}

async fn run_daemon() -> Result<()> {
    use anyhow::Context;
    use codescribe::config::{Config, UserSettings};
    use codescribe::controller::RecordingController;
    use codescribe::os::hotkeys::HotkeyEvent;
    use codescribe::{ipc, tray};
    use crossbeam_channel::unbounded;
    use std::sync::Arc;
    use tokio::runtime::Handle;

    eprintln!("CodeScribe daemon starting...");

    // ── Build metadata ──
    info!(
        "CodeScribe {} | build={} | profile={} | rustc={} | exe={}",
        env!("CARGO_PKG_VERSION"),
        option_env!("CODESCRIBE_BUILD_COMMIT").unwrap_or("dev"),
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        option_env!("CODESCRIBE_RUSTC_VERSION").unwrap_or("unknown"),
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "unknown".into()),
    );

    let config = Config::load();
    let user_settings = UserSettings::load();
    let automation_mode = codescribe::app_automation_mode_enabled();

    if automation_mode {
        info!(
            "App automation mode enabled: skipping Whisper preload, permissions bootstrap, and hotkey registration"
        );
    }

    #[cfg(target_os = "macos")]
    {
        codescribe::set_dock_icon();
        codescribe::apply_dock_icon_visibility(config.show_dock_icon);
        codescribe::install_basic_edit_menu();
    }

    tokio::task::spawn_blocking(|| {
        codescribe_core::attachment::AttachmentStore::cleanup_old(7);
    });

    if !automation_mode {
        codescribe::whisper::init().context("Failed to initialize Whisper")?;
    }
    let controller = Arc::new(RecordingController::new());
    #[cfg(target_os = "macos")]
    codescribe::controller::register_overlay_controller(Arc::clone(&controller));
    #[cfg(target_os = "macos")]
    if !automation_mode {
        codescribe::os::permissions::check_all_permissions();

        if codescribe::should_show_onboarding() {
            codescribe::show_onboarding_wizard();
        }
    }

    if !automation_mode {
        sync_hotkey_config(&config);
    }

    let ipc_controller = Arc::clone(&controller);
    tokio::spawn(async move {
        if let Err(e) = ipc::run_server(ipc_controller).await {
            eprintln!("IPC server error: {}", e);
        }
    });

    let menu_rx = tray::menu_event_receiver()?;
    let menu_controller = Arc::clone(&controller);
    let menu_handle = Handle::current();
    std::thread::spawn(move || {
        for event in menu_rx {
            let event_for_async = event.clone();
            let controller = Arc::clone(&menu_controller);
            let handle = menu_handle.clone();
            handle.spawn(async move {
                // Apply menu-driven config changes deterministically.
                // The tray handler writes to `~/.codescribe/.env`, but reading it back immediately
                // can race on some systems. So we use the event payload as the source of truth
                // for hotkey-related fields and only reload the rest from disk.
                let mut config = Config::load();
                match &event_for_async {
                    tray::TrayMenuEvent::SetQuickNotesEnabled(enabled) => {
                        config.quick_notes_enabled = *enabled;
                    }
                    tray::TrayMenuEvent::SetQuickNotesSaveOnly(save_only) => {
                        config.quick_notes_save_only = *save_only;
                    }
                    tray::TrayMenuEvent::InstallSileroVad => {
                        eprintln!("Installing Silero VAD model…");
                        match codescribe_core::vad::ensure_downloaded_to_user_dir().await {
                            Ok(path) => {
                                eprintln!("Silero VAD downloaded and ready: {}", path.display());
                                #[cfg(target_os = "macos")]
                                {
                                    codescribe::os::notifications::notify(
                                        "CodeScribe",
                                        "Silero VAD is ready",
                                    );
                                }
                            }
                            Err(e) => {
                                eprintln!("Silero VAD download failed: {}", e);
                                #[cfg(target_os = "macos")]
                                {
                                    codescribe::os::notifications::notify(
                                        "CodeScribe",
                                        &format!("Silero VAD download failed: {e}"),
                                    );
                                }
                            }
                        }
                    }
                    _ => {}
                }
                sync_hotkey_config(&config);
                controller.set_config(config).await;
            });

            if matches!(event, tray::TrayMenuEvent::Quit) {
                break;
            }
        }
    });

    let (tx, rx) = unbounded::<HotkeyEvent>();
    let hotkey_controller = Arc::clone(&controller);
    let hotkey_handle = Handle::current();
    std::thread::spawn(move || {
        for event in rx {
            let controller = Arc::clone(&hotkey_controller);
            let handle = hotkey_handle.clone();
            handle.spawn(async move {
                if let Err(e) = dispatch_hotkey_event(event, controller).await {
                    eprintln!("Hotkey event error: {}", e);
                }
            });
        }
    });

    let hotkey_manager = if automation_mode {
        None
    } else {
        match hotkeys::HotkeyManager::new(tx) {
            Ok(manager) => Some(manager),
            Err(e) => {
                eprintln!(
                    "Hotkeys waiting on permissions ({}). Grant Accessibility + Input Monitoring and CodeScribe will reinitialize them live.",
                    e
                );
                None
            }
        }
    };

    // VAD monitor task - auto-finish recording when silence detected
    let vad_controller = Arc::clone(&controller);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            if vad_controller.is_vad_triggered() {
                // IMPORTANT:
                // - In toggle mode, VAD-based auto-finish is the intended UX.
                // - In hold-to-talk mode, the user's key-down is the source of truth; do NOT stop early.
                let state = vad_controller.current_state().await;
                if state == codescribe::controller::State::RecToggle {
                    eprintln!("VAD triggered - auto-finishing recording");
                    vad_controller.clear_vad_triggered();
                    if let Err(e) = vad_controller.finish_recording().await {
                        eprintln!("VAD finish_recording error: {}", e);
                    }
                } else {
                    // Clear so it doesn't "carry over" into a later toggle session.
                    vad_controller.clear_vad_triggered();
                }
            }
        }
    });

    // Quality Loop daemon (self-improvement) — OFF by default.
    //
    // The daemon is useful, but if it survives app restarts it can confuse macOS
    // permissions / input monitoring workflows. Turn it on explicitly when needed.
    let quality_autostart = !automation_mode
        && user_settings
            .quality_daemon_autostart
            .unwrap_or_else(|| env_bool("CODESCRIBE_AUTOSTART_QUALITY_DAEMON", false));
    let quality_child = if quality_autostart {
        spawn_quality_daemon()
    } else {
        stop_quality_daemon_if_running();
        codescribe::quality_loop::mark_daemon_unavailable();
        None
    };

    tray::run_with_hotkeys(hotkey_manager)?;

    // Cleanup: kill quality daemon when tray exits
    if let Some(mut handle) = quality_child {
        let _ = handle.child.kill();
        let _ = std::fs::remove_file(handle.pid_path);
    }

    Ok(())
}

fn env_bool(key: &str, default: bool) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| {
            let v = v.trim().to_lowercase();
            matches!(v.as_str(), "1" | "true" | "yes" | "on")
        })
        .unwrap_or(default)
}

fn stop_quality_daemon_if_running() {
    let config_dir = codescribe::config::Config::config_dir();
    let pid_path = config_dir.join("logs").join("quality_daemon.pid");

    let pid = std::fs::read_to_string(&pid_path)
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok())
        .unwrap_or(0);
    if pid <= 0 {
        let _ = std::fs::remove_file(&pid_path);
        return;
    }

    if !is_process_alive(pid) {
        let _ = std::fs::remove_file(&pid_path);
        return;
    }

    // Best-effort safety check: only kill if it looks like codescribe-loop.
    let is_codescribe_loop = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.contains("codescribe-loop"))
        .unwrap_or(false);

    if is_codescribe_loop {
        let _ = unsafe { libc::kill(pid, libc::SIGTERM) };
        let _ = std::fs::remove_file(&pid_path);
    }
}

/// Spawn `codescribe-loop --daemon` as a background child process.
/// Returns the Child handle so we can kill it on app exit.
struct QualityDaemonHandle {
    child: std::process::Child,
    pid_path: PathBuf,
}

fn spawn_quality_daemon() -> Option<QualityDaemonHandle> {
    use std::process::{Command, Stdio};

    // Strategy: find codescribe-loop binary next to current exe, or in PATH
    let loop_bin = find_sibling_binary("codescribe-loop");

    let bin_path = match loop_bin {
        Some(path) => path,
        None => {
            // Try PATH fallback
            if which_exists("codescribe-loop") {
                PathBuf::from("codescribe-loop")
            } else {
                eprintln!("[quality-daemon] codescribe-loop not found; skipping auto-start");
                codescribe::quality_loop::mark_daemon_unavailable();
                return None;
            }
        }
    };

    let config_dir = codescribe::config::Config::config_dir();
    let log_path = config_dir.join("logs").join("quality_daemon.log");
    let pid_path = config_dir.join("logs").join("quality_daemon.pid");
    std::fs::create_dir_all(config_dir.join("logs")).ok();

    if let Ok(pid_str) = std::fs::read_to_string(&pid_path)
        && let Ok(pid) = pid_str.trim().parse::<i32>()
        && is_process_alive(pid)
    {
        eprintln!(
            "[quality-daemon] Already running (pid={}); skipping auto-start",
            pid
        );
        return None;
    } else if pid_path.exists() {
        let _ = std::fs::remove_file(&pid_path);
    }

    let log_file = match open_append_log_file(&log_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[quality-daemon] Failed to open log file: {}", e);
            codescribe::quality_loop::mark_daemon_unavailable();
            return None;
        }
    };

    let stderr_stdio = match clone_or_reopen_log_file(&log_file, &log_path, "quality daemon stderr")
    {
        Ok(file) => Stdio::from(file),
        Err(error) => {
            eprintln!(
                "[quality-daemon] Failed to prepare stderr log sink: {}. Falling back to null.",
                error
            );
            Stdio::null()
        }
    };

    match Command::new(&bin_path)
        .args(["--daemon", "--apply", "--daemon-interval", "1800"])
        .stdout(Stdio::from(log_file))
        .stderr(stderr_stdio)
        .spawn()
    {
        Ok(child) => {
            let _ = std::fs::write(&pid_path, child.id().to_string());
            eprintln!(
                "[quality-daemon] Started (pid={}, bin={}, log={})",
                child.id(),
                bin_path.display(),
                log_path.display()
            );
            Some(QualityDaemonHandle { child, pid_path })
        }
        Err(e) => {
            eprintln!("[quality-daemon] Failed to spawn: {}", e);
            codescribe::quality_loop::mark_daemon_unavailable();
            None
        }
    }
}

fn is_process_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    let res = unsafe { libc::kill(pid, 0) };
    if res == 0 {
        return true;
    }
    let err = std::io::Error::last_os_error();
    matches!(err.raw_os_error(), Some(code) if code == libc::EPERM)
}

/// Find a sibling binary (same directory as current executable)
fn find_sibling_binary(name: &str) -> Option<PathBuf> {
    let current_exe = std::env::current_exe().ok()?;
    let dir = current_exe.parent()?;
    let sibling = dir.join(name);
    if sibling.exists() {
        Some(sibling)
    } else {
        None
    }
}

/// Check if a binary exists in PATH
fn which_exists(name: &str) -> bool {
    std::process::Command::new("which")
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn emit_stdout(text: &str) -> Result<()> {
    use std::io::Write;

    let mut out = std::io::stdout();
    out.write_all(text.as_bytes())?;
    out.flush()?;
    Ok(())
}

struct StreamEmitter {
    last_len: Mutex<usize>,
    had_output: AtomicBool,
}

impl StreamEmitter {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            last_len: Mutex::new(0),
            had_output: AtomicBool::new(false),
        })
    }

    fn emit_raw(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        if emit_stdout(text).is_ok() {
            self.had_output.store(true, Ordering::SeqCst);
        }
    }

    fn emit_cumulative(&self, cumulative: &str) {
        let mut last_len = self.last_len.lock().unwrap_or_else(|e| e.into_inner());
        let total_len = cumulative.len();
        if total_len <= *last_len {
            return;
        }
        let delta = &cumulative[*last_len..];
        *last_len = total_len;
        self.emit_raw(delta);
    }

    fn finish(&self) {
        if self.had_output.load(Ordering::SeqCst) {
            let _ = emit_stdout("\n");
        }
    }
}

struct LiveCliEventSink {
    emitter: Arc<StreamEmitter>,
    last_preview: Mutex<String>,
}

impl LiveCliEventSink {
    fn new(emitter: Arc<StreamEmitter>) -> Self {
        Self {
            emitter,
            last_preview: Mutex::new(String::new()),
        }
    }

    fn emit_diff_from_last(&self, next: &str) {
        let mut last = self.last_preview.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(delta) = TranscriptDelta::from_diff(&last, next) {
            self.emitter.emit_raw(&delta.delta);
        }
        *last = next.to_string();
    }

    fn clear_preview(&self) {
        let mut last = self.last_preview.lock().unwrap_or_else(|e| e.into_inner());
        last.clear();
    }
}

impl EventSink for LiveCliEventSink {
    fn on_event(&self, event: &EngineEvent) {
        match event {
            EngineEvent::Preview { text, .. } => {
                self.emit_diff_from_last(text);
            }
            EngineEvent::Correction {
                text,
                previous_text,
                ..
            } => {
                let mut last = self.last_preview.lock().unwrap_or_else(|e| e.into_inner());
                if last.is_empty() || *last != *previous_text {
                    return;
                }
                if let Some(delta) = TranscriptDelta::from_diff(&last, text) {
                    self.emitter.emit_raw(&delta.delta);
                }
                *last = text.clone();
            }
            EngineEvent::UtteranceFinal { text, .. } => {
                self.emit_diff_from_last(text);
                self.clear_preview();
            }
            EngineEvent::NoSpeech { .. } => {
                self.clear_preview();
            }
            _ => {}
        }
    }
}

/// Wraps `LiveCliEventSink` and appends UtteranceFinal text to a log file.
struct LiveLogEventSink {
    inner: LiveCliEventSink,
    log_file: Arc<Mutex<std::fs::File>>,
}

impl EventSink for LiveLogEventSink {
    fn on_event(&self, event: &EngineEvent) {
        // Delegate to the CLI sink for stdout output.
        self.inner.on_event(event);

        // Append clean transcript text to log file on utterance boundaries.
        if let EngineEvent::UtteranceFinal { text, .. } = event {
            let trimmed = text.trim();
            if !trimmed.is_empty()
                && let Ok(mut f) = self.log_file.lock()
            {
                use std::io::Write;
                let _ = writeln!(f, "{}", trimmed);
                let _ = f.flush();
            }
        }
    }
}

fn sync_hotkey_config(config: &codescribe::config::Config) {
    codescribe::os::hotkeys::apply_hotkey_config(config);
}

async fn dispatch_hotkey_event(
    event: codescribe::os::hotkeys::HotkeyEvent,
    controller: std::sync::Arc<codescribe::controller::RecordingController>,
) -> Result<()> {
    use codescribe::controller::{HotkeyAction, HotkeyInput, HotkeyType};
    use codescribe::os::hotkeys::{HoldAction, HoldMode, HotkeyEvent};

    match event {
        HotkeyEvent::Hold {
            action,
            mode,
            force_ai,
        } => {
            let mapped_action = match action {
                HoldAction::Down => HotkeyAction::Down,
                HoldAction::Up => HotkeyAction::Up,
            };
            let input = HotkeyInput {
                key_type: HotkeyType::Hold,
                action: mapped_action,
                assistive: !matches!(mode, HoldMode::Raw),
                hold_mode: mode,
                force_raw: matches!(mode, HoldMode::Raw) && !force_ai,
                force_ai,
            };
            controller.handle_hotkey_event(input).await?;
        }
        HotkeyEvent::HoldUpdate { mode, force_ai } => {
            let input = HotkeyInput {
                key_type: HotkeyType::Hold,
                action: HotkeyAction::Press,
                assistive: !matches!(mode, HoldMode::Raw),
                hold_mode: mode,
                force_raw: matches!(mode, HoldMode::Raw) && !force_ai,
                force_ai,
            };
            controller.handle_hotkey_event(input).await?;
        }
        HotkeyEvent::ToggleNormal => {
            let input = HotkeyInput {
                key_type: HotkeyType::Toggle,
                action: HotkeyAction::Press,
                assistive: false,
                hold_mode: HoldMode::Raw,
                force_raw: false,
                force_ai: true,
            };
            controller.handle_hotkey_event(input).await?;
        }
        HotkeyEvent::ToggleRaw => {
            let input = HotkeyInput {
                key_type: HotkeyType::Toggle,
                action: HotkeyAction::Press,
                assistive: false,
                hold_mode: HoldMode::Raw,
                force_raw: true,
                force_ai: false,
            };
            controller.handle_hotkey_event(input).await?;
        }
        HotkeyEvent::ToggleAssistive => {
            let input = HotkeyInput {
                key_type: HotkeyType::Toggle,
                action: HotkeyAction::Press,
                assistive: true,
                hold_mode: HoldMode::Raw,
                force_raw: false,
                force_ai: false,
            };
            controller.handle_hotkey_event(input).await?;
        }
    }

    Ok(())
}
