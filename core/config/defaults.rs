//! Default value functions for Codescribe configuration.
//!
//! These are used by serde for deserialization defaults.

/// Milliseconds a hotkey must be held before hold-to-dictate arms.
///
/// Long enough that an ordinary keypress does not start a recording.
pub fn default_hold_start_delay_ms() -> u64 {
    800
}

/// Maximum gap in milliseconds between two taps for a double-tap gesture.
pub fn default_double_tap_interval_ms() -> u64 {
    200
}

/// Seconds of silence that end a recording in toggle mode.
pub fn default_toggle_silence_sec() -> f32 {
    5.0
}

// Token limits removed - API decides. Tokens are cheap, lost notes are not.
/// Output token cap for the formatting lane; `0` means no cap.
pub fn default_ai_max_tokens() -> i32 {
    0 // 0 = no limit
}

/// Output token cap for the assistive lane; `0` means no cap.
pub fn default_ai_assistive_max_tokens() -> i32 {
    0 // 0 = no limit
}

/// Template wrapping a transcript when it is tagged for insertion.
pub fn default_transcript_tag_template() -> String {
    crate::transcript_tagging::DEFAULT_TRANSCRIPT_TAG_TEMPLATE.to_string()
}

/// Show the menu-bar status glyph.
pub fn default_show_tray_glyph() -> bool {
    true
}

/// Show the Dock icon.
pub fn default_show_dock_icon() -> bool {
    true
}

/// Show the live transcription overlay while recording.
pub fn default_transcription_overlay_enabled() -> bool {
    true
}

/// Show the badge marking an armed hold gesture.
pub fn default_hold_indicator() -> bool {
    true
}

/// Hold badge edge length in points.
pub fn default_hold_badge_size() -> u32 {
    12
}

/// Horizontal hold-badge offset from the cursor, in points.
pub fn default_hold_badge_offset_x() -> i32 {
    10
}

/// Vertical hold-badge offset from the cursor, in points. Negative places the
/// badge above the cursor.
pub fn default_hold_badge_offset_y() -> i32 {
    -10
}

/// Play a sound when recording starts.
pub fn default_beep_on_start() -> bool {
    true
}

/// macOS system sound used for that cue.
pub fn default_sound_name() -> String {
    "Tink".to_string()
}

/// Cue playback volume, 0.0–1.0.
pub fn default_sound_volume() -> f32 {
    1.0
}

/// Keep a local history of transcriptions.
pub fn default_history_enabled() -> bool {
    true
}

/// Restore the previous clipboard contents after a paste-based insertion.
pub fn default_restore_clipboard() -> bool {
    true
}

/// Delay in milliseconds before restoring the clipboard, giving the target app
/// time to read the paste before it is taken back.
pub fn default_restore_clipboard_delay_ms() -> u64 {
    1000
}

/// Enter sends the message in the agent composer (rather than inserting a newline).
pub fn default_agent_enter_sends() -> bool {
    true
}

/// Write audio diagnostics to the logs.
pub fn default_dump_audio_logs() -> bool {
    true
}

/// Whisper model the runtime fallback looks for when nothing is configured.
pub fn default_local_model() -> String {
    super::models::DEFAULT_MODEL.to_string()
}

/// Whether to prime Whisper with an initial prompt.
///
/// Off by default: a domain prompt biases decoding and can inject its own
/// vocabulary into unrelated speech, so it stays opt-in.
pub fn default_stt_initial_prompt_enabled() -> bool {
    false
}
