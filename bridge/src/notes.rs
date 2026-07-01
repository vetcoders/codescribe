//! Daily-note surface — thin UniFFI wrapper over the live codescribe
//! `state::notes` store. Backs the tray's "Save last transcript" and "Save
//! selection" actions plus Notes Mode; every input funnels into the one raw
//! append sink (no paste — Notes is a brain-dump destination, not delivery).
//!
//! Sync-only (NOT tokio): every call here is cheap disk I/O or a one-shot
//! Accessibility selection read. Paths honour the same `CODESCRIBE_NOTES_DIR` /
//! `CODESCRIBE_DATA_DIR` overrides the core respects, so Swift always sees
//! on-disk truth.

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

    /// Append `text` to today's daily note and toast "Saved note". The one-shot
    /// save behind the tray's "Save last transcript" action — no paste, because
    /// Notes is a brain-dump destination, not delivery to the cursor.
    pub fn save_text(&self, text: String) -> Result<String, CsError> {
        let path = notes::append_quick_note(&text, Local::now())?;
        notify_saved(&path);
        Ok(path.to_string_lossy().into_owned())
    }

    /// Capture the user's current selection and append it to the daily note.
    /// Prefers the real UI selection via Accessibility; only when there is none
    /// does it fall back to the clipboard — which may be *stale* content, not a
    /// guaranteed live selection. Returns the saved text, or `None` when there
    /// was nothing to capture.
    pub fn save_selection(&self) -> Result<Option<String>, CsError> {
        const MAX_SELECTION_CHARS: usize = 500_000;
        let text = codescribe::os::selection::get_selected_text(MAX_SELECTION_CHARS)
            .filter(|selection| !selection.trim().is_empty())
            .or_else(|| {
                codescribe::os::clipboard::get_clipboard()
                    .ok()
                    .filter(|clip| !clip.trim().is_empty())
            });

        match text {
            Some(text) => {
                let path = notes::append_quick_note(&text, Local::now())?;
                notify_saved(&path);
                Ok(Some(text))
            }
            None => {
                // Don't fail silently: tell the user there was nothing to capture.
                notify_toast("Nothing to save — no selection");
                Ok(None)
            }
        }
    }
}

/// Best-effort "Saved note: <file>" toast (macOS only).
#[cfg(target_os = "macos")]
fn notify_saved(path: &std::path::Path) {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("note");
    notify_toast(&format!("Saved note: {name}"));
}

#[cfg(not(target_os = "macos"))]
fn notify_saved(_path: &std::path::Path) {}

/// Best-effort non-blocking toast (macOS only).
#[cfg(target_os = "macos")]
fn notify_toast(message: &str) {
    codescribe::os::notifications::notify("Codescribe", message);
}

#[cfg(not(target_os = "macos"))]
fn notify_toast(_message: &str) {}
