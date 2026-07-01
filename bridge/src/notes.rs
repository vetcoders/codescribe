//! Quick-notes surface — thin UniFFI wrapper over the live codescribe
//! `state::notes` daily-note store plus the shared clipboard paste path used by
//! the tray's "Quick notes" actions.
//!
//! Sync-only (NOT tokio): every call here is cheap disk I/O or a one-shot
//! CGEvent paste. Paths honour the same `CODESCRIBE_NOTES_DIR` / `CODESCRIBE_DATA_DIR`
//! overrides the core respects, so Swift always sees on-disk truth.

use chrono::Local;

use codescribe_core::state::notes;

use crate::CsError;

/// Thin handle to the codescribe daily-notes store. Stateless: each call reads
/// or writes through the live on-disk notes dir.
#[derive(uniffi::Object)]
pub struct CodescribeNotes {}

#[uniffi::export]
impl CodescribeNotes {
    #[uniffi::constructor]
    pub fn new() -> Self {
        Self {}
    }

    /// Absolute path to the daily-notes directory (honours `CODESCRIBE_NOTES_DIR`),
    /// creating it if needed. Wraps `state::notes::notes_dir`.
    pub fn notes_dir(&self) -> String {
        notes::notes_dir().to_string_lossy().into_owned()
    }

    /// Absolute path to today's Markdown note file (may not exist yet). Wraps
    /// `state::notes::today_note_path`.
    pub fn today_note_path(&self) -> String {
        notes::today_note_path(&Local::now())
            .to_string_lossy()
            .into_owned()
    }

    /// Append `text` as a raw entry to today's daily note, returning the note
    /// file path. Wraps `state::notes::append_quick_note` (errors on empty text).
    pub fn append_quick_note(&self, text: String) -> Result<String, CsError> {
        let path = notes::append_quick_note(&text, Local::now())?;
        Ok(path.to_string_lossy().into_owned())
    }

    /// Paste `text` into the frontmost app via the shared clipboard path, which
    /// also respects the restore-clipboard setting. Wraps
    /// `codescribe::clipboard::paste_text` (the same delivery dictation uses).
    pub fn paste_text(&self, text: String) -> Result<(), CsError> {
        codescribe::clipboard::paste_text(&text)?;
        Ok(())
    }
}
