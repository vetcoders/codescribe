use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Semaphore;
use tracing::{info, warn};

use super::{AppStatus, IpcCommand, IpcResponse};
use crate::audio::load_audio_file;
use crate::config::prompts::{
    DEFAULT_ASSISTIVE_PROMPT, DEFAULT_FORMATTING_PROMPT, get_assistive_prompt,
    get_assistive_prompt_path, get_formatting_prompt, get_formatting_prompt_path,
};
use crate::config::{Config, UserSettings, keychain, settings::is_promoted_key};
use crate::controller::{HotkeyAction, HotkeyInput, HotkeyType, RecordingController, State};
use crate::stream_postprocess::StreamPostProcessor;
use crate::whisper;
use crate::{ai_formatting, os::hotkeys};

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
    let mut event_rx: Option<tokio::sync::broadcast::Receiver<codescribe_core::ipc::IpcEvent>> =
        None;

    loop {
        tokio::select! {
            read_result = reader.read_line(&mut line) => {
                let n = read_result?;
                if n == 0 {
                    break;
                }

                let cmd = match serde_json::from_str::<IpcCommand>(&line) {
                    Ok(cmd) => cmd,
                    Err(e) => {
                        let response = IpcResponse::Error(format!("Invalid JSON: {}", e));
                        write_response(&mut writer, &response).await?;
                        line.clear();
                        continue;
                    }
                };

                match cmd {
                    IpcCommand::Subscribe => {
                        event_rx = Some(controller.subscribe_events());
                        write_response(&mut writer, &IpcResponse::Ok).await?;
                    }
                    IpcCommand::Unsubscribe => {
                        event_rx = None;
                        write_response(&mut writer, &IpcResponse::Ok).await?;
                    }
                    _ => {
                        let response = handle_command(cmd, &controller).await;
                        write_response(&mut writer, &response).await?;
                    }
                }

                line.clear();
            }
            event = async {
                event_rx.as_mut().expect("event_rx checked by guard").recv().await
            }, if event_rx.is_some() => {
                match event {
                    Ok(ev) => {
                        write_response(&mut writer, &IpcResponse::Event(ev)).await?;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!("IPC subscriber lagged by {} event(s)", n);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        event_rx = None;
                    }
                }
            }
        }
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

            hotkeys::apply_hotkey_config(&config);

            controller.set_config(config).await;
            IpcResponse::Ok
        }
        IpcCommand::ReloadRuntimeConfig => {
            // UI handlers already set hotkey atomics synchronously before
            // sending this IPC command — only controller config reload needed.
            let config = Config::load();
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
            let response = ai_formatting::format_text_with_status(
                &message,
                Some(language.as_str()),
                true,
                None,
            )
            .await;
            IpcResponse::Message(response.text)
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
            let formatted =
                ai_formatting::format_text_with_status(&text, lang, assistive, None).await;

            if formatted.text.trim().is_empty() {
                IpcResponse::Error("Formatting returned empty result".to_string())
            } else {
                IpcResponse::Message(formatted.text)
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
            // Single-pass: engine handles 25s/5s chunking internally
            match whisper::transcribe(&samples, sample_rate, Some(language.as_str())) {
                Ok(raw_text) => {
                    // Apply lexicon/cleanup postprocessing
                    let mut postprocessor = StreamPostProcessor::new();
                    let text = postprocessor.process(&raw_text).unwrap_or(raw_text);
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
                hold_mode: crate::os::hotkeys::HoldMode::Raw,
                force_raw: !assistive,
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

            match controller.stop_recording_from_external_surface().await {
                Ok(()) => IpcResponse::Ok,
                Err(e) => IpcResponse::Error(format!("Failed to stop recording: {}", e)),
            }
        }
        IpcCommand::Subscribe | IpcCommand::Unsubscribe => {
            IpcResponse::Error("Subscribe/Unsubscribe are handled at connection level".to_string())
        }
    }
}

fn state_to_string(state: State) -> String {
    match state {
        State::Idle => "idle".to_string(),
        State::RecHold | State::RecToggle => "recording".to_string(),
        State::Busy => "busy".to_string(),
        State::Conversation => "conversation".to_string(),
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
    enum EnvUpdate {
        Set(String, String),
        Remove(String),
    }

    let env_path = Config::env_path();
    if let Some(parent) = env_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut env_vars = if env_path.exists() {
        Config::parse_env_file(&env_path)?
    } else {
        HashMap::new()
    };

    let mut updated: Vec<EnvUpdate> = Vec::new();
    let mut settings: Option<UserSettings> = None;
    let mut promoted_keys: Vec<String> = Vec::new();

    let mut put = |key: &str, value: String, env_vars: &mut HashMap<String, String>| {
        if is_promoted_key(key) {
            let settings = settings.get_or_insert_with(UserSettings::load);
            persist_promoted_setting(settings, key, &value);
            promoted_keys.push(key.to_string());
            // Keep promoted settings out of legacy .env to avoid stale overrides.
            env_vars.remove(key);
        } else {
            env_vars.insert(key.to_string(), value.clone());
        }
        updated.push(EnvUpdate::Set(key.to_string(), value));
    };

    put(
        "HOLD_EXCLUSIVE",
        bool_to_env(config.hold_exclusive),
        &mut env_vars,
    );
    put(
        "HOLD_START_DELAY_MS",
        config.hold_start_delay_ms.to_string(),
        &mut env_vars,
    );
    put(
        "DOUBLE_TAP_INTERVAL_MS",
        config.double_tap_interval_ms.to_string(),
        &mut env_vars,
    );
    put(
        "TOGGLE_SILENCE_SEC",
        config.toggle_silence_sec.to_string(),
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
        "SHOW_DOCK_ICON",
        bool_to_env(config.show_dock_icon),
        &mut env_vars,
    );
    put(
        "TRANSCRIPTION_OVERLAY_ENABLED",
        bool_to_env(config.transcription_overlay_enabled),
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
        "LLM_ENDPOINT",
        config.llm_endpoint.clone().unwrap_or_default(),
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

    match persist_secret_setting(
        "LLM_API_KEY",
        config.llm_api_key.clone().unwrap_or_default().as_str(),
        &mut env_vars,
    )? {
        Some(secret) => updated.push(EnvUpdate::Set("LLM_API_KEY".to_string(), secret)),
        None => updated.push(EnvUpdate::Remove("LLM_API_KEY".to_string())),
    }
    match persist_secret_setting(
        "STT_API_KEY",
        config.stt_api_key.clone().unwrap_or_default().as_str(),
        &mut env_vars,
    )? {
        Some(secret) => updated.push(EnvUpdate::Set("STT_API_KEY".to_string(), secret)),
        None => updated.push(EnvUpdate::Remove("STT_API_KEY".to_string())),
    }

    Config::write_env_file(&env_path, &env_vars)?;

    if let Some(settings) = settings
        && let Err(e) = settings.save()
    {
        let settings_path = UserSettings::settings_path();
        warn!(
            "IPC SaveConfig failed to persist promoted settings to {} (keys: {}). \
             Values are applied for this process only and may be lost on restart: {}",
            settings_path.display(),
            promoted_keys.join(", "),
            e
        );
    }

    for update in updated {
        match update {
            EnvUpdate::Set(key, value) => {
                // SAFETY: This mirrors Config::save_to_env to keep runtime env in sync.
                unsafe { std::env::set_var(&key, &value) };
            }
            EnvUpdate::Remove(key) => {
                // SAFETY: This mirrors Config::save_to_env semantics for clearing keys.
                unsafe { std::env::remove_var(&key) };
            }
        }
    }

    Ok(())
}

fn persist_secret_setting(
    key: &str,
    raw_value: &str,
    env_vars: &mut HashMap<String, String>,
) -> Result<Option<String>> {
    // Never store secrets in plaintext .env.
    env_vars.remove(key);

    let value = raw_value.trim();
    if value.is_empty() {
        keychain::delete_key(key)
            .with_context(|| format!("Failed to delete {key} from Keychain"))?;
        return Ok(None);
    }

    keychain::save_key(key, value).with_context(|| format!("Failed to save {key} to Keychain"))?;
    Ok(Some(value.to_string()))
}

fn bool_to_env(value: bool) -> String {
    if value {
        "1".to_string()
    } else {
        "0".to_string()
    }
}

fn persist_promoted_setting(settings: &mut UserSettings, key: &str, value: &str) {
    // String fields
    match key {
        "WHISPER_LANGUAGE" => settings.whisper_language = Some(value.to_string()),
        "LOCAL_MODEL" => settings.local_model = Some(value.to_string()),
        "STT_ENDPOINT" => settings.stt_endpoint = Some(value.to_string()),
        "AUDIO_INPUT_DEVICE" => settings.audio_input_device = Some(value.to_string()),
        "SOUND_NAME" => settings.sound_name = Some(value.to_string()),
        "LLM_ENDPOINT" => settings.llm_endpoint = Some(value.to_string()),
        "LLM_MODEL" => settings.llm_model = Some(value.to_string()),
        "LLM_ASSISTIVE_ENDPOINT" => settings.llm_assistive_endpoint = Some(value.to_string()),
        "LLM_ASSISTIVE_MODEL" => settings.llm_assistive_model = Some(value.to_string()),
        "FORMATTING_LEVEL" => settings.formatting_level = Some(value.to_string()),
        "LLM_FORMATTING_ENDPOINT" => settings.llm_formatting_endpoint = Some(value.to_string()),
        "LLM_FORMATTING_MODEL" => settings.llm_formatting_model = Some(value.to_string()),
        "TRANSCRIPT_SEND_MODE" => settings.transcript_send_mode = Some(value.to_string()),
        "WHISPER_MODEL" => settings.whisper_model = Some(value.to_string()),
        // u64 fields
        "HOLD_START_DELAY_MS" => {
            if let Ok(v) = value.parse::<u64>() {
                settings.hold_start_delay_ms = Some(v);
            }
        }
        "DOUBLE_TAP_INTERVAL_MS" => {
            if let Ok(v) = value.parse::<u64>() {
                settings.double_tap_interval_ms = Some(v);
            }
        }
        "CODESCRIBE_BUFFER_DELAY_MS" => {
            if let Ok(v) = value.parse::<u64>() {
                settings.buffer_delay_ms = Some(v);
            }
        }
        "CODESCRIBE_EMIT_WORDS_MAX" => {
            if let Ok(v) = value.parse::<u64>() {
                settings.emit_words_max = Some(v);
            }
        }
        "BACKEND_MAX_UPLOAD_MB" => {
            if let Ok(v) = value.parse::<u64>() {
                settings.backend_max_upload_mb = Some(v);
            }
        }
        // f32 fields
        "TOGGLE_SILENCE_SEC" => {
            if let Ok(v) = value.parse::<f32>() {
                settings.toggle_silence_sec = Some(v);
            }
        }
        "SOUND_VOLUME" => {
            if let Ok(v) = value.parse::<f32>() {
                settings.sound_volume = Some(v);
            }
        }
        "CODESCRIBE_TYPING_CPS" => {
            if let Ok(v) = value.parse::<f32>() {
                settings.typing_cps = Some(v);
            }
        }
        "CODESCRIBE_BUFFERED_INTERIM_SEC" => {
            if let Ok(v) = value.parse::<f32>() {
                settings.buffered_interim_sec = Some(v);
            }
        }
        // bool fields
        "HOLD_EXCLUSIVE"
        | "AI_FORMATTING_ENABLED"
        | "BEEP_ON_START"
        | "USE_LOCAL_STT"
        | "HISTORY_ENABLED"
        | "START_AT_LOGIN"
        | "QUICK_NOTES_ENABLED"
        | "QUICK_NOTES_SAVE_ONLY"
        | "AGENT_ENTER_SENDS"
        | "SHOW_DOCK_ICON"
        | "TRANSCRIPTION_OVERLAY_ENABLED" => {
            let bool_val = matches!(value, "1" | "true" | "yes" | "on");
            match key {
                "HOLD_EXCLUSIVE" => settings.hold_exclusive = Some(bool_val),
                "AI_FORMATTING_ENABLED" => settings.ai_formatting_enabled = Some(bool_val),
                "BEEP_ON_START" => settings.beep_on_start = Some(bool_val),
                "USE_LOCAL_STT" => settings.use_local_stt = Some(bool_val),
                "HISTORY_ENABLED" => settings.history_enabled = Some(bool_val),
                "START_AT_LOGIN" => settings.start_at_login = Some(bool_val),
                "QUICK_NOTES_ENABLED" => settings.quick_notes_enabled = Some(bool_val),
                "QUICK_NOTES_SAVE_ONLY" => settings.quick_notes_save_only = Some(bool_val),
                "AGENT_ENTER_SENDS" => settings.agent_enter_sends = Some(bool_val),
                "SHOW_DOCK_ICON" => settings.show_dock_icon = Some(bool_val),
                "TRANSCRIPTION_OVERLAY_ENABLED" => {
                    settings.transcription_overlay_enabled = Some(bool_val)
                }
                _ => unreachable!(),
            }
        }
        _ => {
            warn!("IPC promoted setting key is not mapped to UserSettings: {key}");
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use codescribe_core::ipc::{EngineEventWire, IpcEventPayload};
    use tokio::time::{Duration, timeout};

    async fn write_command(
        writer: &mut tokio::net::unix::OwnedWriteHalf,
        cmd: &IpcCommand,
    ) -> Result<()> {
        let json = serde_json::to_string(cmd)?;
        writer.write_all(json.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
        Ok(())
    }

    async fn read_response(
        reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    ) -> Result<IpcResponse> {
        let mut line = String::new();
        let bytes = timeout(Duration::from_secs(1), reader.read_line(&mut line))
            .await
            .context("timeout waiting for IPC response")??;
        anyhow::ensure!(bytes > 0, "IPC connection closed unexpectedly");
        Ok(serde_json::from_str::<IpcResponse>(&line)?)
    }

    #[tokio::test]
    async fn subscribe_streams_state_and_engine_events() {
        let controller = Arc::new(RecordingController::new());
        let (client, server) = UnixStream::pair().expect("unix pair");

        let server_controller = Arc::clone(&controller);
        let server_task =
            tokio::spawn(async move { handle_client(server, server_controller).await });

        let (reader_half, mut writer_half) = client.into_split();
        let mut reader = BufReader::new(reader_half);

        write_command(&mut writer_half, &IpcCommand::Subscribe)
            .await
            .expect("subscribe command");
        match read_response(&mut reader)
            .await
            .expect("subscribe response")
        {
            IpcResponse::Ok => {}
            other => panic!("expected Ok after Subscribe, got {:?}", other),
        }

        controller.publish_ipc_event_for_test(IpcEventPayload::StateChange {
            from: "idle".to_string(),
            to: "recording".to_string(),
        });
        match read_response(&mut reader)
            .await
            .expect("state change event response")
        {
            IpcResponse::Event(event) => match event.payload {
                IpcEventPayload::StateChange { from, to } => {
                    assert_eq!(from, "idle");
                    assert_eq!(to, "recording");
                }
                payload => panic!("expected state_change payload, got {:?}", payload),
            },
            other => panic!("expected Event response, got {:?}", other),
        }

        controller.publish_ipc_event_for_test(IpcEventPayload::Engine(EngineEventWire::Preview {
            rev: 7,
            text: "hello".to_string(),
        }));
        match read_response(&mut reader)
            .await
            .expect("engine event response")
        {
            IpcResponse::Event(event) => match event.payload {
                IpcEventPayload::Engine(EngineEventWire::Preview { rev, text }) => {
                    assert_eq!(rev, 7);
                    assert_eq!(text, "hello");
                }
                payload => panic!("expected preview engine payload, got {:?}", payload),
            },
            other => panic!("expected Event response, got {:?}", other),
        }

        write_command(&mut writer_half, &IpcCommand::Unsubscribe)
            .await
            .expect("unsubscribe command");
        match read_response(&mut reader)
            .await
            .expect("unsubscribe response")
        {
            IpcResponse::Ok => {}
            other => panic!("expected Ok after Unsubscribe, got {:?}", other),
        }

        controller.publish_ipc_event_for_test(IpcEventPayload::StateChange {
            from: "recording".to_string(),
            to: "idle".to_string(),
        });

        let mut line = String::new();
        let next = timeout(Duration::from_millis(150), reader.read_line(&mut line)).await;
        assert!(
            next.is_err(),
            "unsubscribed client unexpectedly received an event line: {line:?}"
        );

        drop(writer_half);
        let join = timeout(Duration::from_secs(1), server_task)
            .await
            .expect("server task timeout")
            .expect("server task panicked");
        assert!(join.is_ok(), "server task failed: {join:?}");
    }
}
