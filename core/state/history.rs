//! Simple transcript history manager for CodeScribe
//!
//! Saves transcripts and audio to ~/.codescribe/transcriptions/YYYY-MM-DD/
//! Files are paired: HHMMSS_slug_kind.wav + HHMMSS_slug_kind.txt with matching timestamps.

use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use deunicode::deunicode;
use std::collections::HashSet;
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
    Cloud,
    Ai,
    AiFailed,
    Failed,
}

impl TranscriptKind {
    fn suffix(self) -> &'static str {
        match self {
            TranscriptKind::Raw => "raw",
            TranscriptKind::Cloud => "cloud",
            TranscriptKind::Ai => "ai",
            TranscriptKind::AiFailed => "ai-failed",
            TranscriptKind::Failed => "failed",
        }
    }
}

#[derive(Debug, Default)]
pub struct MigrationReport {
    pub renamed_text: usize,
    pub renamed_audio: usize,
    pub skipped: usize,
    pub errors: usize,
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

fn kind_from_suffix(suffix: &str) -> Option<TranscriptKind> {
    match suffix {
        "raw" => Some(TranscriptKind::Raw),
        "cloud" => Some(TranscriptKind::Cloud),
        "ai" => Some(TranscriptKind::Ai),
        "ai-failed" => Some(TranscriptKind::AiFailed),
        "failed" => Some(TranscriptKind::Failed),
        _ => None,
    }
}

fn split_kind_and_index(
    stem: &str,
    default_kind: TranscriptKind,
) -> (TranscriptKind, String, Option<String>) {
    let parts: Vec<&str> = stem.split('_').collect();
    if parts.len() >= 2 {
        let last = parts[parts.len() - 1];
        let second_last = parts[parts.len() - 2];
        let last_is_num = last.chars().all(|c| c.is_ascii_digit());

        if last_is_num && let Some(kind) = kind_from_suffix(second_last) {
            let base = parts[..parts.len() - 2].join("_");
            return (kind, base, Some(last.to_string()));
        }

        if let Some(kind) = kind_from_suffix(last) {
            let base = parts[..parts.len() - 1].join("_");
            return (kind, base, None);
        }
    }

    (default_kind, stem.to_string(), None)
}

fn split_time_and_slug(stem: &str) -> (Option<String>, Option<String>) {
    let stem = stem.trim_start_matches('_');
    if stem.len() >= 6 && stem.chars().take(6).all(|c| c.is_ascii_digit()) {
        let time_base = stem[..6].to_string();
        let rest = stem.get(6..).unwrap_or("").trim_start_matches('_');
        let slug_hint = if rest.is_empty() {
            None
        } else {
            Some(rest.to_string())
        };
        (Some(time_base), slug_hint)
    } else if stem.is_empty() {
        (None, None)
    } else {
        (None, Some(stem.to_string()))
    }
}

fn time_base_from_metadata(path: &Path) -> Option<String> {
    let metadata = fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?;
    let timestamp = DateTime::<Local>::from(modified);
    Some(timestamp.format("%H%M%S").to_string())
}

fn slug_from_hint(hint: &str) -> String {
    let normalized = hint.replace(['_', '-'], " ");
    make_slug(&normalized, 3)
}

fn choose_unique_base(
    dir: &Path,
    base: &str,
    old_txt: Option<&Path>,
    old_wav: Option<&Path>,
    check_txt: bool,
) -> String {
    let mut candidate = base.to_string();
    for i in 0..=10_000 {
        let txt_path = dir.join(format!("{}.txt", candidate));
        let wav_path = dir.join(format!("{}.wav", candidate));
        let txt_conflict = check_txt && txt_path.exists() && (old_txt != Some(txt_path.as_path()));
        let wav_conflict = wav_path.exists() && (old_wav != Some(wav_path.as_path()));

        if !txt_conflict && !wav_conflict {
            return candidate;
        }

        candidate = format!("{}_{}", base, i + 1);
    }

    base.to_string()
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

// ─────────────────────────────────────────────────────────────────────────────
// Voice Drafts - for Mission Control overlay
// ─────────────────────────────────────────────────────────────────────────────

/// Get the drafts directory, creating it if needed
/// Drafts are voice transcriptions saved for later editing/review
pub fn drafts_dir() -> PathBuf {
    let dir = crate::config::Config::config_dir().join("drafts");

    if !dir.exists()
        && let Err(e) = fs::create_dir_all(&dir)
    {
        error!("Failed to create drafts directory: {}", e);
    }

    dir
}

/// Save a voice draft and return the file path
/// Drafts are saved as: ~/.codescribe/drafts/YYYY-MM-DD_HH-MM-SS.txt
pub fn save_draft(text: &str) -> PathBuf {
    let now = Local::now();
    let filename = format!("{}.txt", now.format("%Y-%m-%d_%H-%M-%S"));
    let path = drafts_dir().join(&filename);

    match fs::write(&path, text) {
        Ok(_) => info!("Saved voice draft: {}", path.display()),
        Err(e) => error!("Failed to save voice draft: {}", e),
    }

    path
}

/// List all draft files, sorted by modification time (newest first)
pub fn list_drafts() -> Vec<PathBuf> {
    let dir = drafts_dir();

    let mut entries: Vec<_> = fs::read_dir(&dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "txt")
                .unwrap_or(false)
        })
        .map(|e| e.path())
        .collect();

    // Sort by filename (which contains timestamp) - newest first
    entries.sort_by(|a, b| b.cmp(a));

    entries
}

/// Get draft content by path
pub fn read_draft(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

/// Delete a draft file
pub fn delete_draft(path: &Path) -> bool {
    fs::remove_file(path).is_ok()
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
    save_entry_with_timestamp_and_slug(text, timestamp, kind, None)
}

/// Save a transcript to history with explicit slug source (for consistent pairing)
pub fn save_entry_with_timestamp_and_slug(
    text: &str,
    timestamp: Option<DateTime<Local>>,
    kind: TranscriptKind,
    slug_hint: Option<&str>,
) -> HistoryEntry {
    let text = text.trim();
    let now = timestamp.unwrap_or_else(Local::now);

    // Get transcriptions directory for this date
    let day_dir = transcriptions_dir(&now);

    // Create file with HHMMSS_slug_kind.txt format (slug = first 3 words)
    // Note: multiple writes within the same second can collide (e.g. raw + formatted back-to-back),
    // so we ensure a unique filename by appending an incrementing suffix.
    let time_base = now.format("%H%M%S").to_string();
    let slug_source = slug_hint.unwrap_or(text);
    let slug = make_slug(slug_source, 3);
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

/// Migrate existing transcript/audio filenames to ASCII + suffix naming.
///
/// Notes:
/// - Existing suffixes (_raw/_ai/_ai-failed/_failed) are preserved.
/// - Files without suffix use `assume_kind`.
/// - Slugs are regenerated from transcript text when possible.
/// - Audio files are renamed to match their transcript when paired.
pub fn migrate_transcriptions(
    assume_kind: TranscriptKind,
    dry_run: bool,
) -> Result<MigrationReport> {
    let base_dir = transcriptions_base_dir();
    let mut report = MigrationReport::default();

    if !base_dir.exists() {
        warn!(
            "No transcriptions directory found at {}",
            base_dir.display()
        );
        return Ok(report);
    }

    let day_dirs = fs::read_dir(&base_dir)
        .with_context(|| format!("Failed to read {}", base_dir.display()))?;

    for day_entry in day_dirs {
        let day_entry = match day_entry {
            Ok(entry) => entry,
            Err(e) => {
                warn!("Failed to read entry in {}: {}", base_dir.display(), e);
                report.errors += 1;
                continue;
            }
        };
        let day_path = day_entry.path();
        if !day_path.is_dir() {
            continue;
        }

        let mut txt_files = Vec::new();
        let mut wav_files = Vec::new();
        let entries = match fs::read_dir(&day_path) {
            Ok(entries) => entries,
            Err(e) => {
                warn!("Failed to read {}: {}", day_path.display(), e);
                report.errors += 1;
                continue;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            match path.extension().and_then(|s| s.to_str()) {
                Some("txt") => txt_files.push(path),
                Some("wav") => wav_files.push(path),
                _ => {}
            }
        }

        let mut handled_audio: HashSet<PathBuf> = HashSet::new();

        for txt_path in txt_files {
            let dir = txt_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| day_path.clone());
            let stem = match txt_path.file_stem().and_then(|s| s.to_str()) {
                Some(stem) => stem.to_string(),
                None => {
                    warn!("Skipping non-UTF8 transcript name: {}", txt_path.display());
                    report.skipped += 1;
                    continue;
                }
            };

            let (kind, base_stem, index) = split_kind_and_index(&stem, assume_kind);
            let (time_base_opt, slug_hint) = split_time_and_slug(&base_stem);
            let time_base = time_base_opt
                .or_else(|| time_base_from_metadata(&txt_path))
                .unwrap_or_else(|| Local::now().format("%H%M%S").to_string());

            let text = fs::read_to_string(&txt_path).ok();
            let mut slug = text.as_deref().map(|t| make_slug(t, 3)).unwrap_or_default();
            if slug.is_empty()
                && let Some(hint) = slug_hint.as_deref()
            {
                slug = slug_from_hint(hint);
            }

            let base = build_base_name(&time_base, &slug, kind);
            let base = match index {
                Some(i) if !i.is_empty() => format!("{}_{}", base, i),
                _ => base,
            };

            let old_audio = dir.join(format!("{}.wav", stem));
            let old_audio = if old_audio.exists() {
                Some(old_audio)
            } else {
                None
            };

            let unique_base =
                choose_unique_base(&dir, &base, Some(&txt_path), old_audio.as_deref(), true);
            let new_txt_path = dir.join(format!("{}.txt", unique_base));
            let new_wav_path = dir.join(format!("{}.wav", unique_base));

            if new_txt_path != txt_path {
                info!(
                    "{} transcript: {} -> {}",
                    if dry_run { "Would rename" } else { "Renaming" },
                    txt_path.display(),
                    new_txt_path.display()
                );
                if !dry_run && let Err(e) = fs::rename(&txt_path, &new_txt_path) {
                    warn!("Failed to rename transcript {}: {}", txt_path.display(), e);
                    report.errors += 1;
                    continue;
                }
                report.renamed_text += 1;
            } else {
                report.skipped += 1;
            }

            if let Some(old_audio_path) = old_audio {
                if new_wav_path != old_audio_path {
                    info!(
                        "{} audio: {} -> {}",
                        if dry_run { "Would rename" } else { "Renaming" },
                        old_audio_path.display(),
                        new_wav_path.display()
                    );
                    if !dry_run {
                        if let Err(e) = fs::rename(&old_audio_path, &new_wav_path) {
                            warn!("Failed to rename audio {}: {}", old_audio_path.display(), e);
                            report.errors += 1;
                        } else {
                            report.renamed_audio += 1;
                        }
                    } else {
                        report.renamed_audio += 1;
                    }
                } else {
                    report.skipped += 1;
                }
                handled_audio.insert(new_wav_path);
            }
        }

        for wav_path in wav_files {
            if handled_audio.contains(&wav_path) {
                continue;
            }

            let dir = wav_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| day_path.clone());
            let stem = match wav_path.file_stem().and_then(|s| s.to_str()) {
                Some(stem) => stem.to_string(),
                None => {
                    warn!("Skipping non-UTF8 audio name: {}", wav_path.display());
                    report.skipped += 1;
                    continue;
                }
            };

            let (kind, base_stem, index) = split_kind_and_index(&stem, assume_kind);
            let (time_base_opt, slug_hint) = split_time_and_slug(&base_stem);
            let time_base = time_base_opt
                .or_else(|| time_base_from_metadata(&wav_path))
                .unwrap_or_else(|| Local::now().format("%H%M%S").to_string());

            let slug = slug_hint.as_deref().map(slug_from_hint).unwrap_or_default();
            let base = build_base_name(&time_base, &slug, kind);
            let base = match index {
                Some(i) if !i.is_empty() => format!("{}_{}", base, i),
                _ => base,
            };

            let unique_base = choose_unique_base(&dir, &base, None, Some(&wav_path), false);
            let new_wav_path = dir.join(format!("{}.wav", unique_base));

            if new_wav_path != wav_path {
                info!(
                    "{} orphan audio: {} -> {}",
                    if dry_run { "Would rename" } else { "Renaming" },
                    wav_path.display(),
                    new_wav_path.display()
                );
                if !dry_run {
                    if let Err(e) = fs::rename(&wav_path, &new_wav_path) {
                        warn!("Failed to rename audio {}: {}", wav_path.display(), e);
                        report.errors += 1;
                    } else {
                        report.renamed_audio += 1;
                    }
                } else {
                    report.renamed_audio += 1;
                }
            } else {
                report.skipped += 1;
            }
        }
    }

    Ok(report)
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
        // Canonicalize to handle macOS /var → /private/var symlink
        let tmp_canon = tmp
            .path()
            .canonicalize()
            .unwrap_or_else(|_| tmp.path().to_path_buf());

        let dir = transcriptions_dir(&Local::now());
        assert!(dir.to_string_lossy().contains("transcriptions"));
        assert!(dir.starts_with(&tmp_canon));
    }

    #[test]
    #[serial]
    fn test_save_and_retrieve() {
        let tmp = TempDir::new().expect("tempdir");
        let _guard = EnvGuard::set_to_temp_dir("CODESCRIBE_DATA_DIR", &tmp);
        // Canonicalize to handle macOS /var → /private/var symlink
        let tmp_canon = tmp
            .path()
            .canonicalize()
            .unwrap_or_else(|_| tmp.path().to_path_buf());

        let text = "Test transcript content";
        let entry = save_entry(text);

        assert!(entry.path.exists());
        assert_eq!(entry.preview, text);
        assert!(entry.path.to_string_lossy().ends_with(".txt"));
        assert!(entry.path.starts_with(&tmp_canon));

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

    #[test]
    #[serial]
    fn test_save_entry_with_slug_hint_consistency() {
        let tmp = TempDir::new().expect("tempdir");
        let _guard = EnvGuard::set_to_temp_dir("CODESCRIBE_DATA_DIR", &tmp);

        let now = Local::now();
        let raw = save_entry_with_timestamp_and_slug(
            "raw content",
            Some(now),
            TranscriptKind::Raw,
            Some("shared slug source"),
        );
        let ai = save_entry_with_timestamp_and_slug(
            "ai content",
            Some(now),
            TranscriptKind::Ai,
            Some("shared slug source"),
        );

        let raw_stem = raw.path.file_stem().unwrap().to_string_lossy();
        let ai_stem = ai.path.file_stem().unwrap().to_string_lossy();
        let raw_base = raw_stem.strip_suffix("_raw").unwrap_or(&raw_stem);
        let ai_base = ai_stem.strip_suffix("_ai").unwrap_or(&ai_stem);

        assert_eq!(raw_base, ai_base, "Slug hint should align base name");
    }
}
