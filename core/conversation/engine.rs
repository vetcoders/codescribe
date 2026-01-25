//! Conversation Engine - main wrapper for Moshi conversational AI.
//!
//! Provides full-duplex conversation capabilities using Kyutai's Moshi model.
//! The engine handles:
//! - Audio encoding/decoding via Mimi codec
//! - Language model inference via Helium
//! - Turn-taking and interruption
//! - Context management
//!
//! Created by M&K (c)2026 VetCoders

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;
use candle_core::Device;
use tracing::{debug, info, warn};

use super::config::MoshiConfig;
use super::context::{ConversationContext, ConversationState, Turn};
use super::turns::{TurnConfig, TurnManager};
use super::{FRAME_SAMPLES, SAMPLE_RATE};

/// Main conversation engine using Moshi
pub struct ConversationEngine {
    /// Moshi configuration
    config: MoshiConfig,

    /// Compute device (Metal/CPU)
    device: Device,

    /// Conversation context
    context: ConversationContext,

    /// Turn manager
    turn_manager: TurnManager,

    /// Whether engine is initialized
    initialized: bool,

    /// Whether model is currently speaking
    is_speaking: Arc<AtomicBool>,

    /// Audio input buffer (accumulates samples until we have a full frame)
    input_buffer: Vec<f32>,

    /// Audio output buffer (generated response samples)
    output_buffer: Vec<f32>,

    /// Pending audio codes from user
    pending_user_codes: Vec<Vec<u32>>,

    /// Generated response codes
    response_codes: Vec<Vec<u32>>,
}

impl ConversationEngine {
    /// Create a new conversation engine (lazy initialization)
    ///
    /// The actual model loading happens on first use or explicit `init()` call.
    pub fn new(config: MoshiConfig) -> Result<Self> {
        let device = Device::new_metal(0).unwrap_or(Device::Cpu);

        info!(
            "ConversationEngine created (device: {:?}, voice: {})",
            device, config.voice
        );

        Ok(Self {
            config,
            device,
            context: ConversationContext::new(),
            turn_manager: TurnManager::new(TurnConfig::default()),
            initialized: false,
            is_speaking: Arc::new(AtomicBool::new(false)),
            input_buffer: Vec::with_capacity(FRAME_SAMPLES * 2),
            output_buffer: Vec::new(),
            pending_user_codes: Vec::new(),
            response_codes: Vec::new(),
        })
    }

    /// Initialize the engine (load models)
    ///
    /// This is called automatically on first use, but can be called explicitly
    /// to pre-warm the models.
    pub fn init(&mut self) -> Result<()> {
        if self.initialized {
            return Ok(());
        }

        // Validate config
        if let Err(e) = self.config.validate() {
            warn!("Moshi model validation failed: {}. Models will be loaded on demand.", e);
            // Don't fail - models might be downloaded later
        }

        info!(
            "Initializing ConversationEngine with {} voice...",
            self.config.voice
        );

        // TODO: Load Moshi LM and Mimi codec here
        // For now, we mark as initialized and defer actual loading
        // until moshi crate provides stable public API

        self.initialized = true;
        info!("ConversationEngine initialized");

        Ok(())
    }

    /// Process incoming audio samples from user
    ///
    /// Audio should be 24kHz mono f32 samples.
    /// Returns true if a complete frame was processed.
    pub fn process_audio(&mut self, samples: &[f32]) -> Result<bool> {
        // Accumulate samples
        self.input_buffer.extend_from_slice(samples);

        // Check if we have enough for a frame
        if self.input_buffer.len() < FRAME_SAMPLES {
            return Ok(false);
        }

        // Extract frame
        let frame: Vec<f32> = self.input_buffer.drain(..FRAME_SAMPLES).collect();

        // Update turn state based on VAD (resample from 24kHz to 16kHz internally)
        let speech_prob = crate::vad::speech_probability(&frame, SAMPLE_RATE);
        let is_speech = speech_prob > 0.5;

        let (state, changed) = self.turn_manager.update(is_speech);

        if changed {
            debug!("Turn state changed to: {:?}", state);
            self.context.set_state(state);
        }

        // If user is speaking, encode audio
        if state == ConversationState::UserSpeaking && is_speech {
            // TODO: Encode with Mimi codec
            // let codes = self.encode_audio(&frame)?;
            // self.pending_user_codes.push(codes);
        }

        // If turn ended, start generating response
        if self.turn_manager.has_pending_response() {
            self.generate_response()?;
        }

        Ok(true)
    }

    /// Generate response to user input
    fn generate_response(&mut self) -> Result<()> {
        if self.pending_user_codes.is_empty() {
            debug!("No user codes to respond to");
            return Ok(());
        }

        info!("Generating response...");
        self.is_speaking.store(true, Ordering::SeqCst);

        // Record user turn
        self.context.add_turn(Turn::user(None));

        // TODO: Run Helium LM inference
        // For now, just clear the buffer and mark as done

        self.pending_user_codes.clear();
        self.turn_manager.start_assistant_turn();

        // TODO: Generate actual response
        // let response_codes = self.run_inference()?;
        // self.response_codes = response_codes;

        // Simulate response completion for now
        self.turn_manager.end_assistant_turn();
        self.is_speaking.store(false, Ordering::SeqCst);

        // Record assistant turn
        self.context.add_turn(Turn::assistant(None));

        Ok(())
    }

    /// Get response audio samples (if available)
    ///
    /// Returns None if no response is ready, or Some(samples) if there's audio.
    pub fn get_response(&mut self) -> Option<Vec<f32>> {
        if self.output_buffer.is_empty() {
            return None;
        }

        // Return and clear the buffer
        let samples = std::mem::take(&mut self.output_buffer);
        Some(samples)
    }

    /// Check if the model is currently speaking
    pub fn is_speaking(&self) -> bool {
        self.is_speaking.load(Ordering::SeqCst)
    }

    /// Interrupt the current response
    pub fn interrupt(&mut self) {
        if self.is_speaking() {
            info!("Response interrupted");
            self.is_speaking.store(false, Ordering::SeqCst);
            self.output_buffer.clear();
            self.response_codes.clear();
            self.turn_manager.reset();
            self.context.set_state(ConversationState::Interrupted);
        }
    }

    /// Get the current conversation state
    pub fn state(&self) -> ConversationState {
        self.context.state()
    }

    /// Get the conversation context
    pub fn context(&self) -> &ConversationContext {
        &self.context
    }

    /// Get mutable access to context
    pub fn context_mut(&mut self) -> &mut ConversationContext {
        &mut self.context
    }

    /// Set system prompt
    pub fn set_system_prompt(&mut self, prompt: &str) {
        self.context.set_system_prompt(prompt);
    }

    /// Reset the conversation
    pub fn reset(&mut self) {
        self.context.reset();
        self.turn_manager.reset();
        self.input_buffer.clear();
        self.output_buffer.clear();
        self.pending_user_codes.clear();
        self.response_codes.clear();
        self.is_speaking.store(false, Ordering::SeqCst);
        info!("Conversation reset");
    }

    /// Get the device being used
    pub fn device(&self) -> &Device {
        &self.device
    }

    /// Get sample rate
    pub fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_creation() {
        let config = MoshiConfig::default();
        let engine = ConversationEngine::new(config);
        assert!(engine.is_ok());
    }

    #[test]
    fn test_initial_state() {
        let engine = ConversationEngine::new(MoshiConfig::default()).unwrap();
        assert_eq!(engine.state(), ConversationState::Idle);
        assert!(!engine.is_speaking());
    }

    #[test]
    fn test_reset() {
        let mut engine = ConversationEngine::new(MoshiConfig::default()).unwrap();
        engine.set_system_prompt("Test prompt");
        engine.reset();
        assert!(engine.context().system_prompt().is_none());
    }
}
