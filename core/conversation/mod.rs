//! Conversation module - full-duplex voice AI using Moshi.
//!
//! Provides conversational AI capabilities using Kyutai's Moshi model,
//! which supports simultaneous listening and speaking (full-duplex).
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                  CONVERSATION ENGINE                         │
//! ├─────────────────────────────────────────────────────────────┤
//! │                                                              │
//! │   Mic → VAD → Mimi Encoder → Helium LM → Mimi Decoder → Spk │
//! │         ↓                      ↓↑                            │
//! │     "speech?"              Context                           │
//! │                                                              │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Quick Start
//!
//! ```ignore
//! use codescribe_core::conversation::{ConversationEngine, MoshiConfig};
//!
//! // Initialize with default config
//! let mut engine = ConversationEngine::new(MoshiConfig::default())?;
//!
//! // Process incoming audio (user speaking)
//! engine.process_audio(&user_samples)?;
//!
//! // Get model's response audio (if any)
//! if let Some(response) = engine.get_response()? {
//!     play_audio(&response);
//! }
//! ```
//!
//! ## Components
//!
//! - `MoshiConfig` - Configuration for model paths and parameters
//! - `ConversationEngine` - Main engine for full-duplex conversation
//! - `ConversationContext` - Manages conversation history and state
//! - `TurnManager` - Handles turn-taking and interruption
//!
//! Created by M&K (c)2026 VetCoders

pub mod config;
pub mod context;
pub mod engine;
pub mod turns;

pub use config::MoshiConfig;
pub use context::ConversationContext;
pub use engine::ConversationEngine;
pub use turns::TurnManager;

/// Sample rate for Moshi (24kHz like Mimi codec)
pub const SAMPLE_RATE: u32 = 24000;

/// Frame size in samples (for Mimi codec)
pub const FRAME_SAMPLES: usize = 1920; // 80ms at 24kHz

/// Number of audio codebooks (RVQ depth)
pub const NUM_CODEBOOKS: usize = 8;
