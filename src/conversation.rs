//! Conversation session tracking for Voice Chat
//!
//! Stores previous_response_id for multi-turn conversations.
//! This enables continuity in Voice Chat sessions where the AI
//! can reference previous context.

use std::sync::{OnceLock, RwLock};
use tracing::info;

/// Current conversation session - stores the last response_id
static CURRENT_SESSION: OnceLock<RwLock<Option<String>>> = OnceLock::new();

/// Get the session lock, initializing if needed
fn get_session() -> &'static RwLock<Option<String>> {
    CURRENT_SESSION.get_or_init(|| RwLock::new(None))
}

/// Get the current previous_response_id (if any)
///
/// Returns the response_id from the last successful LLM call,
/// or None if this is a new conversation.
pub fn get_previous_response_id() -> Option<String> {
    get_session().read().ok()?.clone()
}

/// Store the response_id from the latest response
///
/// Call this after a successful LLM response to enable
/// conversation continuity.
pub fn set_response_id(id: String) {
    if let Ok(mut session) = get_session().write() {
        info!("Stored response_id for conversation: {}", id);
        *session = Some(id);
    }
}

/// Reset conversation (start new)
///
/// Clears the previous_response_id, effectively starting
/// a fresh conversation without prior context.
pub fn reset_conversation() {
    if let Ok(mut session) = get_session().write() {
        *session = None;
    }
    info!("Conversation reset - starting new session");
}

/// Check if there's an active conversation
pub fn has_active_conversation() -> bool {
    get_session()
        .read()
        .ok()
        .map(|guard| guard.is_some())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conversation_lifecycle() {
        // Start fresh
        reset_conversation();
        assert!(!has_active_conversation());
        assert!(get_previous_response_id().is_none());

        // Set a response ID
        set_response_id("resp_123".to_string());
        assert!(has_active_conversation());
        assert_eq!(get_previous_response_id(), Some("resp_123".to_string()));

        // Update response ID
        set_response_id("resp_456".to_string());
        assert_eq!(get_previous_response_id(), Some("resp_456".to_string()));

        // Reset
        reset_conversation();
        assert!(!has_active_conversation());
        assert!(get_previous_response_id().is_none());
    }
}
