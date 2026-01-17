//! Simple transcript history manager for CodeScribe
//!
//! Saves transcripts and audio to ~/.codescribe/transcriptions/YYYY-MM-DD/
//! Files are paired: HHMMSS_slug_kind.wav + HHMMSS_slug_kind.txt with matching timestamps.

use chrono::{DateTime, Local};
use deunicode::deunicode;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, error, info, warn};

/// A single history entry
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub path: PathBuf,
    pub timestamp: DateTime<Local>,
    pub preview: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptKind {
    Raw,
    Ai,
    AiFailed,
    Failed,
}

impl TranscriptKind {
    fn suffix(self) -> &'static str {
        match self {
            TranscriptKind::Raw => "raw",
            TranscriptKind::Ai => "ai",
            TranscriptKind::AiFailed => "ai-failed",
            TranscriptKind::Failed => "failed",
        }
    }
}

impl HistoryEntry {
    /// Get a formatted label for display in menus
    pub fn label(&self) -> String {
        let ts = self.timestamp.format("%H:%M:%S").to_string();
        if self.preview.is_empty() {
            ts
        } else {
            format!("{} – {}", ts, self.preview)
        }
    }
}

/// Create a filename-safe slug from the first N words of text
/// Returns empty string if no valid words found
fn make_slug(text: &str, max_words: usize) -> String {
    let ascii = deunicode(text);
    let slug: String = ascii
        .split_whitespace()
        .take(max_words)
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect::<String>()
        .to_lowercase();

    // Limit length to avoid filesystem issues
    if slug.len() > 30 {
        slug.chars().take(30).collect()
    } else {
        slug
    }
}

fn build_base_name(time_base: &str, slug: &str, kind: TranscriptKind) -> String {
    if slug.is_empty() {
        format!("{}_{}", time_base, kind.suffix())
    } else {
        format!("{}_{}_{}", time_base, slug, kind.suffix())
    }
}

/// Get the transcriptions base directory
fn transcriptions_base_dir() -> PathBuf {
    // Use config_dir as the single source of truth for filesystem roots.
    // This keeps behavior identical in normal runs (defaults to $HOME/.codescribe)
    // while allowing deterministic overrides in tests via CODESCRIBE_DATA_DIR.
    crate::config::Config::config_dir().join("transcriptions")
}

/// Get the transcriptions directory for a specific date, creating it if needed
pub fn transcriptions_dir(date: &DateTime<Local>) -> PathBuf {
    let base = transcriptions_base_dir();
    let date_folder = date.format("%Y-%m-%d").to_string();
    let dir = base.join(date_folder);

    if !dir.exists()
        && let Err(e) = fs::create_dir_all(&dir)
    {
        error!("Failed to create transcriptions directory: {}", e);
    }

    dir
}

/// Get the history directory, creating it if needed
/// Note: Now an alias for transcriptions_dir with current date for backwards compatibility
pub fn history_dir() -> PathBuf {
    transcriptions_dir(&Local::now())
}

/// Save a transcript to history and return the entry
///
/// # Arguments
/// * `text` - The transcript text to save
/// * `timestamp` - Optional timestamp to use (for pairing with audio files).
///   If None, uses current time.
/// * `kind` - What kind of transcript this is (raw/ai/ai-failed)
pub fn save_entry_with_timestamp(
    text: &str,
    timestamp: Option<DateTime<Local>>,
    kind: TranscriptKind,
) -> HistoryEntry {
    let text = text.trim();
    let now = timestamp.unwrap_or_else(Local::now);

    // Get transcriptions directory for this date
    let day_dir = transcriptions_dir(&now);

    // Create file with HHMMSS_slug_kind.txt format (slug = first 3 words)
    // Note: multiple writes within the same second can collide (e.g. raw + formatted back-to-back),
    // so we ensure a unique filename by appending an incrementing suffix.
    let time_base = now.format("%H%M%S").to_string();
    let slug = make_slug(text, 3);
    let base = build_base_name(&time_base, &slug, kind);
    let mut path = day_dir.join(format!("{}.txt", base));
    if path.exists() {
        for i in 1..=10_000 {
            let candidate = day_dir.join(format!("{}_{}.txt", base, i));
            if !candidate.exists() {
                path = candidate;
                break;
            }
        }
    }

    match fs::File::create(&path) {
        Ok(mut file) => {
            if let Err(e) = file.write_all(text.as_bytes()) {
                error!("Failed to write transcript '{}': {}", path.display(), e);
            } else {
                debug!("Saved transcript: {}", path.display());
            }
        }
        Err(e) => {
            error!(
                "Failed to create transcript file '{}': {}",
                path.display(),
                e
            );
        }
    }

    // Extract preview (first line, max 60 chars)
    let preview = text.lines().next().unwrap_or("").chars().take(60).collect();

    HistoryEntry {
        path,
        timestamp: now,
        preview,
    }
}

/// Save a transcript to history and return the entry (convenience wrapper)
pub fn save_entry_with_kind(text: &str, kind: TranscriptKind) -> HistoryEntry {
    save_entry_with_timestamp(text, None, kind)
}

/// Save a transcript to history and return the entry (convenience wrapper)
pub fn save_entry(text: &str) -> HistoryEntry {
    save_entry_with_timestamp(text, None, TranscriptKind::Raw)
}

/// Get recent history entries, sorted by modification time (newest first)
pub fn recent_entries(limit: usize) -> Vec<HistoryEntry> {
    let base_dir = transcriptions_base_dir();
    let mut entries = Vec::new();
    let mut files: Vec<PathBuf> = Vec::new();

    // Collect all .txt files from date subdirectories
    if let Ok(day_dirs) = fs::read_dir(&base_dir) {
        for day_entry in day_dirs.flatten() {
            if day_entry.path().is_dir()
                && let Ok(txt_files) = fs::read_dir(day_entry.path())
            {
                for txt_entry in txt_files.flatten() {
                    let path = txt_entry.path();
                    if path.extension().is_some_and(|ext| ext == "txt") {
                        files.push(path);
                    }
                }
            }
        }
    }

    // Sort by modification time (newest first)
    files.sort_by(|a, b| {
        let a_time = fs::metadata(a).and_then(|m| m.modified()).ok();
        let b_time = fs::metadata(b).and_then(|m| m.modified()).ok();
        b_time.cmp(&a_time)
    });

    // Take the requested limit and create entries
    for path in files.into_iter().take(limit) {
        let timestamp = fs::metadata(&path)
            .and_then(|m| m.modified())
            .map(DateTime::<Local>::from)
            .unwrap_or_else(|_| Local::now());

        let preview = fs::read_to_string(&path)
            .unwrap_or_default()
            .trim()
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(60)
            .collect();

        entries.push(HistoryEntry {
            path,
            timestamp,
            preview,
        });
    }

    entries
}

/// Get the latest history entry, if any
pub fn latest_entry() -> Option<HistoryEntry> {
    recent_entries(1).into_iter().next()
}

/// Open the transcriptions folder in Finder
pub fn open_history_folder() {
    let dir = transcriptions_base_dir();
    if let Err(e) = Command::new("open").arg(&dir).spawn() {
        error!("Failed to open transcriptions folder: {}", e);
    }
}

/// Save audio file to transcriptions folder with the given timestamp and optional slug
///
/// Creates a paired file alongside the transcript (e.g., 143052_czesc-jak_raw.wav pairs with 143052_czesc-jak_raw.txt)
///
/// # Arguments
/// * `src_path` - Path to the source WAV file (typically a temp file)
/// * `timestamp` - Timestamp to use for the filename (should match the transcript)
/// * `transcript_text` - Optional transcript text to generate slug from (first 3 words)
/// * `kind` - What kind of transcript this is (raw/ai/ai-failed)
///
/// # Returns
/// * `Some(PathBuf)` - Path to the saved audio file on success
/// * `None` - If src_path doesn't exist or copy failed
pub fn save_audio(
    src_path: &Path,
    timestamp: DateTime<Local>,
    transcript_text: Option<&str>,
    kind: TranscriptKind,
) -> Option<PathBuf> {
    if !src_path.exists() {
        warn!("save_audio: source file does not exist: {:?}", src_path);
        return None;
    }

    // Get transcriptions directory for this date
    let dest_dir = transcriptions_dir(&timestamp);

    // Create filename with HHMMSS_slug_kind.wav format (matching transcript naming)
    let time_base = timestamp.format("%H%M%S").to_string();
    let slug = transcript_text.map(|t| make_slug(t, 3)).unwrap_or_default();
    let base = build_base_name(&time_base, &slug, kind);
    let mut dest_path = dest_dir.join(format!("{}.wav", base));
    if dest_path.exists() {
        for i in 1..=10_000 {
            let candidate = dest_dir.join(format!("{}_{}.wav", base, i));
            if !candidate.exists() {
                dest_path = candidate;
                break;
            }
        }
    }

    match fs::copy(src_path, &dest_path) {
        Ok(_) => {
            info!("Audio saved: {}", dest_path.display());
            Some(dest_path)
        }
        Err(e) => {
            error!("Failed to save audio to {}: {}", dest_path.display(), e);
            None
        }
    }
}

/// Legacy function for backwards compatibility - saves audio with current timestamp
///
/// Prefer using save_audio() with explicit timestamp for proper pairing with transcripts
#[deprecated(note = "Use save_audio() with explicit timestamp instead")]
pub fn dump_audio(src_path: &Path, _reason: &str) -> Option<PathBuf> {
    save_audio(src_path, Local::now(), None, TranscriptKind::Raw)
}

/// Open the transcriptions folder in Finder (alias for open_history_folder)
pub fn open_audio_logs_folder() {
    open_history_folder();
}

/// Clear all history entries
pub fn clear_history() {
    let dir = history_dir();
    if let Ok(day_dirs) = fs::read_dir(&dir) {
        for day_entry in day_dirs.flatten() {
            if day_entry.path().is_dir()
                && let Ok(txt_files) = fs::read_dir(day_entry.path())
            {
                for txt_entry in txt_files.flatten() {
                    let path = txt_entry.path();
                    if path.extension().is_some_and(|ext| ext == "txt")
                        && let Err(e) = fs::remove_file(&path)
                    {
                        warn!("Failed to delete history entry '{}': {}", path.display(), e);
                    }
                }
            }
        }
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
        fn set_to_temp_dir(key: &'static str, dir: &TempDir) -> Self {
            let prev = std::env::var(key).ok();
            unsafe {
                std::env::set_var(key, dir.path());
            }
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
    fn test_transcriptions_dir() {
        let tmp = TempDir::new().expect("tempdir");
        let _guard = EnvGuard::set_to_temp_dir("CODESCRIBE_DATA_DIR", &tmp);

        let dir = transcriptions_dir(&Local::now());
        assert!(dir.to_string_lossy().contains("transcriptions"));
        assert!(dir.starts_with(tmp.path()));
    }

    #[test]
    #[serial]
    fn test_save_and_retrieve() {
        let tmp = TempDir::new().expect("tempdir");
        let _guard = EnvGuard::set_to_temp_dir("CODESCRIBE_DATA_DIR", &tmp);

        let text = "Test transcript content";
        let entry = save_entry(text);

        assert!(entry.path.exists());
        assert_eq!(entry.preview, text);
        assert!(entry.path.to_string_lossy().ends_with(".txt"));
        assert!(entry.path.starts_with(tmp.path()));

        // Clean up
        let _ = fs::remove_file(&entry.path);
    }

    #[test]
    #[serial]
    fn test_save_entry_with_timestamp() {
        let tmp = TempDir::new().expect("tempdir");
        let _guard = EnvGuard::set_to_temp_dir("CODESCRIBE_DATA_DIR", &tmp);

        let text = "Timestamped transcript";
        let now = Local::now();
        let entry = save_entry_with_timestamp(text, Some(now), TranscriptKind::Raw);

        assert!(entry.path.exists());
        assert_eq!(
            entry.timestamp.format("%H%M%S").to_string(),
            now.format("%H%M%S").to_string()
        );

        // Clean up
        let _ = fs::remove_file(&entry.path);
    }

    #[test]
    #[serial]
    fn test_entry_label() {
        let entry = HistoryEntry {
            path: PathBuf::from("/tmp/test.txt"),
            timestamp: Local::now(),
            preview: "Hello world".to_string(),
        };

        let label = entry.label();
        assert!(label.contains("Hello world"));
    }
}
