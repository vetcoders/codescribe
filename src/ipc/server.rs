use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tracing::{info, warn};

use super::types::{AppStatus, IpcCommand, IpcResponse};
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

const SOCKET_PATH: &str = "/tmp/codescribe.sock";

pub async fn run_server(controller: Arc<RecordingController>) -> Result<()> {
    match std::fs::remove_file(SOCKET_PATH) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => {
            warn!(
                "Failed to remove existing IPC socket {}: {}",
                SOCKET_PATH, e
            );
        }
    }

    let listener = UnixListener::bind(SOCKET_PATH)?;
    info!("IPC server listening on {}", SOCKET_PATH);

    loop {
        let (stream, _) = listener.accept().await?;
        let controller = Arc::clone(&controller);

        tokio::spawn(async move {
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
        IpcCommand::GetConfig => IpcResponse::Config(Box::new(Config::load())),
        IpcCommand::SaveConfig { config } => {
            let config = *config;
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
