//! State module - conversation tracking and history
//!
//! ## Submodules
//!
//! - `conversation` - Voice Chat session tracking (previous_response_id)
//! - `history` - Transcript history management (~/.codescribe/transcriptions/)
//!
//! Created by M&K (c)2026 VetCoders

pub mod conversation;
pub mod history;

// Re-export main types
pub use conversation::{
    get_previous_response_id, has_active_conversation, reset_conversation,
    set_response_id,
};
pub use history::{
    HistoryEntry, clear_history, history_dir, latest_entry, open_audio_logs_folder,
    open_history_folder, recent_entries, save_audio, save_entry, save_entry_with_timestamp,
};
