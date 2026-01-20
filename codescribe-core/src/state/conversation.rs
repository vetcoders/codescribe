//! Conversation session tracking for AI modes
//!
//! Stores separate previous_response_id for each mode:
//! - Formatting mode: cleanup/transcription formatting
//! - Assistive mode: AI augmentation with context
//!
//! This prevents context bleeding between modes while maintaining
//! conversation continuity within each mode.

use std::sync::{OnceLock, RwLock};
use tracing::info;

/// AI mode for conversation tracking
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiMode {
    /// Formatting mode (Ctrl hold) - cleanup only
    Formatting,
    /// Assistive mode (Ctrl+Shift hold) - AI augmentation
    Assistive,
}

/// Conversation sessions - separate stream per mode
#[derive(Default)]
struct ConversationState {
    formatting_response_id: Option<String>,
    assistive_response_id: Option<String>,
}

/// Global conversation state
static CONVERSATION_STATE: OnceLock<RwLock<ConversationState>> = OnceLock::new();

/// Get the state lock, initializing if needed
fn get_state() -> &'static RwLock<ConversationState> {
    CONVERSATION_STATE.get_or_init(|| RwLock::new(ConversationState::default()))
}

/// Get the previous_response_id for a specific mode
///
/// Each mode has its own conversation chain - context doesn't bleed between modes.
pub fn get_previous_response_id_for_mode(mode: AiMode) -> Option<String> {
    let state = get_state().read().ok()?;
    match mode {
        AiMode::Formatting => state.formatting_response_id.clone(),
        AiMode::Assistive => state.assistive_response_id.clone(),
    }
}

/// Store the response_id for a specific mode
///
/// Call this after a successful LLM response to enable
/// conversation continuity within that mode.
pub fn set_response_id_for_mode(mode: AiMode, id: String) {
    if let Ok(mut state) = get_state().write() {
        info!("Stored {:?} mode response_id: {}", mode, id);
        match mode {
            AiMode::Formatting => state.formatting_response_id = Some(id),
            AiMode::Assistive => state.assistive_response_id = Some(id),
        }
    }
}

/// Reset conversation for a specific mode
pub fn reset_conversation_for_mode(mode: AiMode) {
    if let Ok(mut state) = get_state().write() {
        info!("{:?} mode conversation reset", mode);
        match mode {
            AiMode::Formatting => state.formatting_response_id = None,
            AiMode::Assistive => state.assistive_response_id = None,
        }
    }
}

/// Reset all conversation contexts
///
/// Clears both formatting and assistive mode history.
pub fn reset_conversation() {
    if let Ok(mut state) = get_state().write() {
        info!("All conversation contexts reset");
        state.formatting_response_id = None;
        state.assistive_response_id = None;
    }
}

/// Check if there's an active conversation in any mode
pub fn has_active_conversation() -> bool {
    get_state()
        .read()
        .map(|s| s.formatting_response_id.is_some() || s.assistive_response_id.is_some())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_separate_mode_conversations() {
        // Reset first to ensure clean state
        reset_conversation();

        // Set formatting mode response
        set_response_id_for_mode(AiMode::Formatting, "fmt_123".to_string());
        assert_eq!(
            get_previous_response_id_for_mode(AiMode::Formatting),
            Some("fmt_123".to_string())
        );
        assert_eq!(get_previous_response_id_for_mode(AiMode::Assistive), None);

        // Set assistive mode response - doesn't affect formatting
        set_response_id_for_mode(AiMode::Assistive, "ast_456".to_string());
        assert_eq!(
            get_previous_response_id_for_mode(AiMode::Formatting),
            Some("fmt_123".to_string())
        );
        assert_eq!(
            get_previous_response_id_for_mode(AiMode::Assistive),
            Some("ast_456".to_string())
        );

        // Reset one mode doesn't affect other
        reset_conversation_for_mode(AiMode::Formatting);
        assert_eq!(get_previous_response_id_for_mode(AiMode::Formatting), None);
        assert_eq!(
            get_previous_response_id_for_mode(AiMode::Assistive),
            Some("ast_456".to_string())
        );

        // Full reset clears both
        set_response_id_for_mode(AiMode::Formatting, "fmt_789".to_string());
        reset_conversation();
        assert_eq!(get_previous_response_id_for_mode(AiMode::Formatting), None);
        assert_eq!(get_previous_response_id_for_mode(AiMode::Assistive), None);
    }
}
