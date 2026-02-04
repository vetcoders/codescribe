//! Controller helper functions
//!
//! Session state management and utility functions.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::RwLock;

use crate::config::Config;

/// Global flag for current session mode.
/// true = assistive (chat UI), false = non-assistive (simple transcription overlay)
/// This is set before recording starts and checked by the delta callback.
static IS_ASSISTIVE_SESSION: AtomicBool = AtomicBool::new(false);

/// Global flag for conversation mode (full-duplex Moshi).
/// When true, audio is routed to ConversationEngine instead of Whisper.
static IS_CONVERSATION_SESSION: AtomicBool = AtomicBool::new(false);

/// Set the current session mode (called before recording starts)
pub fn set_assistive_session(is_assistive: bool) {
    IS_ASSISTIVE_SESSION.store(is_assistive, Ordering::SeqCst);
}

/// Check if current session is assistive mode
pub fn is_assistive_session() -> bool {
    IS_ASSISTIVE_SESSION.load(Ordering::SeqCst)
}

/// Set conversation mode flag (Moshi full-duplex)
pub fn set_conversation_session(is_conversation: bool) {
    IS_CONVERSATION_SESSION.store(is_conversation, Ordering::SeqCst);
}

/// Check if current session is conversation mode (Moshi)
pub fn is_conversation_session() -> bool {
    IS_CONVERSATION_SESSION.load(Ordering::SeqCst)
}

/// Route transcription delta to the active overlay.
/// Assistive sessions stream into chat bubbles; non-assistive uses transcription overlay.
pub fn route_transcription_delta(delta: &str) {
    if is_assistive_session() {
        crate::voice_chat_ui::append_voice_chat_user_delta(delta);
    } else {
        // Non-assistive: live dictation preview in unified overlay
        crate::voice_chat_ui::append_transcription_delta(delta);
    }
}

/// Setup the voice chat send callback with config
pub fn setup_voice_chat_send_callback(config: Arc<RwLock<Config>>) {
    let callback_config = Arc::clone(&config);
    crate::voice_chat_ui::set_voice_chat_send_callback(Some(Arc::new(move |text: String| {
        let config = Arc::clone(&callback_config);
        tokio::spawn(async move {
            crate::voice_chat_ui::update_voice_chat_status("Sending...");
            crate::voice_chat_ui::set_voice_chat_sending(true);

            let (lang_str, transcript_mode) = {
                let cfg = config.read().await;
                (cfg.whisper_language, cfg.transcript_send_mode)
            };

            let use_streaming = matches!(
                transcript_mode,
                crate::config::TranscriptSendMode::Streaming
            );

            let delta_callback = if use_streaming {
                Some(Arc::new(|delta: &str| {
                    crate::voice_chat_ui::append_voice_chat_assistant_delta(delta);
                }) as Arc<dyn Fn(&str) + Send + Sync>)
            } else {
                None
            };

            let result = crate::ai_formatting::format_text_with_status(
                &text,
                Some(lang_str.as_str()),
                true,
                delta_callback,
            )
            .await;

            match result.status {
                crate::ai_formatting::AiFormatStatus::Applied => {
                    crate::voice_chat_ui::update_voice_chat_status("AI Response:");
                    crate::voice_chat_ui::set_voice_chat_text(&result.text);
                }
                crate::ai_formatting::AiFormatStatus::Failed => {
                    crate::voice_chat_ui::update_voice_chat_status("AI Failed");
                    crate::voice_chat_ui::add_voice_chat_error_message("AI Failed");
                }
                crate::ai_formatting::AiFormatStatus::Skipped => {
                    crate::voice_chat_ui::set_voice_chat_sending(false);
                }
            }
        });
    })));
}

/// Raw transcript saving is always enabled to avoid data loss.
pub fn raw_save_enabled() -> bool {
    true
}
