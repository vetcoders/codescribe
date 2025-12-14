//! Default value functions for CodeScribe configuration.
//!
//! These are used by serde for deserialization defaults.

pub fn default_hold_start_delay_ms() -> u64 {
    800
}

pub fn default_ai_max_tokens() -> i32 {
    512
}

pub fn default_ai_assistive_max_tokens() -> i32 {
    2048
}

pub fn default_show_tray_glyph() -> bool {
    true
}

pub fn default_hold_indicator() -> bool {
    true
}

pub fn default_hold_badge_size() -> u32 {
    12
}

pub fn default_hold_badge_offset_x() -> i32 {
    10
}

pub fn default_hold_badge_offset_y() -> i32 {
    -10
}

pub fn default_beep_on_start() -> bool {
    true
}

pub fn default_sound_name() -> String {
    "Tink".to_string()
}

pub fn default_sound_volume() -> f32 {
    1.0
}

pub fn default_history_enabled() -> bool {
    true
}

pub fn default_whisper_server_url() -> String {
    "http://localhost:8237".to_string()
}

pub fn default_llm_server_url() -> String {
    "http://localhost:8237".to_string()
}

pub fn default_ollama_host() -> String {
    "http://localhost:11434".to_string()
}

pub fn default_ollama_model() -> String {
    "llama3.2".to_string()
}

pub fn default_restore_clipboard() -> bool {
    true
}

pub fn default_restore_clipboard_delay_ms() -> u64 {
    1000
}

pub fn default_backend_ports() -> Vec<u16> {
    vec![8237, 7237, 6237, 5237]
}

pub fn default_silence_db() -> f32 {
    -45.0
}

pub fn default_silence_hang_sec() -> f32 {
    0.8
}
