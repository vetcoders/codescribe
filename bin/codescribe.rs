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
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tracing::{debug, info, warn};
use tracing_subscriber::EnvFilter;

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

    #[command(subcommand)]
    mode: Option<TranscribeMode>,
}

#[derive(Subcommand)]
enum TranscribeMode {
    /// Live transcription from microphone to stdout
    Live,
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
        Some(Commands::MigrateHistory {
            dry_run,
            assume_kind,
        }) => handle_migrate_history_command(dry_run, assume_kind),
        Some(Commands::Daemon) | None => run_daemon().await,
    }
}

static LOG_GUARD: std::sync::OnceLock<tracing_appender::non_blocking::WorkerGuard> =
    std::sync::OnceLock::new();

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let log_dir = codescribe::config::Config::config_dir().join("logs");
    let _ = std::fs::create_dir_all(&log_dir);
    let file_appender = tracing_appender::rolling::never(&log_dir, "codescribe.log");
    let (file_writer, guard) = tracing_appender::non_blocking(file_appender);
    let _ = LOG_GUARD.set(guard);

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(file_writer)
        .with_target(false)
        .try_init();
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
            handle_transcribe_file(file, args.language, args.format, args.llm, args.stream).await
        }
    }
}

async fn handle_transcribe_file(
    file: PathBuf,
    language: Option<String>,
    format: bool,
    llm_model: Option<String>,
    stream: bool,
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

    // Load audio only if needed (language detection or streaming)
    let audio_data = if stream || language.is_none() {
        Some(audio::load_audio_file(&file)?)
    } else {
        None
    };

    // Detect language if not specified
    let lang = if let Some(l) = language {
        l
    } else {
        let (samples, sample_rate) = audio_data
            .as_ref()
            .expect("audio data required for language detection");
        eprintln!("Detecting language...");
        let start = Instant::now();
        let detected = whisper::detect_language(samples, *sample_rate)?;
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
        let (samples, sample_rate) = audio_data
            .as_ref()
            .expect("audio data required for streaming");
        let _raw_text =
            whisper::transcribe_streaming(samples, *sample_rate, Some(&lang), Some(&callback))?;
        eprintln!("Transcription time: {:?}", start.elapsed());
        emitter.finish();
        return Ok(());
    }

    // Transcribe (non-streaming)
    eprintln!("Transcribing...");
    let start = Instant::now();
    let raw_text = whisper::transcribe_file(&file, Some(&lang))?;
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
    eprintln!("CodeScribe Live Transcription");
    eprintln!("Press Ctrl+C to stop.");

    whisper::init()?;

    let config = codescribe::audio::recorder::RecorderConfig::default();
    let mut recorder =
        codescribe::audio::streaming_recorder::StreamingRecorder::with_config(config)?;

    let emitter = StreamEmitter::new();
    recorder.set_delta_callback(Some(Arc::new({
        let emitter = Arc::clone(&emitter);
        move |delta: &str| {
            emitter.emit_delta(delta);
        }
    })));

    recorder.start(language).await?;

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            eprintln!("Stopping live transcription (Ctrl+C)...");
        }
    }

    let _ = recorder.stop_without_saving().await?;
    emitter.finish();

    Ok(())
}

async fn run_daemon() -> Result<()> {
    use anyhow::Context;
    use codescribe::config::Config;
    use codescribe::controller::RecordingController;
    use codescribe::os::hotkeys::HotkeyEvent;
    use codescribe::{ipc, tray};
    use crossbeam_channel::unbounded;
    use std::sync::Arc;
    use tokio::runtime::Handle;

    eprintln!("CodeScribe daemon starting...");

    #[cfg(target_os = "macos")]
    codescribe::set_dock_icon();

    codescribe::whisper::init().context("Failed to initialize Whisper")?;
    let controller = Arc::new(RecordingController::new());
    #[cfg(target_os = "macos")]
    codescribe::controller::register_overlay_controller(Arc::clone(&controller));
    #[cfg(target_os = "macos")]
    {
        if codescribe::should_show_bootstrap() {
            codescribe::schedule_bootstrap();
        }
    }

    let config = Config::load();
    sync_hotkey_config(&config);

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
        use tray::TrayMenuEvent;
        for event in menu_rx {
            // Apply hotkey settings directly from event (avoids race condition with .env save)
            match &event {
                TrayMenuEvent::SetHoldMods(mods) => {
                    hotkeys::set_hold_mods(*mods);
                }
                TrayMenuEvent::SetToggleTrigger(trigger) => {
                    hotkeys::set_toggle_trigger(*trigger);
                }
                TrayMenuEvent::ToggleHoldExclusive => {
                    let config = Config::load();
                    hotkeys::set_exclusive_mode(!config.hold_exclusive);
                }
                _ => {}
            }

            // Update controller with fresh config for non-hotkey settings
            let controller = Arc::clone(&menu_controller);
            let handle = menu_handle.clone();
            handle.spawn(async move {
                let config = Config::load();
                controller.set_config(config).await;
            });

            if matches!(event, TrayMenuEvent::Quit) {
                break;
            }
        }
    });

    let (tx, rx) = unbounded::<HotkeyEvent>();
    let hotkey_manager = hotkeys::HotkeyManager::new(tx).map_err(|e| anyhow::anyhow!(e))?;

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

    // Start Quality Loop daemon (always-on self-improvement)
    let quality_child = spawn_quality_daemon();

    tray::run_with_hotkeys(Some(hotkey_manager))?;

    // Cleanup: kill quality daemon when tray exits
    if let Some(mut handle) = quality_child {
        let _ = handle.child.kill();
        let _ = std::fs::remove_file(handle.pid_path);
    }

    Ok(())
}

/// Spawn `codescribe-loop --daemon` as a background child process.
/// Returns the Child handle so we can kill it on app exit.
struct QualityDaemonHandle {
    child: std::process::Child,
    pid_path: PathBuf,
}

fn spawn_quality_daemon() -> Option<QualityDaemonHandle> {
    use std::process::{Command, Stdio};

    if matches!(
        std::env::var("CODESCRIBE_QUALITY_DAEMON").as_deref(),
        Ok("0") | Ok("false") | Ok("off")
    ) {
        info!("Quality daemon disabled via CODESCRIBE_QUALITY_DAEMON=0");
        codescribe::quality_loop::mark_daemon_unavailable();
        return None;
    }

    // Strategy: find codescribe-loop binary next to current exe, or in PATH
    let loop_bin = find_sibling_binary("codescribe-loop");

    let bin_path = match loop_bin {
        Some(path) => path,
        None => {
            // Try PATH fallback
            if which_exists("codescribe-loop") {
                PathBuf::from("codescribe-loop")
            } else {
                debug!("[quality-daemon] codescribe-loop not found; skipping auto-start");
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
        debug!(
            "[quality-daemon] Already running (pid={}); skipping auto-start",
            pid
        );
        return None;
    } else if pid_path.exists() {
        let _ = std::fs::remove_file(&pid_path);
    }

    let log_file = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        Ok(f) => f,
        Err(e) => {
            warn!("[quality-daemon] Failed to open log file: {}", e);
            codescribe::quality_loop::mark_daemon_unavailable();
            return None;
        }
    };

    let stderr_file = log_file.try_clone().unwrap_or_else(|_| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .expect("log file")
    });

    match Command::new(&bin_path)
        .args(["--daemon", "--apply", "--daemon-interval", "1800"])
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(stderr_file))
        .spawn()
    {
        Ok(child) => {
            let _ = std::fs::write(&pid_path, child.id().to_string());
            info!(
                "[quality-daemon] Started (pid={}, bin={}, log={})",
                child.id(),
                bin_path.display(),
                log_path.display()
            );
            Some(QualityDaemonHandle { child, pid_path })
        }
        Err(e) => {
            warn!("[quality-daemon] Failed to spawn: {}", e);
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
    buffer: Mutex<String>,
}

impl StreamEmitter {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            last_len: Mutex::new(0),
            had_output: AtomicBool::new(false),
            buffer: Mutex::new(String::new()),
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

    fn emit_delta(&self, delta: &str) {
        if delta.is_empty() {
            return;
        }

        let snapshot = {
            let mut buffer = self.buffer.lock().unwrap_or_else(|e| e.into_inner());
            apply_delta_to_string(&mut buffer, delta);
            buffer.clone()
        };
        self.emit_cumulative(&snapshot);
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

fn apply_delta_to_string(target: &mut String, delta: &str) {
    for ch in delta.chars() {
        if ch == '\u{0008}' {
            target.pop();
        } else {
            target.push(ch);
        }
    }
}

fn sync_hotkey_config(config: &codescribe::config::Config) {
    codescribe::os::hotkeys::set_hold_mods(config.hold_mods);
    codescribe::os::hotkeys::set_toggle_trigger(config.toggle_trigger);
    codescribe::os::hotkeys::set_exclusive_mode(config.hold_exclusive);
}

async fn dispatch_hotkey_event(
    event: codescribe::os::hotkeys::HotkeyEvent,
    controller: std::sync::Arc<codescribe::controller::RecordingController>,
) -> Result<()> {
    use codescribe::controller::{HotkeyAction, HotkeyInput, HotkeyType};
    use codescribe::os::hotkeys::{HoldAction, HotkeyEvent};

    match event {
        HotkeyEvent::Hold { action, assistive } => {
            let mapped_action = match action {
                HoldAction::Down => HotkeyAction::Down,
                HoldAction::Up => HotkeyAction::Up,
            };
            let input = HotkeyInput {
                key_type: HotkeyType::Hold,
                action: mapped_action,
                assistive,
                force_ai: false,
            };
            controller.handle_hotkey_event(input).await?;
        }
        HotkeyEvent::ToggleNormal => {
            let input = HotkeyInput {
                key_type: HotkeyType::Toggle,
                action: HotkeyAction::Press,
                assistive: false,
                force_ai: true,
            };
            controller.handle_hotkey_event(input).await?;
        }
        HotkeyEvent::ToggleAssistive => {
            let input = HotkeyInput {
                key_type: HotkeyType::Toggle,
                action: HotkeyAction::Press,
                assistive: true,
                force_ai: false,
            };
            controller.handle_hotkey_event(input).await?;
        }
        HotkeyEvent::Conversation { action } => {
            let mapped_action = match action {
                HoldAction::Down => HotkeyAction::Down,
                HoldAction::Up => HotkeyAction::Up,
            };
            let input = HotkeyInput {
                key_type: HotkeyType::Conversation,
                action: mapped_action,
                assistive: false,
                force_ai: false,
            };
            controller.handle_hotkey_event(input).await?;
        }
    }

    Ok(())
}
