//! CodeScribe - Speech-to-text tray app for macOS
//!
//! Rust frontend that communicates with Python backend (FastAPI + MLX Whisper)

mod ai_formatting;
mod audio;
mod audio_loader;
mod backend;
mod client;
mod clipboard;
mod config;
mod controller;
mod dialog;
mod history;
mod hotkeys;
mod lab_server;
mod launchd;
mod local_stt;
mod models;
mod permissions;
mod sound;
mod tray;
mod voice_chat;
mod voice_chat_ui;
mod whisper_model;

use anyhow::Result;
use clap::Parser;
use crossbeam_channel::unbounded;
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{Level, debug, error, info, warn};
use tracing_subscriber::FmtSubscriber;

/// PID lock file path
fn pid_file_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".CodeScribe")
        .join("codescribe.pid")
}

/// Check if a PID belongs to a CodeScribe process
fn is_codescribe_process(pid: u32) -> bool {
    // Get process command name
    let output = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output();

    if let Ok(output) = output {
        if output.status.success() {
            let comm = String::from_utf8_lossy(&output.stdout);
            let comm = comm.trim().to_lowercase();
            // Check if it's our process (codescribe binary or Python backend)
            return comm.contains("codescribe")
                || comm.contains("codescribeserver")
                || comm.contains("uvicorn");
        }
    }
    false
}

/// Check if another instance is running and acquire lock
fn acquire_pid_lock() -> Result<(), String> {
    let pid_path = pid_file_path();

    // Ensure directory exists
    if let Some(parent) = pid_path.parent() {
        fs::create_dir_all(parent).ok();
    }

    // Check existing PID file
    if pid_path.exists() {
        let mut file = fs::File::open(&pid_path).map_err(|e| e.to_string())?;
        let mut contents = String::new();
        file.read_to_string(&mut contents).ok();

        if let Ok(existing_pid) = contents.trim().parse::<u32>() {
            // Check if process is still running (macOS/Unix)
            let status = std::process::Command::new("kill")
                .args(["-0", &existing_pid.to_string()])
                .status();

            if status.map(|s| s.success()).unwrap_or(false) {
                // Process exists - but is it actually CodeScribe?
                if is_codescribe_process(existing_pid) {
                    return Err(format!(
                        "CodeScribe is already running (PID {}). Use 'make stop' to stop it.",
                        existing_pid
                    ));
                } else {
                    // PID was reused by another process - stale lock, remove it
                    warn!(
                        "Stale PID file found (PID {} is now a different process), removing",
                        existing_pid
                    );
                    fs::remove_file(&pid_path).ok();
                }
            } else {
                // Process doesn't exist - stale lock, remove it
                debug!(
                    "Stale PID file found (process {} not running), removing",
                    existing_pid
                );
                fs::remove_file(&pid_path).ok();
            }
        }
    }

    // Write our PID
    let our_pid = std::process::id();
    let mut file = fs::File::create(&pid_path).map_err(|e| e.to_string())?;
    write!(file, "{}", our_pid).map_err(|e| e.to_string())?;

    Ok(())
}

/// Release PID lock on exit
fn release_pid_lock() {
    let pid_path = pid_file_path();
    fs::remove_file(pid_path).ok();
}

/// Handle --config flag: create default config and open in editor
fn handle_config_command() -> Result<()> {
    use std::path::PathBuf;
    use std::process::Command;

    let config_dir = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string()))
        .join(".codescribe");
    let config_path = config_dir.join(".env");

    // Create directory if needed
    fs::create_dir_all(&config_dir)?;

    // Create default config if missing
    if !config_path.exists() {
        let default_config = r#"# CodeScribe Configuration
# Created by: codescribe --config

# === STT (Speech-to-Text) ===
STT_ENDPOINT=https://api.libraxis.cloud/v1/audio/transcriptions
STT_API_KEY=your-api-key-here # get it from https://api.libraxis.cloud/access
WHISPER_MODEL=mlx-community/whisper-large-v3-mlx
WHISPER_LANGUAGE=en

# === LLM (AI Formatting/Assistive) ===
LLM_ENDPOINT=https://api.libraxis.cloud/v1/responses
LLM_API_KEY=your-api-key-here # get it from https://api.libraxis.cloud/access
LLM_MODEL=gpt-oss-120b-mlx
AI_FORMATTING_ENABLED=1

# === TTS (Text-to-Speech) - future ===
# TTS_ENDPOINT=https://api.libraxis.cloud/v1/synthesize
# TTS_VOICE=MarekNeural

# === Hotkeys ===
HOLD_MODS=ctrl
TOGGLE_TRIGGER=double_option

# === Audio ===
SOUND_VOLUME=0.25

# === Logging ===
LOG_LEVEL=INFO
"#;
        fs::write(&config_path, default_config)?;
        println!("✅ Created default config: {}", config_path.display());
    } else {
        println!("📄 Config exists: {}", config_path.display());
    }

    // Open in editor
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| {
            // Try common editors
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

    println!("📝 Opening in: {}", editor);
    Command::new(&editor).arg(&config_path).status()?;

    Ok(())
}

/// Handle `codescribe transcribe <file>` command
async fn handle_transcribe_command(
    file: PathBuf,
    language: Option<String>,
    model: Option<String>,
    format: bool,
    llm_model: String,
) -> Result<()> {
    use std::time::Instant;

    // Check file exists
    if !file.exists() {
        anyhow::bail!("File not found: {}", file.display());
    }

    // Find model directory
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let model_name = model.as_deref().unwrap_or("whisper-large-v3-turbo-mlx-q8");

    let model_candidates = [
        PathBuf::from(&home)
            .join(".CodeScribe/models")
            .join(model_name),
        PathBuf::from("models").join(model_name),
    ];

    let model_path = model_candidates
        .iter()
        .find(|p| p.join("tokenizer.json").exists())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Model '{}' not found or incomplete. Required files: config.json, weights.safetensors, tokenizer.json, mel_filters.npz",
                model_name
            )
        })?;

    eprintln!("═══════════════════════════════════════════════════════════");
    eprintln!("  CodeScribe Local Transcription");
    eprintln!("═══════════════════════════════════════════════════════════");
    eprintln!("  Audio: {}", file.display());
    eprintln!("  Model: {}", model_path.display());
    eprintln!(
        "  Language: {}",
        language.as_deref().unwrap_or("auto-detect")
    );
    if format {
        eprintln!("  Format: {} (via Ollama)", llm_model);
    }
    eprintln!("───────────────────────────────────────────────────────────");

    // Load model
    eprintln!("  Loading Whisper model...");
    let start = Instant::now();
    let mut engine = local_stt::LocalWhisperEngine::new(model_path)?;
    eprintln!("  Model loaded in {:?}", start.elapsed());

    // Detect language if not specified
    let lang = if let Some(l) = language {
        l
    } else {
        eprintln!("  Detecting language...");
        let start = Instant::now();
        let detected = engine.detect_language_file(&file)?;
        eprintln!("  Detected: {} ({:?})", detected, start.elapsed());
        detected
    };

    // Transcribe
    eprintln!("  Transcribing...");
    let start = Instant::now();
    let raw_text = engine.transcribe_file_with_language(&file, Some(&lang))?;
    let transcribe_time = start.elapsed();

    eprintln!("───────────────────────────────────────────────────────────");
    eprintln!("  Transcription time: {:?}", transcribe_time);
    eprintln!("  Raw characters: {}", raw_text.len());

    // Format with AI if requested
    let final_text = if format {
        eprintln!("  Formatting with AI...");
        let start = Instant::now();
        match format_with_ollama(&raw_text, &llm_model, &lang).await {
            Ok(formatted) => {
                eprintln!("  Formatted in {:?}", start.elapsed());
                eprintln!("  Formatted characters: {}", formatted.len());
                formatted
            }
            Err(e) => {
                eprintln!("  ⚠ Formatting failed: {} - using raw text", e);
                raw_text
            }
        }
    } else {
        raw_text
    };

    eprintln!("  Words: {}", final_text.split_whitespace().count());
    eprintln!("═══════════════════════════════════════════════════════════");
    eprintln!();

    // Output transcription to stdout (pipeable)
    println!("{}", final_text);

    Ok(())
}

/// Format transcription using Ollama LLM
async fn format_with_ollama(text: &str, model: &str, lang: &str) -> Result<String> {
    let host = std::env::var("LLM_HOST")
        .or_else(|_| std::env::var("OLLAMA_HOST"))
        .unwrap_or_else(|_| "http://127.0.0.1:11434".to_string());

    let endpoint = format!("{}/api/chat", host.trim_end_matches('/'));

    let system_prompt = format!(
        r#"You are a transcription formatter. Clean up and format the following speech-to-text transcription.

Rules:
- Fix punctuation, capitalization, and obvious speech recognition errors
- Remove filler words (um, uh, like) and repetitions
- Structure into clear paragraphs where appropriate
- Keep the original meaning and language ({})
- Use bullet points or numbered lists if the content is enumerating items
- Do NOT add any commentary, just output the formatted text
- Do NOT translate - keep the original language"#,
        lang
    );

    #[derive(serde::Serialize)]
    struct OllamaRequest {
        model: String,
        messages: Vec<OllamaMessage>,
        stream: bool,
        options: OllamaOptions,
    }

    #[derive(serde::Serialize)]
    struct OllamaMessage {
        role: &'static str,
        content: String,
    }

    #[derive(serde::Serialize)]
    struct OllamaOptions {
        temperature: f32,
        num_predict: u32,
    }

    #[derive(serde::Deserialize)]
    struct OllamaResponse {
        message: Option<OllamaMessageResponse>,
    }

    #[derive(serde::Deserialize)]
    struct OllamaMessageResponse {
        content: String,
    }

    let request = OllamaRequest {
        model: model.to_string(),
        messages: vec![
            OllamaMessage {
                role: "system",
                content: system_prompt,
            },
            OllamaMessage {
                role: "user",
                content: text.to_string(),
            },
        ],
        stream: false,
        options: OllamaOptions {
            temperature: 0.1,
            num_predict: 4096,
        },
    };

    let client = reqwest::Client::new();
    let response = client
        .post(&endpoint)
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Ollama HTTP {} - {}", status, body);
    }

    let ollama_response: OllamaResponse = response.json().await?;

    ollama_response
        .message
        .map(|m| m.content.trim().to_string())
        .ok_or_else(|| anyhow::anyhow!("Empty Ollama response"))
}

/// CodeScribe - Speech-to-text tray app for macOS
///
/// Hold Ctrl to record, release to transcribe.
/// Double-tap Option to toggle recording.
/// Requires Python backend running (MLX Whisper).
#[derive(Parser)]
#[command(name = "codescribe")]
#[command(version)]
#[command(author = "VetCoders <hello@vetcoders.io>")]
#[command(about = "Speech-to-text tray app for macOS", long_about = None)]
struct Cli {
    /// Enable verbose/debug logging
    #[arg(short, long)]
    verbose: bool,

    /// Open config file in editor (creates default if missing)
    #[arg(long)]
    config: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Transcribe an audio file using local Whisper (bypasses API limits)
    Transcribe {
        /// Path to audio file (wav, mp3, m4a)
        file: PathBuf,

        /// Language code (e.g., pl, en). Default: auto-detect
        #[arg(short, long)]
        language: Option<String>,

        /// Whisper model to use (default: whisper-large-v3-turbo-mlx-q8)
        #[arg(short, long)]
        model: Option<String>,

        /// Format output using AI (Ollama with qwen3-coder:480b-cloud)
        #[arg(short, long)]
        format: bool,

        /// LLM model for formatting (default: qwen3-coder:480b-cloud)
        #[arg(long, default_value = "qwen3-coder:480b-cloud")]
        llm: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Handle --config flag: create/open config file and exit
    if cli.config {
        return handle_config_command();
    }

    // Handle subcommands
    if let Some(command) = cli.command {
        return match command {
            Commands::Transcribe {
                file,
                language,
                model,
                format,
                llm,
            } => handle_transcribe_command(file, language, model, format, llm).await,
        };
    }

    // Acquire PID lock (prevent multiple instances)
    if let Err(msg) = acquire_pid_lock() {
        eprintln!("❌ {}", msg);
        std::process::exit(1);
    }

    // Initialize logging
    let log_level = if cli.verbose {
        Level::DEBUG
    } else {
        Level::INFO
    };

    // Check if we should log to file (when launched from Finder in .app bundle)
    let is_tty = std::io::IsTerminal::is_terminal(&std::io::stdout());
    let exe_path = std::env::current_exe().unwrap_or_default();
    let is_bundled = exe_path
        .to_str()
        .map(|s| s.contains("/CodeScribe.app/"))
        .unwrap_or(false);

    // When launched from .app bundle without TTY, set up file logging
    // Otherwise use normal console logging
    if !is_tty && is_bundled {
        use std::fs::OpenOptions;
        use std::io::Write;
        use std::path::PathBuf;

        let log_dir = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string()))
            .join("Library/Logs");
        std::fs::create_dir_all(&log_dir).ok();
        let log_path = log_dir.join("CodeScribe.log");

        // Note: We use eprintln for logging setup because logger isn't ready yet
        // The actual logs will go to the file after init()
        eprintln!("[CodeScribe] Logging to: {}", log_path.display());

        // Create/append to log file and write startup marker
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&log_path) {
            let _ = writeln!(
                file,
                "\n[{}] CodeScribe starting (from bundle)",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
            );
        }

        // Note: File logging with tracing requires the tracing-appender crate
        // For now, we just use stderr which will be captured by macOS logging
        // Users can view logs with: log stream --predicate 'process == "codescribe"'
        FmtSubscriber::builder()
            .with_max_level(log_level)
            .with_target(false)
            .with_ansi(false)
            .compact()
            .init();
    } else {
        // Normal console logging (development mode)
        FmtSubscriber::builder()
            .with_max_level(log_level)
            .with_target(false)
            .with_ansi(is_tty)
            .compact()
            .init();
    }

    info!("CodeScribe starting...");

    // Set Dock icon for unbundled binary (bundled .app uses Info.plist)
    codescribe::set_dock_icon();

    // Load environment variables from ~/.codescribe/.env
    let env_path = std::env::var("HOME")
        .map(|h| PathBuf::from(h).join(".codescribe").join(".env"))
        .ok()
        .filter(|p| p.exists());

    if let Some(path) = env_path {
        match dotenvy::from_path(&path) {
            Ok(()) => info!("Loaded config from: {}", path.display()),
            Err(e) => warn!("Failed to load {}: {}", path.display(), e),
        }
    } else {
        // Fallback to cwd .env
        match dotenvy::dotenv() {
            Ok(path) => debug!("Loaded environment from: {}", path.display()),
            Err(_) => debug!("No .env file found, using system environment"),
        }
    }

    // Check and request macOS permissions (Accessibility, Microphone)
    permissions::request_all_permissions();

    // Check if using cloud STT (STT_ENDPOINT set) or local Python backend
    let using_cloud = std::env::var("STT_ENDPOINT").is_ok();

    let _backend = if using_cloud {
        info!("Using cloud STT endpoint - skipping local Python backend");
        None
    } else {
        // Start Python backend server (local mode)
        // Must use spawn_blocking because BackendServer::start() uses reqwest::blocking
        info!("Starting Python backend server...");
        match tokio::task::spawn_blocking(backend::BackendServer::start).await {
            Ok(Ok(server)) => {
                info!("Python backend started on port {}", server.port());
                Some(server)
            }
            Ok(Err(e)) => {
                error!("Failed to start Python backend: {}", e);
                error!("Transcription will not work without the backend.");
                error!("Set STT_ENDPOINT for cloud mode or ensure 'uv' is installed.");
                None
            }
            Err(e) => {
                error!("Backend startup task panicked: {}", e);
                None
            }
        }
    };

    // Local backend initialization (skip for cloud mode)
    if !using_cloud {
        // Longer delay to let backend fully initialize (MLX models take time to load)
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        // Verify backend is healthy
        match client::check_health().await {
            Ok(true) => info!("Python backend health check passed"),
            Ok(false) => warn!("Python backend health check failed"),
            Err(e) => warn!("Could not verify backend health: {}", e),
        }

        // Sync WHISPER_VARIANT with backend's actual model
        if let Ok(model_info) = client::get_current_model().await {
            // SAFETY: Startup phase, no concurrent access to env vars
            unsafe { std::env::set_var("WHISPER_VARIANT", &model_info) };
            debug!("Synced WHISPER_VARIANT with backend: {}", model_info);
        }
    }

    // Create channel for hotkey events
    let (tx, rx) = unbounded::<hotkeys::HotkeyEvent>();

    // Get menu event receiver before starting tray
    let menu_rx = tray::menu_event_receiver().expect("Failed to initialize menu event channel");

    // Create HotkeyManager on main thread (required for macOS)
    // This uses global-hotkey which properly handles macOS threading
    info!("Initializing global hotkeys...");
    let hotkey_manager = match hotkeys::HotkeyManager::new(tx) {
        Ok(manager) => {
            info!("Global hotkeys registered successfully");
            Some(manager)
        }
        Err(e) => {
            error!("Failed to initialize hotkeys: {}", e);
            error!("Continuing in tray-only mode (hotkeys disabled)");
            None
        }
    };

    // Create shared config state for menu event handling
    let shared_config = Arc::new(RwLock::new(config::Config::load()));

    // Initialize hotkey settings from config
    {
        let cfg = shared_config.read().await;
        info!("Initializing hotkey configuration from saved settings...");
        hotkeys::set_exclusive_mode(cfg.hold_exclusive);
        hotkeys::set_hold_mods(cfg.hold_mods);
        hotkeys::set_toggle_trigger(cfg.toggle_trigger);
        debug!(
            "Hotkey config: hold_mods={:?}, exclusive={}, toggle={:?}",
            cfg.hold_mods, cfg.hold_exclusive, cfg.toggle_trigger
        );
    }

    // Create controller with shared config
    let controller = Arc::new(controller::RecordingController::with_config(Arc::clone(
        &shared_config,
    )));

    // Spawn async task to handle menu events
    let config_clone = Arc::clone(&shared_config);
    let controller_for_menu = Arc::clone(&controller);
    tokio::spawn(async move {
        info!("Menu event loop started");
        loop {
            match menu_rx.recv() {
                Ok(event) => {
                    info!("Received menu event: {:?}", event);
                    match event {
                        tray::TrayMenuEvent::Quit => {
                            info!("Quit event received - showing confirmation dialog...");
                            // Call dialog from blocking task - osascript handles threading
                            let choice = tokio::task::spawn_blocking(dialog::show_quit_dialog)
                                .await
                                .unwrap_or_else(|e| {
                                    error!("Dialog task failed: {:?}, defaulting to CloseAll", e);
                                    dialog::QuitChoice::CloseAll
                                });

                            info!("Dialog returned: {:?}", choice);

                            match choice {
                                dialog::QuitChoice::CloseAll => {
                                    info!("User chose Close All - stopping backend and exiting...");
                                    // Check if recording is in progress
                                    if controller_for_menu.is_recording().await {
                                        warn!("Recording in progress - forcing reset");
                                        controller_for_menu.reset().await;
                                    } else if controller_for_menu.is_busy().await {
                                        warn!("Processing in progress - forcing reset");
                                        controller_for_menu.reset().await;
                                    }
                                    // Stop any running backend servers (all known ports)
                                    info!("Killing backend processes on known ports...");
                                    backend::BackendServer::kill_existing_on_known_ports();
                                    info!("Backend kill complete");
                                    // Release PID lock before exit
                                    release_pid_lock();
                                    info!("Exiting application (backend stopped)");
                                    // Flush logs before exit
                                    std::io::Write::flush(&mut std::io::stderr()).ok();
                                    std::process::exit(0);
                                }
                                dialog::QuitChoice::LeaveServerRunning => {
                                    info!("User chose Leave Server Running - exiting tray only...");
                                    // Check if recording is in progress
                                    if controller_for_menu.is_recording().await {
                                        warn!("Recording in progress - forcing reset");
                                        controller_for_menu.reset().await;
                                    } else if controller_for_menu.is_busy().await {
                                        warn!("Processing in progress - forcing reset");
                                        controller_for_menu.reset().await;
                                    }
                                    // Release PID lock before exit (but don't stop backend)
                                    release_pid_lock();
                                    info!("Exiting application (backend still running)");
                                    std::process::exit(0);
                                }
                                dialog::QuitChoice::Cancel => {
                                    info!("User cancelled quit");
                                    // Do nothing, continue running
                                }
                            }
                        }
                        tray::TrayMenuEvent::QuitCloseAll => {
                            info!("QuitCloseAll event - stopping backend and exiting...");
                            if controller_for_menu.is_recording().await
                                || controller_for_menu.is_busy().await
                            {
                                controller_for_menu.reset().await;
                            }
                            backend::BackendServer::kill_existing_on_known_ports();
                            release_pid_lock();
                            std::process::exit(0);
                        }
                        tray::TrayMenuEvent::QuitLeaveServer => {
                            info!("QuitLeaveServer event - exiting tray only...");
                            if controller_for_menu.is_recording().await
                                || controller_for_menu.is_busy().await
                            {
                                controller_for_menu.reset().await;
                            }
                            release_pid_lock();
                            std::process::exit(0);
                        }
                        tray::TrayMenuEvent::ToggleHotkeys => {
                            if hotkeys::are_hotkeys_enabled() {
                                info!("Disabling hotkeys");
                                hotkeys::disable_hotkeys();
                            } else {
                                info!("Enabling hotkeys");
                                hotkeys::enable_hotkeys();
                            }
                        }
                        tray::TrayMenuEvent::SetLanguage(lang) => {
                            let new_lang = match lang {
                                tray::Language::Auto => config::Language::Auto,
                                tray::Language::Polish => config::Language::Polish,
                                tray::Language::English => config::Language::English,
                            };
                            info!("Setting language to: {:?}", new_lang);
                            let mut cfg = config_clone.write().await;
                            cfg.whisper_language = new_lang;
                            if let Err(e) = cfg.save_to_env("WHISPER_LANGUAGE", new_lang.as_str()) {
                                error!("Failed to save language setting: {}", e);
                            }
                            // TODO: Refresh tray menu to show updated language selection
                            // tray::update_language_selection(new_lang.as_str());
                        }
                        tray::TrayMenuEvent::CopyLatestToClipboard => {
                            info!("Copy latest to clipboard requested");
                            if let Some(entry) = history::latest_entry() {
                                match std::fs::read_to_string(&entry.path) {
                                    Ok(text) => {
                                        if let Err(e) = clipboard::copy(&text) {
                                            error!("Failed to copy to clipboard: {}", e);
                                        } else {
                                            info!(
                                                "Copied latest transcript to clipboard ({} chars)",
                                                text.len()
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        error!("Failed to read history entry: {}", e);
                                    }
                                }
                            } else {
                                warn!("No history entries found");
                            }
                        }
                        tray::TrayMenuEvent::OpenHistoryFolder => {
                            info!("Opening history folder");
                            history::open_history_folder();
                        }
                        tray::TrayMenuEvent::SelectHistoryEntry(index) => {
                            info!("Selecting history entry at index {}", index);
                            let entries = history::recent_entries(10);
                            if let Some(entry) = entries.get(index) {
                                match std::fs::read_to_string(&entry.path) {
                                    Ok(text) => {
                                        if let Err(e) = clipboard::copy(&text) {
                                            error!("Failed to copy to clipboard: {}", e);
                                        } else {
                                            info!(
                                                "Copied history entry {} to clipboard ({} chars)",
                                                index,
                                                text.len()
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        error!("Failed to read history entry: {}", e);
                                    }
                                }
                            } else {
                                warn!("History entry at index {} not found", index);
                            }
                        }
                        tray::TrayMenuEvent::ToggleHistory => {
                            info!("Toggle history save requested (always enabled for now)");
                            // History saving is always enabled in current implementation
                        }
                        tray::TrayMenuEvent::SetWhisperModel(model) => {
                            let variant = match model {
                                tray::WhisperModel::Small => "small",
                                tray::WhisperModel::Medium => "medium",
                                tray::WhisperModel::LargeV3 => "large-v3",
                                tray::WhisperModel::LargeV3Turbo => "large-v3-turbo",
                                tray::WhisperModel::LargeV3Q8 => "large-v3-mlx-q8",
                            };
                            // Skip if model is already set (CheckMenuItem sends event on menu show)
                            let current = std::env::var("WHISPER_VARIANT").unwrap_or_default();
                            if current == variant {
                                debug!(
                                    "Model already set to {}, ignoring duplicate event",
                                    variant
                                );
                                continue;
                            }
                            info!("Setting Whisper model to: {}", variant);
                            // Call backend to switch model
                            match client::set_whisper_model(variant).await {
                                Ok(()) => {
                                    info!("Whisper model switched to: {}", variant);
                                    // Update environment for next restart
                                    // SAFETY: Menu event handler, single-threaded context
                                    unsafe { std::env::set_var("WHISPER_VARIANT", variant) };
                                    // Note: Menu checkmarks already updated synchronously in handle_menu_event
                                }
                                Err(e) => {
                                    error!("Failed to switch Whisper model: {}", e);
                                    // Revert menu to previous state on error
                                    let current =
                                        std::env::var("WHISPER_VARIANT").unwrap_or_default();
                                    if !current.is_empty() {
                                        tray::update_model_selection(&current);
                                    }
                                }
                            }
                        }
                        tray::TrayMenuEvent::OpenModelsFolder => {
                            info!("Open models folder requested (handled in tray.rs)");
                            // Action is handled directly in tray.rs handle_menu_event
                        }
                        tray::TrayMenuEvent::ToggleAiFormatting => {
                            info!("Toggle AI formatting requested");
                            // Toggle the AI formatting setting
                            let current = std::env::var("FORMAT_ENABLED")
                                .map(|v| v == "1" || v.to_lowercase() == "true")
                                .unwrap_or(false);
                            let new_value = if current { "0" } else { "1" };
                            // SAFETY: Menu event handler, single-threaded context
                            unsafe { std::env::set_var("FORMAT_ENABLED", new_value) };
                            info!(
                                "AI formatting {}",
                                if new_value == "1" {
                                    "enabled"
                                } else {
                                    "disabled"
                                }
                            );
                            // TODO: Refresh tray menu to show updated AI formatting state
                            // tray::update_formatting_toggle(new_value == "1");
                        }
                        tray::TrayMenuEvent::SetFormattingProvider(provider) => {
                            let provider_str = match provider {
                                tray::FormattingProvider::Harmony => "harmony",
                                tray::FormattingProvider::Ollama => "ollama",
                            };
                            info!("Setting formatting provider to: {}", provider_str);
                            // SAFETY: Menu event handler, single-threaded context
                            unsafe { std::env::set_var("AI_PROVIDER", provider_str) };
                            // TODO: Refresh tray menu to show updated provider selection
                            // tray::update_formatting_provider(provider_str);
                        }
                        // Sound settings
                        tray::TrayMenuEvent::ToggleStartSound => {
                            let mut cfg = config_clone.write().await;
                            cfg.beep_on_start = !cfg.beep_on_start;
                            let enabled = cfg.beep_on_start;
                            if let Err(e) =
                                cfg.save_to_env("BEEP_ON_START", if enabled { "1" } else { "0" })
                            {
                                error!("Failed to save beep setting: {}", e);
                            }
                            info!(
                                "Start sound {}",
                                if enabled { "enabled" } else { "disabled" }
                            );
                            // TODO: Refresh tray menu to show updated sound state
                            // tray::update_sound_settings();
                        }
                        tray::TrayMenuEvent::SetSoundType(sound) => {
                            let sound_name = match sound {
                                tray::SoundType::Tink => "Tink",
                                tray::SoundType::Pop => "Pop",
                            };
                            info!("Setting sound type to: {}", sound_name);
                            // SAFETY: Menu event handler, single-threaded context
                            unsafe { std::env::set_var("SOUND_TYPE", sound_name) };
                            // Play preview
                            sound::play_sound(sound_name);
                            // TODO: Refresh tray menu to show updated sound type
                            // tray::update_sound_settings();
                        }
                        tray::TrayMenuEvent::SetVolume(level) => {
                            let volume = level.as_f32();
                            info!("Setting volume to: {} ({})", level.label(), volume);
                            let mut cfg = config_clone.write().await;
                            cfg.sound_volume = volume;
                            if let Err(e) = cfg.save_to_env("SOUND_VOLUME", &volume.to_string()) {
                                error!("Failed to save volume setting: {}", e);
                            }
                            // Play preview sound at new volume
                            sound::play_sound_with_volume("Tink", volume);
                            // TODO: Refresh tray menu to show updated volume level
                            // tray::update_sound_settings();
                        }
                        // Hold hotkey settings
                        tray::TrayMenuEvent::SetHoldMods(mods) => {
                            info!("Setting hold modifiers to: {:?}", mods);
                            let mut cfg = config_clone.write().await;
                            cfg.hold_mods = mods;
                            if let Err(e) = cfg.save_to_env("HOLD_MODS", mods.as_str()) {
                                error!("Failed to save hold mods setting: {}", e);
                            }
                            // Apply runtime reconfiguration
                            hotkeys::set_hold_mods(mods);
                            // TODO: Refresh tray menu to show updated hotkey config
                            // tray::update_hotkey_settings();
                        }
                        tray::TrayMenuEvent::ToggleHoldExclusive => {
                            let mut cfg = config_clone.write().await;
                            cfg.hold_exclusive = !cfg.hold_exclusive;
                            let exclusive = cfg.hold_exclusive;
                            if let Err(e) =
                                cfg.save_to_env("HOLD_EXCLUSIVE", if exclusive { "1" } else { "0" })
                            {
                                error!("Failed to save exclusive setting: {}", e);
                            }
                            // Apply runtime reconfiguration
                            hotkeys::set_exclusive_mode(exclusive);
                            info!(
                                "Exclusive mode {} (applied immediately)",
                                if exclusive { "enabled" } else { "disabled" }
                            );
                            // TODO: Refresh tray menu to show updated hotkey config
                            // tray::update_hotkey_settings();
                        }
                        tray::TrayMenuEvent::SetToggleTrigger(trigger) => {
                            info!(
                                "Setting toggle trigger to: {} ({})",
                                trigger.label(),
                                trigger.as_str()
                            );
                            let mut cfg = config_clone.write().await;
                            cfg.toggle_trigger = trigger;
                            if let Err(e) = cfg.save_to_env("TOGGLE_TRIGGER", trigger.as_str()) {
                                error!("Failed to save toggle trigger setting: {}", e);
                            }
                            // Apply runtime reconfiguration
                            hotkeys::set_toggle_trigger(trigger);
                            // TODO: Refresh tray menu to show updated hotkey config
                            // tray::update_hotkey_settings();
                        }
                        // Permissions
                        tray::TrayMenuEvent::CheckPermissions => {
                            info!("Checking permissions...");
                            permissions::check_all_permissions();
                            // Note: Menu needs refresh to show updated status
                        }
                        tray::TrayMenuEvent::OpenAccessibilitySettings => {
                            info!("Open Accessibility Settings (handled in tray.rs)");
                        }
                        tray::TrayMenuEvent::OpenMicrophoneSettings => {
                            info!("Open Microphone Settings (handled in tray.rs)");
                        }
                        // Tools submenu
                        tray::TrayMenuEvent::OpenVoiceLab => {
                            info!("Open Voice Lab (handled in tray.rs)");
                        }
                        tray::TrayMenuEvent::OpenTeacher => {
                            info!("Open Calibration Teacher (handled in tray.rs)");
                        }
                        tray::TrayMenuEvent::OpenNativeLab => {
                            info!("Open Native Lab (handled in tray.rs)");
                        }
                        tray::TrayMenuEvent::NewConversation => {
                            info!("New conversation started (handled in tray.rs)");
                        }
                        // Appearance
                        tray::TrayMenuEvent::ToggleStatusGlyph => {
                            let new_state = !tray::is_status_glyph_enabled();
                            info!(
                                "Toggling status glyph to: {}",
                                if new_state { "enabled" } else { "disabled" }
                            );
                            tray::set_status_glyph_enabled(new_state);
                            // Refresh icon to apply change
                            let _ = tray::update_tray_status(tray::TrayStatus::Idle);
                        }
                        tray::TrayMenuEvent::RefreshTrayIcon => {
                            info!("Refreshing tray icon...");
                            let _ = tray::update_tray_status(tray::TrayStatus::Idle);
                        }
                        // System
                        tray::TrayMenuEvent::StartAtLogin(enabled) => {
                            info!("Start at login: {}", enabled);
                            let result = if enabled {
                                launchd::enable_login_item()
                            } else {
                                launchd::disable_login_item()
                            };

                            match result {
                                Ok(()) => {
                                    info!(
                                        "Successfully {} Start at Login",
                                        if enabled { "enabled" } else { "disabled" }
                                    );
                                }
                                Err(e) => {
                                    error!(
                                        "Failed to {} Start at Login: {}",
                                        if enabled { "enable" } else { "disable" },
                                        e
                                    );
                                }
                            }
                        } // Note: ToggleHotkeys is handled above (line ~176)
                    }
                }
                Err(e) => {
                    error!("Menu channel closed: {}", e);
                    break;
                }
            }
        }
        info!("Menu event loop terminated");
    });

    // Spawn blocking task to handle hotkey events (rx.recv() is blocking)
    // Get runtime handle BEFORE spawning thread (must be in tokio context)
    let rt = tokio::runtime::Handle::current();
    let controller_clone = Arc::clone(&controller);
    std::thread::spawn(move || {
        info!("Hotkey event loop started");
        loop {
            // Receive hotkey event from channel (blocking)
            match rx.recv() {
                Ok(raw_event) => {
                    info!("Received hotkey event: {:?}", raw_event);
                    // Convert hotkeys::HotkeyEvent to controller::HotkeyInput
                    let controller_event = match raw_event {
                        hotkeys::HotkeyEvent::Hold { action, assistive } => {
                            let controller_action = match action {
                                hotkeys::HoldAction::Down => controller::HotkeyAction::Down,
                                hotkeys::HoldAction::Up => controller::HotkeyAction::Up,
                            };
                            controller::HotkeyInput {
                                key_type: controller::HotkeyType::Hold,
                                action: controller_action,
                                assistive,
                            }
                        }
                        hotkeys::HotkeyEvent::Toggle => controller::HotkeyInput {
                            key_type: controller::HotkeyType::Toggle,
                            action: controller::HotkeyAction::Press,
                            assistive: false,
                        },
                    };

                    // Handle the event asynchronously (don't block the receiver thread)
                    let controller = Arc::clone(&controller_clone);
                    rt.spawn(async move {
                        if let Err(e) = controller.handle_hotkey_event(controller_event).await {
                            error!("Error handling hotkey event: {}", e);
                        }
                    });
                }
                Err(e) => {
                    error!("Hotkey channel closed: {}", e);
                    break;
                }
            }
        }
        info!("Hotkey event loop terminated");
    });

    // Run the tray application with hotkey manager (blocking)
    // Both tray and hotkeys run on main thread with shared event loop
    info!("Starting system tray...");
    tray::run_with_hotkeys(hotkey_manager)?;

    info!("CodeScribe shutting down...");
    release_pid_lock();
    Ok(())
}
