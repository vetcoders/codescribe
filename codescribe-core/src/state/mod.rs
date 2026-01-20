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

// Re-export main types (public API for GUI apps)
#[allow(unused_imports)] // Public API for external consumers
pub use conversation::{
    AiMode, get_previous_response_id_for_mode, has_active_conversation, reset_conversation,
    reset_conversation_for_mode, set_response_id_for_mode,
};
#[allow(unused_imports)] // Public API for external consumers
pub use history::{
    HistoryEntry,
    TranscriptKind,
    // Voice Drafts API (Mission Control)
    delete_draft,
    drafts_dir,
    latest_entry,
    list_drafts,
    open_history_folder,
    read_draft,
    recent_entries,
    save_audio,
    save_draft,
    save_entry,
    save_entry_with_kind,
    save_entry_with_timestamp,
};
