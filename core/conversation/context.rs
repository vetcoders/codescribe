//! Conversation context management.
//!
//! Tracks conversation history, embeddings, and state for context-aware responses.
//!
//! Created by M&K (c)2026 VetCoders

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Maximum number of turns to keep in history
const MAX_HISTORY_TURNS: usize = 20;

/// A single turn in the conversation
#[derive(Debug, Clone)]
pub struct Turn {
    /// Who spoke: "user" or "assistant"
    pub speaker: String,

    /// Transcribed text (if available)
    pub text: Option<String>,

    /// Audio codes (RVQ tokens)
    pub audio_codes: Option<Vec<Vec<u32>>>,

    /// Timestamp when turn started
    pub started_at: Instant,

    /// Duration of the turn
    pub duration: Duration,

    /// Embedding vector (from E5) for semantic search
    pub embedding: Option<Vec<f32>>,
}

impl Turn {
    /// Create a new user turn
    pub fn user(text: Option<String>) -> Self {
        Self {
            speaker: "user".to_string(),
            text,
            audio_codes: None,
            started_at: Instant::now(),
            duration: Duration::ZERO,
            embedding: None,
        }
    }

    /// Create a new assistant turn
    pub fn assistant(text: Option<String>) -> Self {
        Self {
            speaker: "assistant".to_string(),
            text,
            audio_codes: None,
            started_at: Instant::now(),
            duration: Duration::ZERO,
            embedding: None,
        }
    }

    /// Set audio codes
    pub fn with_audio_codes(mut self, codes: Vec<Vec<u32>>) -> Self {
        self.audio_codes = Some(codes);
        self
    }

    /// Set duration
    pub fn with_duration(mut self, duration: Duration) -> Self {
        self.duration = duration;
        self
    }

    /// Set embedding
    pub fn with_embedding(mut self, embedding: Vec<f32>) -> Self {
        self.embedding = Some(embedding);
        self
    }
}

/// Conversation context manager
#[derive(Debug)]
pub struct ConversationContext {
    /// History of turns
    history: VecDeque<Turn>,

    /// System prompt / persona
    system_prompt: Option<String>,

    /// Current conversation state
    state: ConversationState,

    /// Total conversation duration (sum of all turns)
    total_duration: Duration,

    /// When conversation started
    started_at: Instant,
}

/// Current state of the conversation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationState {
    /// Waiting for user input
    Idle,
    /// User is speaking
    UserSpeaking,
    /// Processing user input
    Processing,
    /// Assistant is speaking
    AssistantSpeaking,
    /// User interrupted assistant
    Interrupted,
}

impl Default for ConversationContext {
    fn default() -> Self {
        Self::new()
    }
}

impl ConversationContext {
    /// Create a new empty context
    pub fn new() -> Self {
        Self {
            history: VecDeque::with_capacity(MAX_HISTORY_TURNS),
            system_prompt: None,
            state: ConversationState::Idle,
            total_duration: Duration::ZERO,
            started_at: Instant::now(),
        }
    }

    /// Create with a system prompt
    pub fn with_system_prompt(prompt: &str) -> Self {
        let mut ctx = Self::new();
        ctx.system_prompt = Some(prompt.to_string());
        ctx
    }

    /// Add a turn to history
    pub fn add_turn(&mut self, turn: Turn) {
        // Track total speaking time
        self.total_duration += turn.duration;

        if self.history.len() >= MAX_HISTORY_TURNS {
            // Subtract duration of removed turn
            if let Some(old) = self.history.front() {
                self.total_duration = self.total_duration.saturating_sub(old.duration);
            }
            self.history.pop_front();
        }
        self.history.push_back(turn);
    }

    /// Get recent history
    pub fn recent_history(&self, n: usize) -> Vec<&Turn> {
        self.history.iter().rev().take(n).collect()
    }

    /// Get all history
    pub fn history(&self) -> &VecDeque<Turn> {
        &self.history
    }

    /// Get system prompt
    pub fn system_prompt(&self) -> Option<&str> {
        self.system_prompt.as_deref()
    }

    /// Set system prompt
    pub fn set_system_prompt(&mut self, prompt: &str) {
        self.system_prompt = Some(prompt.to_string());
    }

    /// Get current state
    pub fn state(&self) -> ConversationState {
        self.state
    }

    /// Set state
    pub fn set_state(&mut self, state: ConversationState) {
        self.state = state;
    }

    /// Check if user is currently speaking
    pub fn is_user_speaking(&self) -> bool {
        self.state == ConversationState::UserSpeaking
    }

    /// Check if assistant is currently speaking
    pub fn is_assistant_speaking(&self) -> bool {
        self.state == ConversationState::AssistantSpeaking
    }

    /// Get elapsed time since conversation started
    pub fn duration(&self) -> Duration {
        self.started_at.elapsed()
    }

    /// Get total speaking duration (sum of all turn durations)
    pub fn total_speaking_duration(&self) -> Duration {
        self.total_duration
    }

    /// Get number of turns
    pub fn turn_count(&self) -> usize {
        self.history.len()
    }

    /// Clear history but keep system prompt
    pub fn clear_history(&mut self) {
        self.history.clear();
        self.state = ConversationState::Idle;
    }

    /// Reset everything
    pub fn reset(&mut self) {
        self.history.clear();
        self.system_prompt = None;
        self.state = ConversationState::Idle;
        self.started_at = Instant::now();
    }

    /// Build context string for LLM (text-based fallback)
    pub fn build_text_context(&self, max_chars: usize) -> String {
        let mut context = String::new();

        if let Some(ref prompt) = self.system_prompt {
            context.push_str("System: ");
            context.push_str(prompt);
            context.push('\n');
        }

        // Add history in order (oldest first)
        for turn in &self.history {
            if let Some(ref text) = turn.text {
                let prefix = if turn.speaker == "user" {
                    "User: "
                } else {
                    "Assistant: "
                };
                context.push_str(prefix);
                context.push_str(text);
                context.push('\n');
            }

            // Stop if we exceed max chars
            if context.len() > max_chars {
                break;
            }
        }

        context
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_creation() {
        let ctx = ConversationContext::new();
        assert_eq!(ctx.state(), ConversationState::Idle);
        assert_eq!(ctx.turn_count(), 0);
    }

    #[test]
    fn test_add_turns() {
        let mut ctx = ConversationContext::new();

        ctx.add_turn(Turn::user(Some("Hello".to_string())));
        ctx.add_turn(Turn::assistant(Some("Hi there!".to_string())));

        assert_eq!(ctx.turn_count(), 2);
    }

    #[test]
    fn test_max_history() {
        let mut ctx = ConversationContext::new();

        for i in 0..30 {
            ctx.add_turn(Turn::user(Some(format!("Message {}", i))));
        }

        assert_eq!(ctx.turn_count(), MAX_HISTORY_TURNS);
    }
}
