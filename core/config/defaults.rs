//! Default value functions for CodeScribe configuration.
//!
//! These are used by serde for deserialization defaults.

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
