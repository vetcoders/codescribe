use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Result, bail};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Semaphore;
use tracing::{info, warn};

use super::{AppStatus, IpcCommand, IpcResponse};
use crate::audio::load_audio_file;
use crate::audio::streaming_recorder::transcribe_streaming_samples;
use crate::config::prompts::{
    DEFAULT_ASSISTIVE_PROMPT, DEFAULT_FORMATTING_PROMPT, get_assistive_prompt,
    get_assistive_prompt_path, get_formatting_prompt, get_formatting_prompt_path,
};
use crate::config::{AiProvider, Config};
use crate::controller::{HotkeyAction, HotkeyInput, HotkeyType, RecordingController, State};
use crate::stream_postprocess::StreamPostProcessor;
use crate::{ai_formatting, hotkeys};

const REDACTED_VALUE: &str = "<redacted>";

/// Maximum number of concurrent IPC client connections.
/// Prevents resource exhaustion from malicious/buggy local clients.
const MAX_CONCURRENT_CLIENTS: usize = 32;

pub async fn run_server(controller: Arc<RecordingController>) -> Result<()> {
    let socket_path = super::socket_path();
    ensure_socket_dir(&socket_path)?;

    match fs::remove_file(&socket_path) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => {
            warn!(
                "Failed to remove existing IPC socket {}: {}",
                socket_path.display(),
                e
            );
        }
    }

    let listener = UnixListener::bind(&socket_path)?;
    set_socket_permissions(&socket_path);
    info!("IPC server listening on {}", socket_path.display());

    // Semaphore to limit concurrent connections (DoS protection)
    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_CLIENTS));

    loop {
        let (stream, _) = listener.accept().await?;
        let controller = Arc::clone(&controller);

        // Acquire permit before spawning (blocks if at limit)
        let permit = Arc::clone(&semaphore).acquire_owned().await;
        if permit.is_err() {
            warn!("IPC semaphore closed unexpectedly");
            continue;
        }
        let permit = permit.unwrap();

        tokio::spawn(async move {
            // Permit is held for the duration of the connection
            let _permit = permit;

            if let Err(e) = verify_peer(&stream) {
                warn!("IPC client rejected: {}", e);
                return;
            }
            if let Err(e) = handle_client(stream, controller).await {
                warn!("IPC client error: {}", e);
            }
        });
    }
}

async fn handle_client(stream: UnixStream, controller: Arc<RecordingController>) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    while reader.read_line(&mut line).await? > 0 {
        let cmd = match serde_json::from_str::<IpcCommand>(&line) {
            Ok(cmd) => cmd,
            Err(e) => {
                let response = IpcResponse::Error(format!("Invalid JSON: {}", e));
                write_response(&mut writer, &response).await?;
                line.clear();
                continue;
            }
        };

        let response = handle_command(cmd, &controller).await;
        write_response(&mut writer, &response).await?;
        line.clear();
    }

    Ok(())
}

async fn write_response(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    response: &IpcResponse,
) -> Result<()> {
    let json = serde_json::to_string(response)?;
    writer.write_all(json.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

async fn handle_command(cmd: IpcCommand, controller: &RecordingController) -> IpcResponse {
    match cmd {
        IpcCommand::GetConfig => {
            let config = redact_config_for_ipc(Config::load());
            IpcResponse::Config(Box::new(config))
        }
        IpcCommand::SaveConfig { config } => {
            let config = merge_sensitive_fields(*config);
            if let Err(e) = persist_config(&config) {
                return IpcResponse::Error(format!("Failed to save config: {}", e));
            }

            hotkeys::set_hold_mods(config.hold_mods);
            hotkeys::set_toggle_trigger(config.toggle_trigger);
            hotkeys::set_exclusive_mode(config.hold_exclusive);

            controller.set_config(config).await;
            IpcResponse::Ok
        }
        IpcCommand::GetPrompt { prompt_type } => match prompt_type.as_str() {
            "formatting" => IpcResponse::Prompt(get_formatting_prompt()),
            "assistive" => IpcResponse::Prompt(get_assistive_prompt()),
            _ => IpcResponse::Error(format!("Unknown prompt type: {}", prompt_type)),
        },
        IpcCommand::SavePrompt {
            prompt_type,
            content,
        } => match prompt_spec(&prompt_type) {
            Some((path, _default)) => match save_prompt(&path, &content) {
                Ok(()) => IpcResponse::Ok,
                Err(e) => IpcResponse::Error(format!("Failed to save prompt: {}", e)),
            },
            None => IpcResponse::Error(format!("Unknown prompt type: {}", prompt_type)),
        },
        IpcCommand::ResetPrompt { prompt_type } => match prompt_spec(&prompt_type) {
            Some((path, default)) => match save_prompt(&path, default) {
                Ok(()) => IpcResponse::Prompt(default.to_string()),
                Err(e) => IpcResponse::Error(format!("Failed to reset prompt: {}", e)),
            },
            None => IpcResponse::Error(format!("Unknown prompt type: {}", prompt_type)),
        },
        IpcCommand::SendMessage { message } => {
            if message.trim().is_empty() {
                return IpcResponse::Error("Empty message".to_string());
            }

            let language = Config::load().whisper_language;
            let response =
                ai_formatting::format_text(&message, Some(language.as_str()), true).await;
            IpcResponse::Message(response)
        }
        IpcCommand::ResetContext => {
            ai_formatting::reset_ollama_memory();
            crate::state::conversation::reset_conversation();
            IpcResponse::Ok
        }
        IpcCommand::FormatTranscript {
            text,
            language,
            assistive,
        } => {
            if text.trim().is_empty() {
                return IpcResponse::Error("Empty text cannot be formatted".to_string());
            }

            let lang = language.as_deref();
            let formatted = ai_formatting::format_text(&text, lang, assistive).await;

            if formatted.trim().is_empty() {
                IpcResponse::Error("Formatting returned empty result".to_string())
            } else {
                IpcResponse::Message(formatted)
            }
        }
        IpcCommand::TranscribeFile { path } => {
            let audio_path = PathBuf::from(&path);
            if !audio_path.exists() {
                return IpcResponse::Error(format!(
                    "Audio file not found: {}",
                    audio_path.display()
                ));
            }

            let (samples, sample_rate) = match load_audio_file(&audio_path) {
                Ok(data) => data,
                Err(e) => {
                    return IpcResponse::Error(format!("Failed to load audio: {}", e));
                }
            };

            let language = Config::load().whisper_language;
            let mut postprocessor = StreamPostProcessor::new();
            match transcribe_streaming_samples(
                &samples,
                sample_rate,
                Some(language.as_str()),
                Some(&mut postprocessor),
            ) {
                Ok(text) => {
                    if text.trim().is_empty() {
                        IpcResponse::Error("Transcription returned empty result".to_string())
                    } else {
                        IpcResponse::Message(text)
                    }
                }
                Err(e) => IpcResponse::Error(format!("Transcription failed: {}", e)),
            }
        }
        IpcCommand::GetStatus => {
            let state = controller.current_state().await;
            let status = AppStatus {
                state: state_to_string(state),
                ai_formatting: Config::load().ai_formatting_enabled,
            };
            IpcResponse::Status(status)
        }
        IpcCommand::StartRecording { assistive } => {
            if controller.is_recording().await || controller.is_busy().await {
                return IpcResponse::Error("Recording already in progress".to_string());
            }

            let event = HotkeyInput {
                key_type: HotkeyType::Toggle,
                action: HotkeyAction::Press,
                assistive,
                force_ai: false,
            };

            match controller.handle_hotkey_event(event).await {
                Ok(()) => IpcResponse::Ok,
                Err(e) => IpcResponse::Error(format!("Failed to start recording: {}", e)),
            }
        }
        IpcCommand::StopRecording => {
            if !controller.is_recording().await {
                return IpcResponse::Error("No recording in progress".to_string());
            }

            match controller.finish_recording().await {
                Ok(()) => IpcResponse::Ok,
                Err(e) => IpcResponse::Error(format!("Failed to stop recording: {}", e)),
            }
        }
    }
}

fn state_to_string(state: State) -> String {
    match state {
        State::Idle => "idle".to_string(),
        State::RecHold | State::RecToggle => "recording".to_string(),
        State::Busy => "busy".to_string(),
    }
}

fn prompt_spec(prompt_type: &str) -> Option<(PathBuf, &'static str)> {
    match prompt_type {
        "formatting" => Some((get_formatting_prompt_path(), DEFAULT_FORMATTING_PROMPT)),
        "assistive" => Some((get_assistive_prompt_path(), DEFAULT_ASSISTIVE_PROMPT)),
        _ => None,
    }
}

fn save_prompt(path: &PathBuf, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)?;
    Ok(())
}

fn persist_config(config: &Config) -> Result<()> {
    let env_path = Config::env_path();
    if let Some(parent) = env_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut env_vars = if env_path.exists() {
        Config::parse_env_file(&env_path)?
    } else {
        HashMap::new()
    };

    let mut updated: Vec<(String, String)> = Vec::new();

    let mut put = |key: &str, value: String, env_vars: &mut HashMap<String, String>| {
        env_vars.insert(key.to_string(), value.clone());
        updated.push((key.to_string(), value));
    };

    put(
        "HOLD_MODS",
        config.hold_mods.as_str().to_string(),
        &mut env_vars,
    );
    put(
        "HOLD_EXCLUSIVE",
        bool_to_env(config.hold_exclusive),
        &mut env_vars,
    );
    put(
        "TOGGLE_TRIGGER",
        config.toggle_trigger.as_str().to_string(),
        &mut env_vars,
    );
    put(
        "HOLD_START_DELAY_MS",
        config.hold_start_delay_ms.to_string(),
        &mut env_vars,
    );

    put(
        "WHISPER_LANGUAGE",
        config.whisper_language.as_str().to_string(),
        &mut env_vars,
    );

    put(
        "AI_FORMATTING_ENABLED",
        bool_to_env(config.ai_formatting_enabled),
        &mut env_vars,
    );
    put(
        "AI_PROVIDER",
        ai_provider_to_env(config.ai_provider),
        &mut env_vars,
    );
    put(
        "AI_MAX_TOKENS",
        config.ai_max_tokens.to_string(),
        &mut env_vars,
    );
    put(
        "AI_ASSISTIVE_MAX_TOKENS",
        config.ai_assistive_max_tokens.to_string(),
        &mut env_vars,
    );

    put(
        "SHOW_TRAY_GLYPH",
        bool_to_env(config.show_tray_glyph),
        &mut env_vars,
    );
    put(
        "HOLD_INDICATOR",
        bool_to_env(config.hold_indicator),
        &mut env_vars,
    );
    put(
        "HOLD_BADGE_SIZE",
        config.hold_badge_size.to_string(),
        &mut env_vars,
    );
    put(
        "HOLD_BADGE_OFFSET_X",
        config.hold_badge_offset_x.to_string(),
        &mut env_vars,
    );
    put(
        "HOLD_BADGE_OFFSET_Y",
        config.hold_badge_offset_y.to_string(),
        &mut env_vars,
    );

    put(
        "BEEP_ON_START",
        bool_to_env(config.beep_on_start),
        &mut env_vars,
    );
    put("SOUND_NAME", config.sound_name.clone(), &mut env_vars);
    put(
        "SOUND_VOLUME",
        config.sound_volume.to_string(),
        &mut env_vars,
    );

    put(
        "AUDIO_INPUT_DEVICE",
        config.audio_input_device.clone().unwrap_or_default(),
        &mut env_vars,
    );

    put(
        "HISTORY_ENABLED",
        bool_to_env(config.history_enabled),
        &mut env_vars,
    );

    put(
        "USE_LOCAL_STT",
        bool_to_env(config.use_local_stt),
        &mut env_vars,
    );
    put("LOCAL_MODEL", config.local_model.clone(), &mut env_vars);
    put(
        "STT_ENDPOINT",
        config.stt_endpoint.clone().unwrap_or_default(),
        &mut env_vars,
    );
    put(
        "WHISPER_SERVER_URL",
        config.whisper_server_url.clone(),
        &mut env_vars,
    );

    put(
        "LLM_SERVER_URL",
        config.llm_server_url.clone(),
        &mut env_vars,
    );
    put("LLM_HOST", config.ollama_host.clone(), &mut env_vars);
    put("OLLAMA_HOST", config.ollama_host.clone(), &mut env_vars);
    put("LLM_MODEL", config.ollama_model.clone(), &mut env_vars);
    put("OLLAMA_MODEL", config.ollama_model.clone(), &mut env_vars);
    put(
        "LLM_ENDPOINT",
        config.llm_endpoint.clone().unwrap_or_default(),
        &mut env_vars,
    );
    put(
        "LLM_API_KEY",
        config.llm_api_key.clone().unwrap_or_default(),
        &mut env_vars,
    );
    put(
        "STT_API_KEY",
        config.stt_api_key.clone().unwrap_or_default(),
        &mut env_vars,
    );

    put(
        "RESTORE_CLIPBOARD",
        bool_to_env(config.restore_clipboard),
        &mut env_vars,
    );
    put(
        "RESTORE_CLIPBOARD_DELAY_MS",
        config.restore_clipboard_delay_ms.to_string(),
        &mut env_vars,
    );

    put(
        "START_AT_LOGIN",
        bool_to_env(config.start_at_login),
        &mut env_vars,
    );
    put(
        "DUMP_AUDIO_LOGS",
        bool_to_env(config.dump_audio_logs),
        &mut env_vars,
    );

    Config::write_env_file(&env_path, &env_vars)?;

    for (key, value) in updated {
        // SAFETY: This mirrors Config::save_to_env to keep runtime env in sync.
        unsafe { std::env::set_var(&key, &value) };
    }

    Ok(())
}

fn bool_to_env(value: bool) -> String {
    if value {
        "1".to_string()
    } else {
        "0".to_string()
    }
}

fn ai_provider_to_env(provider: AiProvider) -> String {
    match provider {
        AiProvider::Harmony => "harmony".to_string(),
        AiProvider::Ollama => "ollama".to_string(),
    }
}

fn redact_config_for_ipc(mut config: Config) -> Config {
    if config.llm_api_key.is_some() {
        config.llm_api_key = Some(REDACTED_VALUE.to_string());
    }
    if config.stt_api_key.is_some() {
        config.stt_api_key = Some(REDACTED_VALUE.to_string());
    }
    config
}

fn merge_sensitive_fields(mut config: Config) -> Config {
    let existing = Config::load();
    if config.llm_api_key.as_deref() == Some(REDACTED_VALUE) || config.llm_api_key.is_none() {
        config.llm_api_key = existing.llm_api_key;
    }
    if config.stt_api_key.as_deref() == Some(REDACTED_VALUE) || config.stt_api_key.is_none() {
        config.stt_api_key = existing.stt_api_key;
    }
    config
}

fn ensure_socket_dir(socket_path: &Path) -> Result<()> {
    let dir = socket_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("IPC socket path missing parent"))?;
    fs::create_dir_all(dir)?;
    let permissions = fs::Permissions::from_mode(0o700);
    if let Err(e) = fs::set_permissions(dir, permissions) {
        warn!(
            "Failed to set IPC socket directory permissions for {}: {}",
            dir.display(),
            e
        );
    }
    Ok(())
}

fn set_socket_permissions(socket_path: &Path) {
    let permissions = fs::Permissions::from_mode(0o600);
    if let Err(e) = fs::set_permissions(socket_path, permissions) {
        warn!(
            "Failed to set IPC socket permissions for {}: {}",
            socket_path.display(),
            e
        );
    }
}

fn verify_peer(stream: &UnixStream) -> Result<()> {
    let current_uid = unsafe { libc::geteuid() };
    let Some(peer_uid) = peer_uid(stream) else {
        bail!("Unable to determine peer uid");
    };
    if peer_uid != current_uid {
        bail!(
            "Peer uid {} does not match current uid {}",
            peer_uid,
            current_uid
        );
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn peer_uid(stream: &UnixStream) -> Option<libc::uid_t> {
    let fd = stream.as_raw_fd();
    let mut ucred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut ucred as *mut _ as *mut _,
            &mut len,
        )
    };
    (rc == 0).then_some(ucred.uid)
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
fn peer_uid(stream: &UnixStream) -> Option<libc::uid_t> {
    let fd = stream.as_raw_fd();
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    let rc = unsafe { libc::getpeereid(fd, &mut uid, &mut gid) };
    (rc == 0).then_some(uid)
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
)))]
fn peer_uid(_stream: &UnixStream) -> Option<libc::uid_t> {
    None
}
