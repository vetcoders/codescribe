//! CodeScribe CLI - Local speech-to-text transcription
//!
//! Lightweight CLI for direct audio file transcription.
//! For tray app + GUI, use CodeScribe.app (Tauri bundle).
//!
//! Created by M&K (c)2026 VetCoders

use anyhow::Result;
use clap::{Parser, Subcommand};
use codescribe::{audio, whisper};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

mod lab_server;

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

    /// LLM model for formatting (defaults to config)
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
        let default_config = include_str!("../codescribe-core/src/config/default_env.txt");
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
        let llm_model =
            llm_model.unwrap_or_else(|| codescribe::config::Config::load().ollama_model);
        eprintln!("Formatting with AI ({})...", llm_model);
        let start = Instant::now();
        match format_with_ollama(&raw_text, &llm_model, &lang).await {
            Ok(formatted) => {
                eprintln!("Formatted in {:?}", start.elapsed());
                formatted
            }
            Err(e) => {
                eprintln!("Formatting failed: {} - using raw text", e);
                raw_text
            }
        }
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
    use tokio::sync::mpsc;

    eprintln!("CodeScribe Live Transcription");
    eprintln!("Press Ctrl+C to stop.");

    whisper::init()?;

    let mut recorder = codescribe::audio::streaming_recorder::StreamingRecorder::new()?;
    let (vad_tx, mut vad_rx) = mpsc::unbounded_channel::<()>();
    recorder.recorder.set_on_vad_stop({
        let vad_tx = vad_tx.clone();
        move || {
            let _ = vad_tx.send(());
        }
    });

    let emitter = StreamEmitter::new();
    recorder.set_delta_callback(Some(Arc::new({
        let emitter = Arc::clone(&emitter);
        move |delta: &str| {
            emitter.emit_raw(delta);
        }
    })));

    recorder.start(language).await?;

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            eprintln!("Stopping live transcription (Ctrl+C)...");
        }
        _ = vad_rx.recv() => {
            eprintln!("Stopping live transcription (VAD)...");
        }
    }

    let _ = recorder.stop().await?;
    emitter.finish();

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
            num_predict: 0,
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

async fn run_daemon() -> Result<()> {
    use anyhow::Context;
    use codescribe::config::Config;
    use codescribe::controller::RecordingController;
    use codescribe::hotkeys::HotkeyEvent;
    use codescribe::{hotkeys, ipc, tray};
    use crossbeam_channel::unbounded;
    use std::sync::Arc;
    use tokio::runtime::Handle;

    eprintln!("CodeScribe daemon starting...");

    #[cfg(target_os = "macos")]
    codescribe::set_dock_icon();

    codescribe::whisper::init().context("Failed to initialize Whisper")?;
    let controller = Arc::new(RecordingController::new());

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
        for event in menu_rx {
            let controller = Arc::clone(&menu_controller);
            let handle = menu_handle.clone();
            handle.spawn(async move {
                let config = Config::load();
                sync_hotkey_config(&config);
                controller.set_config(config).await;
            });

            if matches!(event, tray::TrayMenuEvent::Quit) {
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

    // VAD monitor task - auto-finish recording when silence detected
    let vad_controller = Arc::clone(&controller);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            if vad_controller.is_vad_triggered() {
                eprintln!("VAD triggered - auto-finishing recording");
                vad_controller.clear_vad_triggered();
                if let Err(e) = vad_controller.finish_recording().await {
                    eprintln!("VAD finish_recording error: {}", e);
                }
            }
        }
    });

    // Start Lab Server (Side-by-side comparison MVP)
    lab_server::start_lab_server();

    tray::run_with_hotkeys(Some(hotkey_manager))?;

    Ok(())
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

fn sync_hotkey_config(config: &codescribe::config::Config) {
    codescribe::hotkeys::set_hold_mods(config.hold_mods);
    codescribe::hotkeys::set_toggle_trigger(config.toggle_trigger);
    codescribe::hotkeys::set_exclusive_mode(config.hold_exclusive);
}

async fn dispatch_hotkey_event(
    event: codescribe::hotkeys::HotkeyEvent,
    controller: std::sync::Arc<codescribe::controller::RecordingController>,
) -> Result<()> {
    use codescribe::controller::{HotkeyAction, HotkeyInput, HotkeyType};
    use codescribe::hotkeys::{HoldAction, HotkeyEvent};

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
    }

    Ok(())
}
