//! Turn-taking management for conversational AI.
//!
//! Handles turn detection, interruption, and timing for natural conversation flow.

use std::time::{Duration, Instant};

use super::context::ConversationState;

/// Configuration for turn-taking behavior
#[derive(Debug, Clone)]
pub struct TurnConfig {
    /// Minimum speech duration to register as a turn (ms)
    pub min_speech_ms: u64,

    /// Silence duration to end a turn (ms)
    pub end_of_turn_silence_ms: u64,

    /// Silence duration to detect interruption (ms)
    pub interruption_threshold_ms: u64,

    /// Delay before assistant starts responding (ms)
    pub response_delay_ms: u64,

    /// Allow user to interrupt assistant
    pub allow_interruption: bool,
}

impl Default for TurnConfig {
    fn default() -> Self {
        Self {
            min_speech_ms: 100,
            end_of_turn_silence_ms: 800,
            interruption_threshold_ms: 200,
            response_delay_ms: 100,
            allow_interruption: true,
        }
    }
}

/// Manages turn-taking in conversation
#[derive(Debug)]
pub struct TurnManager {
    config: TurnConfig,

    /// Current speaker state
    state: ConversationState,

    /// When current turn started
    turn_start: Option<Instant>,

    /// When speech was last detected
    last_speech: Option<Instant>,

    /// When silence started
    silence_start: Option<Instant>,

    /// Whether we're waiting to start assistant response
    pending_response: bool,

    /// When assistant started speaking
    assistant_start: Option<Instant>,
}

impl Default for TurnManager {
    fn default() -> Self {
        Self::new(TurnConfig::default())
    }
}

impl TurnManager {
    /// Create a new turn manager with config
    pub fn new(config: TurnConfig) -> Self {
        Self {
            config,
            state: ConversationState::Idle,
            turn_start: None,
            last_speech: None,
            silence_start: None,
            pending_response: false,
            assistant_start: None,
        }
    }

    /// Update state based on VAD result
    ///
    /// Returns the new state and whether a state change occurred
    pub fn update(&mut self, is_speech: bool) -> (ConversationState, bool) {
        let now = Instant::now();
        let previous_state = self.state;

        match self.state {
            ConversationState::Idle => {
                if is_speech {
                    // User started speaking
                    self.state = ConversationState::UserSpeaking;
                    self.turn_start = Some(now);
                    self.last_speech = Some(now);
                    self.silence_start = None;
                }
            }

            ConversationState::UserSpeaking => {
                if is_speech {
                    self.last_speech = Some(now);
                    self.silence_start = None;
                } else {
                    // Silence detected
                    if self.silence_start.is_none() {
                        self.silence_start = Some(now);
                    }

                    let silence_duration = self
                        .silence_start
                        .map(|s| now.duration_since(s))
                        .unwrap_or(Duration::ZERO);

                    // Check if turn ended
                    if silence_duration.as_millis() as u64 >= self.config.end_of_turn_silence_ms {
                        // Check minimum speech duration
                        let speech_duration = self
                            .turn_start
                            .map(|s| {
                                self.last_speech
                                    .map(|e| e.duration_since(s))
                                    .unwrap_or(Duration::ZERO)
                            })
                            .unwrap_or(Duration::ZERO);

                        if speech_duration.as_millis() as u64 >= self.config.min_speech_ms {
                            // Valid turn ended, start processing
                            self.state = ConversationState::Processing;
                            self.pending_response = true;
                        } else {
                            // Too short, back to idle
                            self.state = ConversationState::Idle;
                            self.turn_start = None;
                        }
                    }
                }
            }

            ConversationState::Processing => {
                // Check for interruption during processing
                if is_speech && self.config.allow_interruption {
                    self.state = ConversationState::UserSpeaking;
                    self.turn_start = Some(now);
                    self.last_speech = Some(now);
                    self.silence_start = None;
                    self.pending_response = false;
                }
            }

            ConversationState::AssistantSpeaking => {
                // Check for user interruption
                if is_speech && self.config.allow_interruption {
                    let _speech_duration = self
                        .silence_start
                        .map(|_| Duration::ZERO)
                        .unwrap_or_else(|| {
                            self.last_speech
                                .map(|s| now.duration_since(s))
                                .unwrap_or(Duration::ZERO)
                        });

                    // Brief check: is user actually speaking (not just noise)?
                    if self.last_speech.is_none() {
                        self.last_speech = Some(now);
                    }

                    let user_speech_duration = self
                        .last_speech
                        .map(|s| now.duration_since(s))
                        .unwrap_or(Duration::ZERO);

                    if user_speech_duration.as_millis() as u64
                        >= self.config.interruption_threshold_ms
                    {
                        // User is interrupting
                        self.state = ConversationState::Interrupted;
                        self.turn_start = Some(now);
                    }
                } else if !is_speech {
                    self.last_speech = None;
                }
            }

            ConversationState::Interrupted => {
                // Transition to user speaking
                if is_speech {
                    self.state = ConversationState::UserSpeaking;
                    self.last_speech = Some(now);
                    self.silence_start = None;
                } else {
                    // User stopped after interruption
                    self.state = ConversationState::Idle;
                    self.turn_start = None;
                    self.last_speech = None;
                }
            }
        }

        (self.state, self.state != previous_state)
    }

    /// Signal that assistant should start speaking
    pub fn start_assistant_turn(&mut self) {
        self.state = ConversationState::AssistantSpeaking;
        self.assistant_start = Some(Instant::now());
        self.pending_response = false;
    }

    /// Signal that assistant finished speaking
    pub fn end_assistant_turn(&mut self) {
        self.state = ConversationState::Idle;
        self.assistant_start = None;
    }

    /// Check if there's a pending response to generate
    pub fn has_pending_response(&self) -> bool {
        self.pending_response
    }

    /// Get current state
    pub fn state(&self) -> ConversationState {
        self.state
    }

    /// Get turn duration so far
    pub fn turn_duration(&self) -> Duration {
        self.turn_start
            .map(|s| Instant::now().duration_since(s))
            .unwrap_or(Duration::ZERO)
    }

    /// Reset to idle state
    pub fn reset(&mut self) {
        self.state = ConversationState::Idle;
        self.turn_start = None;
        self.last_speech = None;
        self.silence_start = None;
        self.pending_response = false;
        self.assistant_start = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_idle_to_speaking() {
        let mut manager = TurnManager::default();

        let (state, changed) = manager.update(true);
        assert_eq!(state, ConversationState::UserSpeaking);
        assert!(changed);
    }

    #[test]
    fn test_speech_continues() {
        let mut manager = TurnManager::default();

        manager.update(true); // Start speaking
        let (state, changed) = manager.update(true); // Continue

        assert_eq!(state, ConversationState::UserSpeaking);
        assert!(!changed);
    }

    #[test]
    fn test_reset() {
        let mut manager = TurnManager::default();

        manager.update(true);
        manager.reset();

        assert_eq!(manager.state(), ConversationState::Idle);
    }
}
