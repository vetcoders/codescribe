//! Quick notes storage (voice → text notes).
//!
//! This is a lightweight, Vista-friendly primitive:
//! - Append each finalized transcript as a timestamped bullet into a daily Markdown file.
//! - Default location: `~/.codescribe/notes/YYYY-MM-DD.md`
//! - Override location: `CODESCRIBE_NOTES_DIR=/some/path`
//!
//! The host app (Codescribe) decides UX and trigger; this module is just persistence.

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

fn normalize_note_line(text: &str) -> String {
    // Turn any multi-line transcript into a single, readable line.
    // (Doctors dictating quickly tend to produce punctuation and pauses, not intentional newlines.)
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Append a single "quick note" entry to today's Markdown file.
///
/// Format:
/// - `# YYYY-MM-DD` (created once if file is empty/new)
/// - `- HH:MM:SS …`
pub fn append_quick_note(
    transcript_text: &str,
    timestamp: DateTime<Local>,
    frontmost_app: Option<&str>,
) -> Result<PathBuf> {
    let line = normalize_note_line(transcript_text.trim());
    if line.is_empty() {
        anyhow::bail!("Empty note");
    }

    let path = today_note_path(&timestamp);
    let dir = path.parent().map(PathBuf::from).unwrap_or_else(notes_dir);
    fs::create_dir_all(&dir).context("Failed to create notes dir")?;

    let is_new_or_empty = match fs::metadata(&path) {
        Ok(m) => m.len() == 0,
        Err(_) => true,
    };

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("Failed to open note file: {}", path.display()))?;

    if is_new_or_empty {
        writeln!(file, "# {}", timestamp.format("%Y-%m-%d"))?;
        writeln!(file)?;
    }

    let time = timestamp.format("%H:%M:%S");
    let app_suffix = frontmost_app
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!(" ({})", s))
        .unwrap_or_default();
    writeln!(file, "- {}{} {}", time, app_suffix, line)?;

    info!("Quick note appended: {}", path.display());
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
    fn test_append_quick_note_creates_daily_file() {
        let tmp = TempDir::new().expect("tempdir");
        let _guard = EnvGuard::set(
            "CODESCRIBE_NOTES_DIR",
            tmp.path().to_string_lossy().as_ref(),
        );

        let ts = Local::now();
        let path = append_quick_note("Dzień dobry", ts, Some("TestApp")).expect("append");
        assert!(path.exists());
        let body = fs::read_to_string(&path).expect("read");
        assert!(body.contains("# "));
        assert!(body.contains("- "));
        assert!(body.contains("Dzień dobry"));
        assert!(body.contains("(TestApp)"));
    }

    #[test]
    #[serial]
    fn test_append_quick_note_normalizes_multiline() {
        let tmp = TempDir::new().expect("tempdir");
        let _guard = EnvGuard::set(
            "CODESCRIBE_NOTES_DIR",
            tmp.path().to_string_lossy().as_ref(),
        );

        let ts = Local::now();
        let path = append_quick_note("Ala\n\nma kota\n", ts, None).expect("append");
        let body = fs::read_to_string(&path).expect("read");
        assert!(body.contains("Ala ma kota"));
        assert!(!body.contains("Ala\n"));
    }
}
