//! State module - conversation tracking and history
//!
//! ## Submodules
//!
//! - `conversation` - Voice Chat session tracking (previous_response_id)
//! - `history` - Transcript history management (~/.codescribe/transcriptions/)

pub mod conversation;
pub mod history;
pub mod notes;

// Re-export main types (public API for GUI apps)
pub use conversation::{
    AiMode, get_previous_response_id_for_mode, has_active_conversation, reset_conversation,
    reset_conversation_for_mode, set_response_id_for_mode,
};
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

pub use notes::{
    append_quick_note, notes_dir, open_notes_folder, open_today_note, today_note_path,
};
