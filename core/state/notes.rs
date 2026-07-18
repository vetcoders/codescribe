//! Daily-note storage — the single "brain dump buffer" sink.
//!
//! One daily Markdown file, append-only, intentionally structureless:
//! - Append text as-is (edge-trimmed; internal newlines preserved), with one
//!   blank line between entries. No header, no timestamp, no bullets — the
//!   file's date lives in its name (`YYYY-MM-DD.md`).
//! - Default location: `~/.codescribe/notes/YYYY-MM-DD.md`
//! - Override location: `CODESCRIBE_NOTES_DIR=/some/path`
//!
//! Every input (Notes Mode voice, Save last transcript, Save selection) funnels
//! through `append_quick_note`; the host app decides UX and trigger.

use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use tracing::{error, info};

fn notes_base_dir() -> PathBuf {
    if let Ok(custom) = std::env::var("CODESCRIBE_NOTES_DIR") {
        return PathBuf::from(shellexpand::tilde(&custom).into_owned());
    }
    crate::config::Config::config_dir().join("notes")
}

/// Get the notes directory, creating it if needed.
pub fn notes_dir() -> PathBuf {
    let dir = notes_base_dir();
    if !dir.exists()
        && let Err(e) = fs::create_dir_all(&dir)
    {
        error!("Failed to create notes directory: {}", e);
    }
    dir
}

/// Daily Markdown file path for the given timestamp (local time).
pub fn today_note_path(timestamp: &DateTime<Local>) -> PathBuf {
    notes_dir().join(format!("{}.md", timestamp.format("%Y-%m-%d")))
}

/// Append one entry to today's daily note — the single brain-dump sink.
///
/// Raw and structureless by design: the text is written as-is (only edge-trimmed
/// so internal newlines survive — multi-line selections must be preserved), then
/// a blank line separates it from the next entry. No header, no timestamp, no
/// bullets. `timestamp` only picks the dated file name.
pub fn append_quick_note(text: &str, timestamp: DateTime<Local>) -> Result<PathBuf> {
    let entry = text.trim();
    if entry.is_empty() {
        anyhow::bail!("Empty note");
    }

    let path = today_note_path(&timestamp);
    let dir = path.parent().map(PathBuf::from).unwrap_or_else(notes_dir);
    fs::create_dir_all(&dir).context("Failed to create notes dir")?;

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("Failed to open note file: {}", path.display()))?;

    // Entry as-is, then a single blank line as the only separator.
    writeln!(file, "{entry}\n")?;

    info!("Note appended: {}", path.display());
    Ok(path)
}

/// Open the notes folder in the OS file manager (best-effort).
pub fn open_notes_folder() {
    let dir = notes_dir();
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("open").arg(&dir).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = Command::new("xdg-open").arg(&dir).spawn();
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        tracing::warn!("open_notes_folder: unsupported OS");
    }
}

/// Open today's note in a text editor (best-effort).
pub fn open_today_note() {
    let now = Local::now();
    let path = today_note_path(&now);

    #[cfg(target_os = "macos")]
    {
        // `-t` opens in the default text editor.
        let _ = Command::new("open").arg("-t").arg(&path).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = Command::new("xdg-open").arg(&path).spawn();
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        tracing::warn!("open_today_note: unsupported OS");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use tempfile::TempDir;

    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            unsafe { std::env::set_var(key, value) };
            Self { key, prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.prev {
                    Some(v) => std::env::set_var(self.key, v),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    #[test]
    #[serial]
    fn test_append_quick_note_writes_raw_entry() {
        let tmp = TempDir::new().expect("tempdir");
        let _guard = EnvGuard::set(
            "CODESCRIBE_NOTES_DIR",
            tmp.path().to_string_lossy().as_ref(),
        );

        let ts = Local::now();
        let path = append_quick_note("Dzień dobry", ts).expect("append");
        assert!(path.exists());
        let body = fs::read_to_string(&path).expect("read");
        // Raw: text present, and NONE of the old scaffolding (header/bullet).
        assert!(body.contains("Dzień dobry"));
        assert!(!body.contains("# "), "no date header");
        assert!(!body.contains("- "), "no bullet prefix");
        // Blank-line separator terminates the entry.
        assert!(body.ends_with("\n\n"));
    }

    #[test]
    #[serial]
    fn test_append_quick_note_preserves_internal_newlines() {
        let tmp = TempDir::new().expect("tempdir");
        let _guard = EnvGuard::set(
            "CODESCRIBE_NOTES_DIR",
            tmp.path().to_string_lossy().as_ref(),
        );

        let ts = Local::now();
        // A multi-line selection (e.g. an agent reply) must survive verbatim.
        let path = append_quick_note("Ala\nma kota", ts).expect("append");
        let body = fs::read_to_string(&path).expect("read");
        assert!(body.contains("Ala\nma kota"));
    }

    #[test]
    #[serial]
    fn test_append_quick_note_blank_line_between_entries() {
        let tmp = TempDir::new().expect("tempdir");
        let _guard = EnvGuard::set(
            "CODESCRIBE_NOTES_DIR",
            tmp.path().to_string_lossy().as_ref(),
        );

        let ts = Local::now();
        append_quick_note("first", ts).expect("append 1");
        let path = append_quick_note("second", ts).expect("append 2");
        let body = fs::read_to_string(&path).expect("read");
        assert_eq!(body, "first\n\nsecond\n\n");
    }
}
