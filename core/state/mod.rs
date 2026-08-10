//! State module - conversation tracking and history
//!
//! ## Submodules
//!
//! - `conversation` - Voice Chat session tracking (previous_response_id)
//! - `history` - Transcript history management (~/.codescribe/transcriptions/)

pub mod conversation;
/// Persisted transcription/session history store and query helpers.
pub mod history;
/// User notes and selection-linked note persistence.
pub mod notes;
/// Qube donor state: opt-in recording of WAV + transcript pairs for training.
pub mod qube_donor;

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
pub use qube_donor::{
    QubeDonorPaths, persist_qube_donor_pair, qube_donor_enabled, qube_inbox_base_dir,
    try_persist_qube_donor_at_stop,
};
