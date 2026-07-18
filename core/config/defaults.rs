//! Default value functions for Codescribe configuration.
//!
//! These are used by serde for deserialization defaults.

pub const DEFAULT_OPENAI_RESPONSES_ENDPOINT: &str = "https://api.openai.com/v1/responses";
pub const DEFAULT_LLM_MODEL: &str = DEFAULT_FORMATTING_MODEL;
pub const DEFAULT_FORMATTING_MODEL: &str = "gpt-4.1";
pub const DEFAULT_ASSISTIVE_MODEL: &str = "gpt-5.5";

/// Default LLM provider identity for both lanes — OpenAI Responses. This is the
/// protected default: neither lane routes to another provider unless explicitly
/// configured. Mirrors [`crate::llm::provider::ProviderKind::default`].
pub const DEFAULT_LLM_PROVIDER: &str = "openai-responses";

pub fn default_llm_endpoint() -> String {
    DEFAULT_OPENAI_RESPONSES_ENDPOINT.to_string()
}

pub fn default_llm_endpoint_option() -> Option<String> {
    Some(default_llm_endpoint())
}

pub fn default_llm_model() -> String {
    DEFAULT_LLM_MODEL.to_string()
}

pub fn default_formatting_model() -> String {
    DEFAULT_FORMATTING_MODEL.to_string()
}

pub fn default_assistive_model() -> String {
    DEFAULT_ASSISTIVE_MODEL.to_string()
}

pub fn default_formatting_provider() -> String {
    DEFAULT_LLM_PROVIDER.to_string()
}

pub fn default_assistive_provider() -> String {
    DEFAULT_LLM_PROVIDER.to_string()
}

pub fn default_hold_start_delay_ms() -> u64 {
    800
}

pub fn default_double_tap_interval_ms() -> u64 {
    200
}

pub fn default_toggle_silence_sec() -> f32 {
    5.0
}

// Token limits removed - API decides. Tokens are cheap, lost notes are not.
pub fn default_ai_max_tokens() -> i32 {
    0 // 0 = no limit
}

pub fn default_ai_assistive_max_tokens() -> i32 {
    0 // 0 = no limit
}

pub fn default_transcript_tag_template() -> String {
    crate::transcript_tagging::DEFAULT_TRANSCRIPT_TAG_TEMPLATE.to_string()
}

pub fn default_show_tray_glyph() -> bool {
    true
}

pub fn default_show_dock_icon() -> bool {
    true
}

pub fn default_transcription_overlay_enabled() -> bool {
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

pub fn default_restore_clipboard() -> bool {
    true
}

pub fn default_restore_clipboard_delay_ms() -> u64 {
    1000
}

pub fn default_agent_enter_sends() -> bool {
    true
}
pub fn default_dump_audio_logs() -> bool {
    true
}

pub fn default_local_model() -> String {
    super::models::DEFAULT_MODEL.to_string()
}

pub fn default_stt_initial_prompt_enabled() -> bool {
    false
}
