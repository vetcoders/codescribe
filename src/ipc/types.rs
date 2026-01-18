use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub enum IpcCommand {
    // Config
    GetConfig,
    SaveConfig {
        config: Box<crate::config::Config>,
    },

    // Prompts
    GetPrompt {
        prompt_type: String,
    },
    SavePrompt {
        prompt_type: String,
        content: String,
    },
    ResetPrompt {
        prompt_type: String,
    },

    // AI / Chat
    SendMessage {
        message: String,
    },
    ResetContext,
    FormatTranscript {
        text: String,
        language: Option<String>,
        assistive: bool,
    },
    TranscribeFile {
        path: String,
    },

    // Status
    GetStatus,

    // Recording
    StartRecording {
        assistive: bool,
    },
    StopRecording,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum IpcResponse {
    Config(Box<crate::config::Config>),
    Prompt(String),
    Message(String),
    Status(AppStatus),
    Ok,
    Error(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppStatus {
    pub state: String, // "idle", "recording", "busy"
    pub ai_formatting: bool,
}
