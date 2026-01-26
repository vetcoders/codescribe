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

use anyhow::{Context, Result};
use candle_core::{Device, IndexOp, Tensor};
use tracing::{debug, info};

use super::config::MoshiConfig;
use super::context::{ConversationContext, ConversationState, Turn};
use super::turns::{TurnConfig, TurnManager};
use super::{FRAME_SAMPLES, NUM_CODEBOOKS, SAMPLE_RATE};

// ═══════════════════════════════════════════════════════════
// Resampler24k - converts any input rate to 24kHz for Moshi
// ═══════════════════════════════════════════════════════════

/// Resampler for converting audio to 24kHz (Moshi/Mimi codec rate)
///
/// Uses linear interpolation for efficiency. For production use with
/// critical quality needs, consider rubato or similar library.
pub struct Resampler24k {
    buffer: Vec<f32>,
    ratio: f32,
    input_rate: u32,
}

impl Resampler24k {
    /// Create resampler for given input sample rate
    pub fn new(input_rate: u32) -> Self {
        let ratio = SAMPLE_RATE as f32 / input_rate as f32;
        debug!(
            "Resampler24k created: {}Hz → {}Hz (ratio: {:.4})",
            input_rate, SAMPLE_RATE, ratio
        );
        Self {
            buffer: Vec::with_capacity(FRAME_SAMPLES * 2),
            ratio,
            input_rate,
        }
    }

    /// Resample audio to 24kHz (linear interpolation)
    ///
    /// Returns owned Vec to avoid lifetime complexity in callbacks.
    pub fn resample(&mut self, samples: &[f32]) -> Vec<f32> {
        // No resampling needed if already at target rate
        if (self.ratio - 1.0).abs() < 0.001 {
            return samples.to_vec();
        }

        let output_len = (samples.len() as f32 * self.ratio) as usize;
        self.buffer.clear();
        self.buffer.reserve(output_len);

        for i in 0..output_len {
            let src_idx = i as f32 / self.ratio;
            let idx0 = src_idx.floor() as usize;
            let idx1 = (idx0 + 1).min(samples.len().saturating_sub(1));
            let frac = src_idx - idx0 as f32;

            let sample = if idx0 < samples.len() {
                samples[idx0] * (1.0 - frac) + samples.get(idx1).copied().unwrap_or(0.0) * frac
            } else {
                0.0
            };
            self.buffer.push(sample);
        }

        self.buffer.clone()
    }

    /// Get the input sample rate this resampler was created for
    pub fn input_rate(&self) -> u32 {
        self.input_rate
    }
}

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

    /// Pending audio codes from user (each Vec<u32> is one timestep, NUM_CODEBOOKS values)
    pending_user_codes: Vec<Vec<u32>>,

    /// Generated response codes
    response_codes: Vec<Vec<u32>>,

    // === Moshi Models ===
    /// Mimi codec for audio encode/decode
    mimi: Option<moshi::mimi::Mimi>,

    /// LM generation state (Helium + DepFormer)
    lm_state: Option<moshi::lm_generate_multistream::State>,

    /// LM config (needed for state recreation)
    lm_config: Option<moshi::lm::Config>,

    /// Previous text token (for autoregressive generation)
    prev_text_token: u32,

    /// Number of generated audio codebooks
    generated_audio_codebooks: usize,

    /// Resampler for input audio (any rate → 24kHz)
    resampler: Option<Resampler24k>,
}

impl ConversationEngine {
    /// Create a new conversation engine (lazy initialization)
    ///
    /// The actual model loading happens on first use or explicit `init()` call.
    pub fn new(config: MoshiConfig) -> Result<Self> {
        let device = match Device::new_metal(0) {
            Ok(d) => {
                info!("Using Metal GPU for Moshi");
                d
            }
            Err(e) => {
                info!("Metal unavailable ({}), falling back to CPU", e);
                Device::Cpu
            }
        };

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
            // Moshi models (loaded in init)
            mimi: None,
            lm_state: None,
            lm_config: None,
            prev_text_token: 0,
            generated_audio_codebooks: NUM_CODEBOOKS,
            resampler: None,
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
        self.config
            .validate()
            .map_err(|e| anyhow::anyhow!("{}", e))?;

        info!(
            "Initializing ConversationEngine with {} voice...",
            self.config.voice
        );

        let dtype = self.device.bf16_default_to_f32();
        info!("Using dtype: {:?}, device: {:?}", dtype, self.device);

        // 1. Load Mimi codec
        info!("Loading Mimi codec from: {}", self.config.mimi_path.display());
        let mimi_path_str = self.config.mimi_path.to_string_lossy();
        let mimi = moshi::mimi::load(&mimi_path_str, Some(NUM_CODEBOOKS), &self.device)
            .context("Failed to load Mimi codec")?;
        info!("Mimi codec loaded");

        // 2. Load LM config (use built-in v0_1 config)
        let lm_config = moshi::lm::Config::v0_1();
        self.generated_audio_codebooks = lm_config
            .depformer
            .as_ref()
            .map_or(NUM_CODEBOOKS, |v| v.num_slices);

        // 3. Load LM model
        info!("Loading LM model from: {}", self.config.model_path.display());
        let lm_model =
            moshi::lm::load_lm_model(lm_config.clone(), &self.config.model_path, dtype, &self.device)
                .context("Failed to load Moshi LM model")?;
        info!("LM model loaded");

        // 4. Create generation state
        let audio_lp = candle_transformers::generation::LogitsProcessor::from_sampling(
            42, // seed
            candle_transformers::generation::Sampling::TopK {
                k: 250,
                temperature: self.config.temperature as f64,
            },
        );
        let text_lp = candle_transformers::generation::LogitsProcessor::from_sampling(
            42,
            candle_transformers::generation::Sampling::TopK {
                k: 250,
                temperature: self.config.temperature as f64,
            },
        );

        let gen_config = moshi::lm_generate_multistream::Config {
            acoustic_delay: self.config.acoustic_delay,
            audio_vocab_size: lm_config.audio_vocab_size,
            generated_audio_codebooks: self.generated_audio_codebooks,
            input_audio_codebooks: lm_config.audio_codebooks - self.generated_audio_codebooks,
            text_start_token: lm_config.text_out_vocab_size as u32,
            text_eop_token: 0,
            text_pad_token: 3,
        };

        let state = moshi::lm_generate_multistream::State::new(
            lm_model,
            self.config.max_response_frames + 20,
            audio_lp,
            text_lp,
            None,
            None,
            None, // cfg_alpha
            gen_config,
        );

        // Store text_start_token for generation
        self.prev_text_token = lm_config.text_out_vocab_size as u32;

        self.mimi = Some(mimi);
        self.lm_state = Some(state);
        self.lm_config = Some(lm_config);
        self.initialized = true;

        info!("ConversationEngine initialized successfully");
        Ok(())
    }

    /// Ensure engine is initialized before use
    fn ensure_init(&mut self) -> Result<()> {
        if !self.initialized {
            self.init()?;
        }
        Ok(())
    }

    /// Encode audio frame to Mimi codes
    fn encode_audio(&mut self, frame: &[f32]) -> Result<Option<Vec<u32>>> {
        let mimi = self
            .mimi
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Mimi not initialized"))?;

        // Convert to tensor: (1, 1, frame_len)
        let pcm = Tensor::from_vec(frame.to_vec(), (1, 1, frame.len()), &self.device)?;

        // Encode step
        let codes = mimi.encode_step(&pcm.into(), &().into())?;

        if let Some(codes) = codes.as_option() {
            let (_b, _codebooks, steps) = codes.dims3()?;
            if steps > 0 {
                // Get first step's codes
                let codes_vec = codes.i((0, .., 0))?.to_vec1::<u32>()?;
                return Ok(Some(codes_vec));
            }
        }

        Ok(None)
    }

    /// Decode audio codes to PCM samples
    fn decode_audio(&mut self, codes: &[u32]) -> Result<Option<Vec<f32>>> {
        let mimi = self
            .mimi
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Mimi not initialized"))?;

        // Convert to tensor: (codebooks, 1, 1) then transpose
        let audio_tokens =
            Tensor::new(&codes[..self.generated_audio_codebooks], &self.device)?
                .reshape((1, 1, self.generated_audio_codebooks))?
                .t()?;

        let out_pcm = mimi.decode_step(&audio_tokens.into(), &().into())?;

        if let Some(out_pcm) = out_pcm.as_option() {
            let samples = out_pcm.i((0, 0))?.to_vec1::<f32>()?;
            return Ok(Some(samples));
        }

        Ok(None)
    }

    /// Process incoming audio samples from user
    ///
    /// Audio should be 24kHz mono f32 samples.
    /// Returns true if a complete frame was processed.
    pub fn process_audio(&mut self, samples: &[f32]) -> Result<bool> {
        self.ensure_init()?;

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
        if state == ConversationState::UserSpeaking
            && is_speech
            && let Some(codes) = self.encode_audio(&frame)?
        {
            self.pending_user_codes.push(codes);
        }

        // If turn ended, start generating response
        if self.turn_manager.has_pending_response() {
            self.generate_response()?;
        }

        Ok(true)
    }

    /// Process incoming audio samples from any sample rate
    ///
    /// Automatically resamples to 24kHz before processing.
    /// This is the preferred method when receiving audio from CoreAudio (48kHz).
    ///
    /// Returns true if a complete frame was processed.
    pub fn process_audio_any_rate(&mut self, samples: &[f32], sample_rate: u32) -> Result<bool> {
        // Fast path: already at 24kHz
        if sample_rate == SAMPLE_RATE {
            return self.process_audio(samples);
        }

        // Lazy init resampler (or recreate if rate changed)
        let needs_new_resampler = self
            .resampler
            .as_ref()
            .is_none_or(|r| r.input_rate() != sample_rate);

        if needs_new_resampler {
            info!(
                "Initializing Resampler24k for {}Hz → {}Hz",
                sample_rate, SAMPLE_RATE
            );
            self.resampler = Some(Resampler24k::new(sample_rate));
        }

        // Resample and process
        let resampled = self
            .resampler
            .as_mut()
            .expect("resampler just initialized")
            .resample(samples);

        self.process_audio(&resampled)
    }

    /// Generate response to user input
    fn generate_response(&mut self) -> Result<()> {
        if self.pending_user_codes.is_empty() {
            debug!("No user codes to respond to");
            return Ok(());
        }

        info!(
            "Generating response for {} audio frames...",
            self.pending_user_codes.len()
        );
        self.is_speaking.store(true, Ordering::SeqCst);

        // Record user turn
        self.context.add_turn(Turn::user(None));
        self.turn_manager.start_assistant_turn();

        // Take ownership of pending codes to avoid borrow issues
        let user_codes = std::mem::take(&mut self.pending_user_codes);

        // Collect audio tokens to decode after LM processing
        let mut tokens_to_decode: Vec<Vec<u32>> = Vec::new();

        {
            let state = self
                .lm_state
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("LM state not initialized"))?;

            // Process each user audio code through the LM
            for codes in user_codes {
                // Run LM step
                self.prev_text_token = state.step_(
                    Some(self.prev_text_token),
                    &codes,
                    None, // no visual input
                    None, // no visual mask
                    None, // no conditions
                )?;

                // Collect generated audio tokens for later decoding
                if let Some(audio_tokens) = state.last_audio_tokens() {
                    tokens_to_decode.push(audio_tokens.to_vec());
                }
            }
        } // state borrow ends here

        // Now decode all collected audio tokens
        for audio_tokens in tokens_to_decode {
            if let Some(pcm) = self.decode_audio(&audio_tokens)? {
                self.output_buffer.extend(pcm);
            }
        }

        self.turn_manager.end_assistant_turn();
        self.is_speaking.store(false, Ordering::SeqCst);

        // Record assistant turn
        self.context.add_turn(Turn::assistant(None));

        info!(
            "Response generated: {} samples ({:.2}s)",
            self.output_buffer.len(),
            self.output_buffer.len() as f32 / SAMPLE_RATE as f32
        );

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

        // Reset LM state prev_text_token
        if let Some(config) = &self.lm_config {
            self.prev_text_token = config.text_out_vocab_size as u32;
        }

        // Clear resampler (will be recreated on next process_audio_any_rate call)
        self.resampler = None;

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

    /// Check if engine is initialized
    pub fn is_initialized(&self) -> bool {
        self.initialized
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
        assert!(!engine.is_initialized());
    }

    #[test]
    fn test_reset() {
        let mut engine = ConversationEngine::new(MoshiConfig::default()).unwrap();
        engine.set_system_prompt("Test prompt");
        engine.reset();
        assert!(engine.context().system_prompt().is_none());
    }

    #[test]
    fn test_resampler_48k_to_24k() {
        let mut resampler = Resampler24k::new(48000);
        assert_eq!(resampler.input_rate(), 48000);

        // 48kHz input: 480 samples = 10ms
        let input: Vec<f32> = (0..480).map(|i| (i as f32 * 0.01).sin()).collect();

        // Should become ~240 samples at 24kHz (ratio 0.5)
        let output = resampler.resample(&input);
        assert!((output.len() as i32 - 240).abs() <= 1);
    }

    #[test]
    fn test_resampler_24k_passthrough() {
        let mut resampler = Resampler24k::new(24000);

        let input: Vec<f32> = (0..512).map(|i| (i as f32 * 0.01).sin()).collect();
        let output = resampler.resample(&input);

        // Should be same length (passthrough)
        assert_eq!(output.len(), input.len());
    }
}
