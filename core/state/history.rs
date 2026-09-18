//! Simple transcript history manager for Codescribe
//!
//! Saves transcripts and audio to ~/.codescribe/transcriptions/YYYY-MM-DD/
//! Files are paired: HHMMSS_slug_kind.m4a + HHMMSS_slug_kind.txt with matching timestamps.

use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use deunicode::deunicode;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{error, info, warn};

use crate::pipeline::take_truth::{TakeTruth, write_truth_sidecar};

/// Audio containers an archived recording may use: `m4a` normally, `wav` when
/// encoding failed and the raw copy was kept as a fallback.
const AUDIO_ARCHIVE_EXTENSIONS: &[&str] = &["m4a", "wav"];
const NO_SPEECH_HISTORY_TITLE: &str = "(no speech)";

/// A single history entry
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    /// Path of the `.txt` transcript on disk.
    pub path: PathBuf,
    /// File modification time, used for ordering.
    pub timestamp: DateTime<Local>,
    /// First line, truncated to 60 characters, for menu display.
    pub preview: String,
    /// Artifact kind recovered from the filename suffix.
    pub kind: TranscriptKind,
}

/// Which artifact of a dictation a file holds.
///
/// One recording can produce several of these, and the kind is encoded in the
/// filename suffix — it is what lets history tell a raw draft apart from its
/// formatted final, and a real transcript apart from a failure marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptKind {
    /// Local engine output, before any formatting pass.
    Raw,
    /// Cloud transcription output.
    Cloud,
    /// Transcript after a successful LLM formatting pass.
    FormattedTranscript,
    /// Assistant answer to a spoken instruction, not a transcript of it.
    AssistantInterpretation,
    /// Formatting failed; the text is the unformatted fallback.
    FormattingFailed,
    /// The dictation produced no usable transcript.
    Failed,
}

/// Transcript outcome accepted by the session archive boundary.
///
/// Diagnostics deliberately have their own variant, so a lane or transport
/// error cannot be confused with committed user speech and written into the
/// transcript document. `NoSpeech` is the only non-transcript outcome that
/// creates a visible history row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionTranscriptArchive<'a> {
    /// Reducer-committed user speech.
    Committed(&'a str),
    /// A successful take with no committed words.
    NoSpeech,
    /// A lane, transport, or seal failure. The diagnostic is never persisted
    /// as transcript text; its owner reports it through status/log surfaces.
    Unavailable(&'a str),
}

impl<'a> SessionTranscriptArchive<'a> {
    /// Classify a reducer render without letting an empty string masquerade as
    /// committed speech.
    pub fn from_committed(text: &'a str) -> Self {
        if text.trim().is_empty() {
            Self::NoSpeech
        } else {
            Self::Committed(text)
        }
    }
}

impl TranscriptKind {
    /// Filename suffix for this kind. Writes emit these; [`kind_from_suffix`]
    /// reads them back.
    fn suffix(self) -> &'static str {
        match self {
            TranscriptKind::Raw => "raw",
            TranscriptKind::Cloud => "cloud",
            TranscriptKind::FormattedTranscript => "formatted",
            TranscriptKind::AssistantInterpretation => "interpretation",
            TranscriptKind::FormattingFailed => "formatting-failed",
            TranscriptKind::Failed => "failed",
        }
    }

    /// Whether this artifact is real transcript text a user would want pasted.
    ///
    /// Excludes failure markers and assistant answers, so "copy last transcript"
    /// never hands back "no speech detected" or a chat reply.
    pub fn is_copyable_transcript(self) -> bool {
        matches!(
            self,
            TranscriptKind::Raw
                | TranscriptKind::Cloud
                | TranscriptKind::FormattedTranscript
                | TranscriptKind::FormattingFailed
        )
    }
}

/// Derive product-facing history text from artifact class. Failure payloads
/// are diagnostics, including legacy rows that may still contain an error
/// string; their only permitted title is the explicit no-speech sentinel.
fn history_preview(kind: TranscriptKind, text: &str) -> String {
    if kind == TranscriptKind::Failed {
        return NO_SPEECH_HISTORY_TITLE.to_string();
    }
    text.trim()
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(60)
        .collect()
}

/// Tally of one [`migrate_transcriptions`] pass, dry-run or applied.
#[derive(Debug, Default)]
pub struct MigrationReport {
    /// Transcripts renamed to the canonical scheme.
    pub renamed_text: usize,
    /// Audio files renamed, including orphans with no transcript.
    pub renamed_audio: usize,
    /// Files already canonical, or skipped as unreadable.
    pub skipped: usize,
    /// Failures encountered; migration continues past them.
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

/// Assemble the canonical stem `HHMMSS[_slug]_kind`.
///
/// The slug is omitted entirely when empty, rather than leaving a double
/// underscore that the parser would have to special-case.
fn build_base_name(time_base: &str, slug: &str, kind: TranscriptKind) -> String {
    if slug.is_empty() {
        format!("{}_{}", time_base, kind.suffix())
    } else {
        format!("{}_{}_{}", time_base, slug, kind.suffix())
    }
}

/// Recover a kind from a filename suffix, `None` if the suffix is not one.
///
/// Accepts the retired `ai` / `ai-failed` spellings so files written before the
/// rename still classify correctly instead of silently reading as `Raw`.
fn kind_from_suffix(suffix: &str) -> Option<TranscriptKind> {
    match suffix {
        "raw" => Some(TranscriptKind::Raw),
        "cloud" => Some(TranscriptKind::Cloud),
        "ai" | "formatted" => Some(TranscriptKind::FormattedTranscript),
        "ai-failed" | "formatting-failed" => Some(TranscriptKind::FormattingFailed),
        "interpretation" => Some(TranscriptKind::AssistantInterpretation),
        "failed" => Some(TranscriptKind::Failed),
        _ => None,
    }
}

/// Split a stem into `(kind, base, collision index)`.
///
/// Handles both `..._raw` and `..._raw_1`, the latter produced when two saves
/// land in the same second. A stem with no recognizable suffix keeps
/// `default_kind` and is returned whole as the base.
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

/// Split a kind-stripped stem into its `HHMMSS` prefix and slug remainder.
///
/// Only a leading run of exactly six digits counts as a time base; anything
/// else is treated as slug text, leaving the caller to source a timestamp from
/// file metadata instead of misreading arbitrary digits as a clock.
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

/// Derive an `HHMMSS` base from the file's mtime, for names that carry no time.
fn time_base_from_metadata(path: &Path) -> Option<String> {
    let metadata = fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?;
    let timestamp = DateTime::<Local>::from(modified);
    Some(timestamp.format("%H%M%S").to_string())
}

/// Rebuild a slug from an existing filename fragment.
///
/// Separators become spaces first, so an old `czesc-jak-sie` fragment re-slugs
/// to the same three words [`make_slug`] would produce from transcript text.
fn slug_from_hint(hint: &str) -> String {
    let normalized = hint.replace(['_', '-'], " ");
    make_slug(&normalized, 3)
}

/// Find a stem that collides with no existing transcript or audio file.
///
/// The file being renamed is passed as `old_txt` / `old_audio` and excluded
/// from the check, so a file never counts as a conflict with itself. Appends
/// `_1`, `_2`, … and gives up after 10 000 attempts, returning `base` unchanged.
fn choose_unique_base(
    dir: &Path,
    base: &str,
    old_txt: Option<&Path>,
    old_audio: Option<&Path>,
    check_txt: bool,
) -> String {
    let mut candidate = base.to_string();
    for i in 0..=10_000 {
        let txt_path = dir.join(format!("{}.txt", candidate));
        let txt_conflict = check_txt && txt_path.exists() && (old_txt != Some(txt_path.as_path()));
        let audio_conflict = AUDIO_ARCHIVE_EXTENSIONS.iter().any(|ext| {
            let audio_path = dir.join(format!("{}.{}", candidate, ext));
            audio_path.exists() && (old_audio != Some(audio_path.as_path()))
        });

        if !txt_conflict && !audio_conflict {
            return candidate;
        }

        candidate = format!("{}_{}", base, i + 1);
    }

    base.to_string()
}

/// Locate the audio file paired with a transcript stem, in either container.
fn existing_audio_for_stem(dir: &Path, stem: &str) -> Option<PathBuf> {
    AUDIO_ARCHIVE_EXTENSIONS
        .iter()
        .map(|ext| dir.join(format!("{stem}.{ext}")))
        .find(|path| path.exists())
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
/// * `kind` - What kind of transcript artifact this is
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

    let base = build_base_name(
        &now.format("%H%M%S").to_string(),
        &make_slug(slug_hint.unwrap_or(text), 3),
        kind,
    );
    let intended = transcriptions_base_dir()
        .join(now.format("%Y-%m-%d").to_string())
        .join(format!("{base}.txt"));
    let path =
        match daily_archive::save_text(&crate::config::Config::config_dir(), &now, &base, text) {
            Ok(path) => path,
            Err(error) => {
                error!(
                    "Failed to save transcript {}: {error:#}",
                    intended.display()
                );
                // Legacy return type cannot express failure; no file is invented.
                intended
            }
        };

    let preview = history_preview(kind, text);

    HistoryEntry {
        path,
        timestamp: now,
        preview,
        kind,
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

    // Collapse same-save families: a raw draft and its post-processed final can
    // land within the same second as `base.txt` + `base_1.txt` (see the collision
    // suffix in save_entry_with_timestamp_and_slug). History surfaces one row per
    // dictation — newest file of each (dir, base, kind) family wins.
    let mut seen_families: HashSet<(PathBuf, String, &'static str)> = HashSet::new();
    let files: Vec<PathBuf> = files
        .into_iter()
        .filter(|path| {
            let stem = path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            let (kind, base, _) = split_kind_and_index(stem, TranscriptKind::Raw);
            let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
            // Distinct paired takes must remain discoverable even in one second.
            let family = if existing_audio_for_stem(&dir, stem).is_some() {
                stem.to_string()
            } else {
                base
            };
            seen_families.insert((dir, family, kind.suffix()))
        })
        .collect();

    // Take the requested limit and create entries
    for path in files.into_iter().take(limit) {
        let timestamp = fs::metadata(&path)
            .and_then(|m| m.modified())
            .map(DateTime::<Local>::from)
            .unwrap_or_else(|_| Local::now());
        let stem = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        let (kind, _, _) = split_kind_and_index(stem, TranscriptKind::Raw);

        let preview = history_preview(kind, &fs::read_to_string(&path).unwrap_or_default());

        entries.push(HistoryEntry {
            path,
            timestamp,
            preview,
            kind,
        });
    }

    entries
}

/// Get the latest history entry, if any
pub fn latest_entry() -> Option<HistoryEntry> {
    recent_entries(1).into_iter().next()
}

/// Get the latest entry that is still a transcript artifact suitable for copy/paste.
///
/// This skips explicit failure markers and non-transcript categories so tray
/// actions do not present "last transcript" as a failed/no-speech artifact.
pub fn latest_copyable_entry() -> Option<HistoryEntry> {
    recent_entries(256)
        .into_iter()
        .find(|entry| entry.kind.is_copyable_transcript())
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
        let mut audio_files = Vec::new();
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
                Some("wav" | "m4a") => audio_files.push(path),
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

            let old_audio = existing_audio_for_stem(&dir, &stem);

            let unique_base =
                choose_unique_base(&dir, &base, Some(&txt_path), old_audio.as_deref(), true);
            let new_txt_path = dir.join(format!("{}.txt", unique_base));

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
                let audio_ext = old_audio_path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .unwrap_or("wav");
                let new_audio_path = dir.join(format!("{}.{}", unique_base, audio_ext));
                if new_audio_path != old_audio_path {
                    info!(
                        "{} audio: {} -> {}",
                        if dry_run { "Would rename" } else { "Renaming" },
                        old_audio_path.display(),
                        new_audio_path.display()
                    );
                    if !dry_run {
                        if let Err(e) = fs::rename(&old_audio_path, &new_audio_path) {
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
                handled_audio.insert(new_audio_path);
            }
        }

        for audio_path in audio_files {
            if handled_audio.contains(&audio_path) {
                continue;
            }

            let dir = audio_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| day_path.clone());
            let stem = match audio_path.file_stem().and_then(|s| s.to_str()) {
                Some(stem) => stem.to_string(),
                None => {
                    warn!("Skipping non-UTF8 audio name: {}", audio_path.display());
                    report.skipped += 1;
                    continue;
                }
            };

            let (kind, base_stem, index) = split_kind_and_index(&stem, assume_kind);
            let (time_base_opt, slug_hint) = split_time_and_slug(&base_stem);
            let time_base = time_base_opt
                .or_else(|| time_base_from_metadata(&audio_path))
                .unwrap_or_else(|| Local::now().format("%H%M%S").to_string());

            let slug = slug_hint.as_deref().map(slug_from_hint).unwrap_or_default();
            let base = build_base_name(&time_base, &slug, kind);
            let base = match index {
                Some(i) if !i.is_empty() => format!("{}_{}", base, i),
                _ => base,
            };

            let unique_base = choose_unique_base(&dir, &base, None, Some(&audio_path), false);
            let audio_ext = audio_path
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or("wav");
            let new_audio_path = dir.join(format!("{}.{}", unique_base, audio_ext));

            if new_audio_path != audio_path {
                info!(
                    "{} orphan audio: {} -> {}",
                    if dry_run { "Would rename" } else { "Renaming" },
                    audio_path.display(),
                    new_audio_path.display()
                );
                if !dry_run {
                    if let Err(e) = fs::rename(&audio_path, &new_audio_path) {
                        warn!("Failed to rename audio {}: {}", audio_path.display(), e);
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

/// Save audio file to transcriptions folder with the given timestamp and optional slug.
///
/// Creates an optimized m4a archive alongside the transcript
/// (e.g., 143052_czesc-jak_raw.m4a pairs with 143052_czesc-jak_raw.txt).
///
/// # Arguments
/// * `src_path` - Path to the source WAV file (typically a temp file)
/// * `timestamp` - Timestamp to use for the filename (should match the transcript)
/// * `transcript_text` - Optional transcript text to generate slug from (first 3 words)
/// * `kind` - What kind of transcript artifact this is
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
    let result = (|| -> Result<PathBuf> {
        let mut source = daily_archive::admit_source(src_path)?;
        let base = build_base_name(
            &timestamp.format("%H%M%S").to_string(),
            &transcript_text
                .map(|text| make_slug(text, 3))
                .unwrap_or_default(),
            kind,
        );
        daily_archive::save(
            &crate::config::Config::config_dir(),
            &mut source,
            &timestamp,
            &base,
            None,
            crate::audio::archive::encode_wav_to_m4a,
        )
    })();
    archive_result(result)
}

fn archive_result(result: Result<PathBuf>) -> Option<PathBuf> {
    match result {
        Ok(path) => Some(path),
        Err(error) => {
            warn!("daily archive failed; source preserved: {error:#}");
            None
        }
    }
}

/// Put one take — hold or toggle — into the daily transcriptions bag.
///
/// Committed speech writes a paired `*_raw.{m4a,wav}` + `*_raw.txt` under
/// `~/.codescribe/transcriptions/YYYY-MM-DD/`. No-speech writes a `*_failed`
/// audio + empty marker with the fixed history title; unavailable lane output
/// archives audio only and never persists its diagnostic as transcript text.
/// `sessions/<id>.wav` is demux identity, not a second product bag.
#[cfg(test)]
fn save_session_transcript(
    transcript: SessionTranscriptArchive<'_>,
    timestamp: DateTime<Local>,
) -> Option<HistoryEntry> {
    match transcript {
        SessionTranscriptArchive::Committed(text) if !text.trim().is_empty() => Some(
            save_entry_with_timestamp(text, Some(timestamp), TranscriptKind::Raw),
        ),
        SessionTranscriptArchive::Committed(_) | SessionTranscriptArchive::NoSpeech => {
            Some(save_entry_with_timestamp_and_slug(
                "",
                Some(timestamp),
                TranscriptKind::Failed,
                Some("no-speech"),
            ))
        }
        SessionTranscriptArchive::Unavailable(_diagnostic) => None,
    }
}

/// Admit a pathname once, then use the held-source daily archive owner.
pub fn archive_session_take(
    src_path: &Path,
    transcript: SessionTranscriptArchive<'_>,
) -> Option<PathBuf> {
    match daily_archive::admit_source(src_path) {
        Ok(mut source) => archive_session_take_from_file(&mut source, transcript),
        Err(error) => archive_result(Err(error)),
    }
}

/// Archive the already admitted WAV. Never reopen its former pathname.
pub fn archive_session_take_from_file(
    source: &mut fs::File,
    transcript: SessionTranscriptArchive<'_>,
) -> Option<PathBuf> {
    archive_session_take_from_file_with_truth(source, transcript, None)
}

/// Archive the already admitted WAV and, when a take truth is supplied and the
/// take persisted transcript text, write its observer card beside the text:
/// `<base>.txt.truth.json` (`docs/truth-contract.md`).
///
/// The sidecar is written for the RETURNED archive path, so a collision-rename
/// (`_raw_1`) still pairs correctly. It is an OBSERVER projection — no
/// delivery path reads it back. An `Unavailable` take persists no text and
/// grows no sidecar. A sidecar write failure is a warning, never an archive
/// failure: the paired audio + text are already published at that point.
pub fn archive_session_take_from_file_with_truth(
    source: &mut fs::File,
    transcript: SessionTranscriptArchive<'_>,
    truth: Option<&TakeTruth>,
) -> Option<PathBuf> {
    let now = Local::now();
    let (slug, kind, text) = archive_classification(transcript);
    let base = build_base_name(&now.format("%H%M%S").to_string(), &make_slug(slug, 3), kind);
    let archived = archive_result(daily_archive::save(
        &crate::config::Config::config_dir(),
        source,
        &now,
        &base,
        text,
        crate::audio::archive::encode_wav_to_m4a,
    ));
    if let (Some(audio), Some(truth), Some(_)) = (archived.as_ref(), truth, text) {
        let transcript_path = audio.with_extension("txt");
        if let Err(error) = write_truth_sidecar(&transcript_path, truth) {
            warn!(
                "truth sidecar write failed for {}: {error:#}",
                transcript_path.display()
            );
        }
    }
    archived
}

fn archive_classification(
    transcript: SessionTranscriptArchive<'_>,
) -> (&str, TranscriptKind, Option<&str>) {
    match transcript {
        SessionTranscriptArchive::Committed(text) if !text.trim().is_empty() => {
            (text, TranscriptKind::Raw, Some(text.trim()))
        }
        SessionTranscriptArchive::Committed(_) | SessionTranscriptArchive::NoSpeech => {
            ("no-speech", TranscriptKind::Failed, Some(""))
        }
        SessionTranscriptArchive::Unavailable(_) => ("", TranscriptKind::Failed, None),
    }
}

/// Daily archive filesystem owner. All mutations are relative to pinned dirs.
#[cfg(unix)]
mod daily_archive {
    use super::*;
    use std::ffi::{CStr, CString};
    use std::fs::File;
    use std::io::{Seek, SeekFrom, Write};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Component;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_STAGE: AtomicU64 = AtomicU64::new(0);

    fn open_at(dir: &File, name: &CStr, flags: libc::c_int) -> std::io::Result<File> {
        if name.to_bytes().is_empty()
            || name.to_bytes().contains(&b'/')
            || matches!(name.to_bytes(), b"." | b"..")
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "unsafe archive component",
            ));
        }
        // SAFETY: live directory descriptor and single NUL-terminated component.
        let fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0o600,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: successful open transfers exactly one owned descriptor.
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    fn parent(path: &Path) -> Result<(File, CString)> {
        anyhow::ensure!(
            !path.components().any(|c| matches!(c, Component::ParentDir)),
            "archive refuses parent traversal"
        );
        let leaf = CString::new(path.file_name().context("archive needs leaf")?.as_bytes())?;
        let resolved = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .canonicalize()?;
        let mut dir = File::open("/")?;
        for component in resolved.components() {
            match component {
                Component::RootDir => {}
                Component::Normal(name) => {
                    dir = open_at(
                        &dir,
                        &CString::new(name.as_bytes())?,
                        libc::O_RDONLY | libc::O_DIRECTORY,
                    )?;
                }
                _ => anyhow::bail!("archive parent must be absolute"),
            }
        }
        Ok((dir, leaf))
    }

    pub(super) fn admit_source(path: &Path) -> Result<File> {
        let (dir, leaf) = parent(path)?;
        let source = open_at(&dir, &leaf, libc::O_RDONLY | libc::O_NONBLOCK)?;
        anyhow::ensure!(source.metadata()?.is_file(), "archive WAV must be regular");
        Ok(source)
    }

    fn subdir(dir: &File, name: &CStr) -> Result<File> {
        // SAFETY: names come from fixed components or validated date/root leaves.
        if unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o700) } < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(error.into());
            }
        }
        Ok(open_at(dir, name, libc::O_RDONLY | libc::O_DIRECTORY)?)
    }

    fn day(root: &Path, now: &DateTime<Local>) -> Result<(File, PathBuf)> {
        let (parent, leaf) = parent(root)?;
        let root_dir = subdir(&parent, &leaf)?;
        let archive = subdir(&root_dir, c"transcriptions")?;
        let date = now.format("%Y-%m-%d").to_string();
        let dir = subdir(&archive, &CString::new(date.as_str())?)?;
        Ok((dir, root.join("transcriptions").join(date)))
    }

    /// Own only an exclusively created entry. Published links have other names
    /// and are never removed by this guard, even after partial pair failure.
    struct Entry<'a> {
        dir: &'a File,
        name: CString,
        file: File,
    }

    impl Drop for Entry<'_> {
        fn drop(&mut self) {
            // SAFETY: remove only this guard's single staging/reservation entry.
            if unsafe { libc::unlinkat(self.dir.as_raw_fd(), self.name.as_ptr(), 0) } < 0 {
                warn!(
                    "archive owned-entry cleanup failed: {}",
                    std::io::Error::last_os_error()
                );
            }
        }
    }

    fn create<'a>(dir: &'a File, name: CString) -> std::io::Result<Entry<'a>> {
        let file = open_at(dir, &name, libc::O_RDWR | libc::O_CREAT | libc::O_EXCL)?;
        Ok(Entry { dir, name, file })
    }

    fn occupied(dir: &File, name: &CStr) -> Result<bool> {
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: fstatat initializes stat on success; no fields are read.
        let rc = unsafe {
            libc::fstatat(
                dir.as_raw_fd(),
                name.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if rc == 0 {
            return Ok(true);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            Ok(false)
        } else {
            Err(error.into())
        }
    }

    /// One reservation coordinates all writers, including text-only history.
    /// Existing dangling links, hardlinks and any other leaves occupy the stem.
    fn reserve<'a>(dir: &'a File, base: &str) -> Result<(Entry<'a>, String)> {
        for index in 0..=10_000 {
            let stem = if index == 0 {
                base.to_string()
            } else {
                format!("{base}_{index}")
            };
            let lock = match create(dir, CString::new(format!(".{stem}.reserve"))?) {
                Ok(entry) => entry,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            };
            let mut conflict = false;
            for extension in ["txt", "wav", "m4a"] {
                conflict |= occupied(dir, &CString::new(format!("{stem}.{extension}"))?)?;
            }
            if !conflict {
                return Ok((lock, stem));
            }
        }
        anyhow::bail!("daily archive stem allocation exhausted")
    }

    fn stage(dir: &File) -> Result<Entry<'_>> {
        for _ in 0..10_000 {
            let sequence = NEXT_STAGE.fetch_add(1, Ordering::Relaxed);
            let name = CString::new(format!(".archive-{}-{sequence}.tmp", std::process::id()))?;
            match create(dir, name) {
                Ok(entry) => return Ok(entry),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        anyhow::bail!("daily archive staging allocation exhausted")
    }

    fn publish(entry: &Entry<'_>, name: &str) -> Result<()> {
        entry.file.sync_all()?;
        let name = CString::new(name)?;
        // SAFETY: atomic no-replace publication in the same pinned directory.
        // A newly planted final leaf causes EEXIST, never an overwrite.
        if unsafe {
            libc::linkat(
                entry.dir.as_raw_fd(),
                entry.name.as_ptr(),
                entry.dir.as_raw_fd(),
                name.as_ptr(),
                0,
            )
        } < 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }

    pub(super) fn save_text(
        root: &Path,
        now: &DateTime<Local>,
        base: &str,
        text: &str,
    ) -> Result<PathBuf> {
        let (dir, path) = day(root, now)?;
        let (_reservation, stem) = reserve(&dir, base)?;
        let mut entry = stage(&dir)?;
        entry.file.write_all(text.as_bytes())?;
        let name = format!("{stem}.txt");
        publish(&entry, &name)?;
        Ok(path.join(name))
    }

    pub(super) fn save(
        root: &Path,
        source: &mut File,
        now: &DateTime<Local>,
        base: &str,
        text: Option<&str>,
        encode: impl FnOnce(&mut File, &mut File) -> Result<()>,
    ) -> Result<PathBuf> {
        anyhow::ensure!(
            source.metadata()?.is_file(),
            "archive source must be regular"
        );
        let (dir, path) = day(root, now)?;
        let (_reservation, stem) = reserve(&dir, base)?;
        // The converter can neither mutate the admitted WAV nor published data.
        let mut input = tempfile::tempfile()?;
        source.seek(SeekFrom::Start(0))?;
        std::io::copy(source, &mut input)?;
        input.seek(SeekFrom::Start(0))?;
        let mut encoded = tempfile::tempfile()?;
        let result = encode(&mut input, &mut encoded).and_then(|()| {
            anyhow::ensure!(
                encoded.metadata()?.len() > 0,
                "archive encoder returned empty success"
            );
            Ok(())
        });
        let mut audio = stage(&dir)?;
        let extension = match result {
            Ok(()) => {
                encoded.seek(SeekFrom::Start(0))?;
                std::io::copy(&mut encoded, &mut audio.file)?;
                "m4a"
            }
            Err(error) => {
                warn!("daily archive encoder refused; retaining admitted WAV: {error:#}");
                source.seek(SeekFrom::Start(0))?;
                std::io::copy(source, &mut audio.file)?;
                "wav"
            }
        };
        let audio_name = format!("{stem}.{extension}");
        publish(&audio, &audio_name)?;
        // Audio is retained if text persistence fails; never roll it back.
        if let Some(text) = text {
            let mut transcript = stage(&dir)?;
            transcript.file.write_all(text.as_bytes())?;
            publish(&transcript, &format!("{stem}.txt")).with_context(|| {
                format!(
                    "audio retained at {}; paired transcript failed",
                    path.join(&audio_name).display()
                )
            })?;
        }
        info!(
            "daily archive retained {}",
            path.join(&audio_name).display()
        );
        Ok(path.join(audio_name))
    }
}

/// Unsupported platforms refuse all daily writes rather than relax no-follow.
#[cfg(not(unix))]
mod daily_archive {
    use super::*;
    pub(super) fn admit_source(_path: &Path) -> Result<fs::File> {
        anyhow::bail!("secure daily archive requires Unix directory descriptors")
    }
    pub(super) fn save_text(
        _root: &Path,
        _now: &DateTime<Local>,
        _base: &str,
        _text: &str,
    ) -> Result<PathBuf> {
        anyhow::bail!("secure daily archive requires Unix directory descriptors")
    }
    pub(super) fn save(
        _root: &Path,
        _source: &mut fs::File,
        _now: &DateTime<Local>,
        _base: &str,
        _text: Option<&str>,
        _encode: impl FnOnce(&mut fs::File, &mut fs::File) -> Result<()>,
    ) -> Result<PathBuf> {
        anyhow::bail!("secure daily archive requires Unix directory descriptors")
    }
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

/// History path, save/retrieve, family collapse, and audio-archive regressions.
#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::io::Write;
    use tempfile::TempDir;

    #[cfg(unix)]
    fn archive_fixture(
        root: &Path,
        source: &mut fs::File,
        now: &DateTime<Local>,
        transcript: SessionTranscriptArchive<'_>,
        encode: impl FnOnce(&mut fs::File, &mut fs::File) -> Result<()>,
    ) -> Result<PathBuf> {
        let (slug, kind, text) = archive_classification(transcript);
        let base = build_base_name(&now.format("%H%M%S").to_string(), &make_slug(slug, 3), kind);
        daily_archive::save(root, source, now, &base, text, encode)
    }

    #[cfg(unix)]
    fn refuse_encoder(_input: &mut fs::File, _output: &mut fs::File) -> Result<()> {
        anyhow::bail!("injected converter failure")
    }

    /// A committed take archived with a `TakeTruth` leaves `<base>.txt` and a
    /// parseable `<base>.txt.truth.json` beside it; an `Unavailable` take
    /// persists no text and grows no sidecar.
    #[test]
    #[serial]
    #[cfg(unix)]
    fn archived_committed_take_with_truth_writes_parseable_sidecar() {
        use crate::pipeline::contracts::{
            RawTranscript, TranscriptionEngineMode, TranscriptionEngineVerdict,
            TranscriptionSource, TranscriptionVerdict, VadVerdict,
        };
        use crate::pipeline::take_truth::{read_truth_sidecar, truth_sidecar_path};

        let tmp = TempDir::new().expect("tempdir");
        let _guard = EnvGuard::set_to_temp_dir("CODESCRIBE_DATA_DIR", &tmp);
        let source_path = tmp.path().join("source.wav");
        fs::write(&source_path, b"take pcm").expect("source");

        let verdict = TranscriptionVerdict::from_parts(
            "zdanie".to_string(),
            RawTranscript {
                text: "zdanie".to_string(),
                ..Default::default()
            },
            Some(VadVerdict {
                speech_pct: 61.0,
                speech_windows: 10,
                total_windows: 25,
                no_speech: false,
                no_speech_reason: None,
                sparkline: "░███".to_string(),
                fine_sparkline: "▁▃█▃▁".to_string(),
                fine_hop_ms: 32,
            }),
            TranscriptionSource::LocalFinalPass,
            TranscriptionEngineVerdict::whisper(TranscriptionEngineMode::EmbeddedDefault),
            None,
        );
        let truth = TakeTruth::from_verdict(&verdict, "Final-pass local • Transcript", 0);

        let mut source = daily_archive::admit_source(&source_path).expect("admit");
        let audio = archive_session_take_from_file_with_truth(
            &mut source,
            SessionTranscriptArchive::Committed("zdanie"),
            Some(&truth),
        )
        .expect("archive");
        let transcript_path = audio.with_extension("txt");
        assert_eq!(
            fs::read_to_string(&transcript_path).expect("txt persisted"),
            "zdanie"
        );
        let restored = read_truth_sidecar(&transcript_path).expect("sidecar parses");
        assert_eq!(restored, truth);

        let mut unavailable_source = daily_archive::admit_source(&source_path).expect("admit");
        let unavailable_audio = archive_session_take_from_file_with_truth(
            &mut unavailable_source,
            SessionTranscriptArchive::Unavailable("lane down"),
            Some(&truth),
        )
        .expect("archive unavailable");
        let unavailable_txt = unavailable_audio.with_extension("txt");
        assert!(
            !unavailable_txt.exists(),
            "Unavailable persists no transcript text"
        );
        assert!(
            !truth_sidecar_path(&unavailable_txt).exists(),
            "Unavailable grows no truth sidecar"
        );
    }

    #[test]
    #[cfg(unix)]
    fn archive_rejects_root_archive_and_day_symlinks_without_outside_writes() {
        use std::os::unix::fs::symlink;
        for component in ["root", "archive", "day"] {
            let tmp = TempDir::new().expect("tempdir");
            let root = tmp.path().join("root");
            let outside = tmp.path().join("outside");
            fs::create_dir(&outside).expect("outside");
            fs::write(outside.join("sentinel"), b"untouched").expect("sentinel");
            let now = Local::now();
            let target = match component {
                "root" => root.clone(),
                "archive" => {
                    fs::create_dir(&root).expect("root");
                    root.join("transcriptions")
                }
                _ => {
                    fs::create_dir_all(root.join("transcriptions")).expect("archive");
                    root.join("transcriptions")
                        .join(now.format("%Y-%m-%d").to_string())
                }
            };
            symlink(&outside, target).expect("link");
            let source_path = tmp.path().join("source.wav");
            fs::write(&source_path, b"source survives").expect("source");
            let mut source = daily_archive::admit_source(&source_path).expect("admit");
            assert!(
                archive_fixture(
                    &root,
                    &mut source,
                    &now,
                    SessionTranscriptArchive::NoSpeech,
                    refuse_encoder
                )
                .is_err()
            );
            assert_eq!(fs::read(source_path).expect("source"), b"source survives");
            assert_eq!(fs::read_dir(&outside).expect("outside").count(), 1);
            assert_eq!(
                fs::read(outside.join("sentinel")).expect("sentinel"),
                b"untouched"
            );
        }
    }

    #[test]
    #[cfg(unix)]
    fn dangling_and_hardlinked_output_leaves_occupy_stem_without_overwrite() {
        use std::os::unix::fs::symlink;
        let tmp = TempDir::new().expect("tempdir");
        let now = Local::now();
        let day = tmp
            .path()
            .join("transcriptions")
            .join(now.format("%Y-%m-%d").to_string());
        fs::create_dir_all(&day).expect("day");
        let base = build_base_name(
            &now.format("%H%M%S").to_string(),
            "words",
            TranscriptKind::Raw,
        );
        let absent = tmp.path().join("absent");
        symlink(&absent, day.join(format!("{base}.m4a"))).expect("dangling");
        let outside = tmp.path().join("outside");
        fs::write(&outside, b"outside inode").expect("outside");
        fs::hard_link(&outside, day.join(format!("{base}_1.wav"))).expect("hardlink");
        symlink(&outside, day.join(format!("{base}_2.txt"))).expect("text link");
        let mut source = daily_archive::admit_source(&outside).expect("admit");
        let audio = archive_fixture(
            tmp.path(),
            &mut source,
            &now,
            SessionTranscriptArchive::Committed("words"),
            refuse_encoder,
        )
        .expect("archive");
        assert_eq!(
            audio.file_stem().and_then(|s| s.to_str()),
            Some(format!("{base}_3").as_str())
        );
        assert_eq!(fs::read(&outside).expect("outside"), b"outside inode");
        assert!(!absent.exists());
        assert_eq!(
            fs::read(audio.with_extension("txt")).expect("text"),
            b"words"
        );
    }

    #[test]
    #[cfg(unix)]
    fn renamed_day_and_source_replacement_keep_admitted_objects() {
        use std::os::unix::fs::symlink;
        let tmp = TempDir::new().expect("tempdir");
        let source_path = tmp.path().join("source.wav");
        fs::write(&source_path, b"admitted voice").expect("source");
        let mut source = daily_archive::admit_source(&source_path).expect("admit");
        let now = Local::now();
        let day = tmp
            .path()
            .join("transcriptions")
            .join(now.format("%Y-%m-%d").to_string());
        let moved = tmp.path().join("moved-day");
        let outside = tmp.path().join("outside");
        fs::create_dir(&outside).expect("outside");
        let audio = archive_fixture(
            tmp.path(),
            &mut source,
            &now,
            SessionTranscriptArchive::Committed("voice"),
            |input, _| {
                use std::io::{Read, Seek};
                fs::rename(&source_path, tmp.path().join("original.wav"))?;
                fs::write(&source_path, b"substituted")?;
                fs::rename(&day, &moved)?;
                symlink(&outside, &day)?;
                let mut bytes = Vec::new();
                input.rewind()?;
                input.read_to_end(&mut bytes)?;
                assert_eq!(bytes, b"admitted voice");
                anyhow::bail!("force WAV fallback")
            },
        )
        .expect("fallback");
        assert_eq!(
            fs::read(moved.join(audio.file_name().expect("name"))).expect("audio"),
            b"admitted voice"
        );
        assert_eq!(fs::read_dir(&outside).expect("outside").count(), 0);
        assert_eq!(fs::read_dir(&moved).expect("moved").count(), 2);
        assert_eq!(fs::read(source_path).expect("replacement"), b"substituted");
    }

    #[test]
    #[cfg(unix)]
    fn late_output_collision_refuses_publication_and_preserves_unowned_entry() {
        use std::os::unix::fs::symlink;
        let tmp = TempDir::new().expect("tempdir");
        let source_path = tmp.path().join("source.wav");
        fs::write(&source_path, b"source").expect("source");
        let mut source = daily_archive::admit_source(&source_path).expect("admit");
        let now = Local::now();
        let day = tmp
            .path()
            .join("transcriptions")
            .join(now.format("%Y-%m-%d").to_string());
        let base = build_base_name(
            &now.format("%H%M%S").to_string(),
            "",
            TranscriptKind::Failed,
        );
        let absent = tmp.path().join("absent");
        assert!(
            archive_fixture(
                tmp.path(),
                &mut source,
                &now,
                SessionTranscriptArchive::Unavailable("diagnostic"),
                |_, _| {
                    symlink(&absent, day.join(format!("{base}.wav")))?;
                    anyhow::bail!("fallback")
                }
            )
            .is_err()
        );
        assert!(!absent.exists());
        assert_eq!(fs::read_dir(day).expect("day").count(), 1);
        assert_eq!(fs::read(source_path).expect("source"), b"source");
    }

    #[test]
    #[cfg(unix)]
    #[serial]
    fn concurrent_same_second_archives_keep_pairs_and_history_rows() {
        use std::sync::{Arc, Barrier};
        let tmp = TempDir::new().expect("tempdir");
        let _guard = EnvGuard::set_to_temp_dir("CODESCRIBE_DATA_DIR", &tmp);
        let source_path = tmp.path().join("source.wav");
        fs::write(&source_path, b"same admitted voice").expect("source");
        let now = Local::now();
        let barrier = Arc::new(Barrier::new(2));
        let paths = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..2)
                .map(|_| {
                    let barrier = Arc::clone(&barrier);
                    let root = tmp.path();
                    let source_path = &source_path;
                    let now = &now;
                    scope.spawn(move || {
                        let mut source = daily_archive::admit_source(source_path).expect("admit");
                        archive_fixture(
                            root,
                            &mut source,
                            now,
                            SessionTranscriptArchive::Committed("same words"),
                            |_, output| {
                                barrier.wait();
                                output.write_all(b"fake encoded audio")?;
                                Ok(())
                            },
                        )
                        .expect("archive")
                    })
                })
                .collect();
            workers
                .into_iter()
                .map(|w| w.join().expect("worker"))
                .collect::<Vec<_>>()
        });
        assert_ne!(paths[0], paths[1]);
        for audio in paths {
            assert_eq!(fs::read(&audio).expect("audio"), b"fake encoded audio");
            assert_eq!(
                fs::read(audio.with_extension("txt")).expect("text"),
                b"same words"
            );
        }
        assert_eq!(recent_entries(10).len(), 2);
    }

    #[test]
    #[cfg(unix)]
    fn child_failure_deadline_and_empty_exit_publish_wav_and_clean_staging() {
        use crate::audio::archive::TestEncoderOutcome;
        for outcome in [
            TestEncoderOutcome::Failed,
            TestEncoderOutcome::Empty,
            TestEncoderOutcome::Hanging,
            TestEncoderOutcome::Symlink,
            TestEncoderOutcome::Directory,
            TestEncoderOutcome::Fifo,
            TestEncoderOutcome::Hardlink,
        ] {
            let tmp = TempDir::new().expect("tempdir");
            let source_path = tmp.path().join("source.wav");
            fs::write(&source_path, b"original WAV").expect("source");
            let mut source = daily_archive::admit_source(&source_path).expect("admit");
            let audio = archive_fixture(
                tmp.path(),
                &mut source,
                &Local::now(),
                SessionTranscriptArchive::Committed("words"),
                |input, output| crate::audio::archive::encode_test_child(input, output, outcome),
            )
            .expect("fallback");
            assert_eq!(audio.extension().and_then(|s| s.to_str()), Some("wav"));
            assert_eq!(fs::read(&audio).expect("audio"), b"original WAV");
            assert_eq!(fs::read(source_path).expect("source"), b"original WAV");
            assert_eq!(
                fs::read(audio.with_extension("txt")).expect("text"),
                b"words"
            );
            assert_eq!(
                fs::read_dir(audio.parent().expect("day"))
                    .expect("day")
                    .count(),
                2
            );
        }
    }

    #[test]
    #[cfg(unix)]
    #[serial]
    fn real_archive_classifies_outcomes_and_empty_success_falls_back() {
        let tmp = TempDir::new().expect("tempdir");
        let _guard = EnvGuard::set_to_temp_dir("CODESCRIBE_DATA_DIR", &tmp);
        let source_path = tmp.path().join("source.wav");
        fs::write(&source_path, b"recover this voice").expect("source");
        for transcript in [
            SessionTranscriptArchive::Committed("spoken words"),
            SessionTranscriptArchive::NoSpeech,
            SessionTranscriptArchive::Unavailable("SECRET diagnostic"),
        ] {
            let mut source = daily_archive::admit_source(&source_path).expect("admit");
            let audio = archive_fixture(
                tmp.path(),
                &mut source,
                &Local::now(),
                transcript,
                |_, _| Ok(()),
            )
            .expect("empty-success WAV fallback");
            assert_eq!(audio.extension().and_then(|s| s.to_str()), Some("wav"));
            assert_eq!(fs::read(&audio).expect("audio"), b"recover this voice");
            let text = audio.with_extension("txt");
            match transcript {
                SessionTranscriptArchive::Committed(words) => {
                    assert_eq!(fs::read_to_string(text).expect("text"), words)
                }
                SessionTranscriptArchive::NoSpeech => {
                    assert_eq!(fs::read(text).expect("marker"), b"")
                }
                SessionTranscriptArchive::Unavailable(_) => assert!(!text.exists()),
            }
        }
        let entries = recent_entries(10);
        assert_eq!(entries.len(), 2);
        assert!(
            entries
                .iter()
                .any(|e| e.kind == TranscriptKind::Failed && e.preview == NO_SPEECH_HISTORY_TITLE)
        );
        assert_eq!(
            latest_copyable_entry().expect("copyable").preview,
            "spoken words"
        );
        assert_eq!(
            fs::read(source_path).expect("source"),
            b"recover this voice"
        );
    }

    /// Write a short PCM16 sine WAV fixture for m4a archive size/decode checks.
    fn write_pcm16_sine_wav(path: &Path, sample_rate: u32, seconds: u32) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).expect("create wav fixture");
        let total_samples = sample_rate as usize * seconds as usize;
        for i in 0..total_samples {
            let t = i as f32 / sample_rate as f32;
            let tone = (2.0 * std::f32::consts::PI * 440.0 * t).sin()
                + 0.35 * (2.0 * std::f32::consts::PI * 880.0 * t).sin();
            let sample = (tone * 12_000.0).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
            writer.write_sample(sample).expect("write wav sample");
        }
        writer.finalize().expect("finalize wav fixture");
    }

    /// Restores a process env var on drop so serial history tests do not leak dirs.
    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }

    impl EnvGuard {
        /// Point `key` at a temp data dir, capturing the previous value for restore.
        fn set_to_temp_dir(key: &'static str, dir: &TempDir) -> Self {
            let prev = std::env::var(key).ok();
            unsafe {
                std::env::set_var(key, dir.path());
            }
            Self { key, prev }
        }
    }

    impl Drop for EnvGuard {
        /// Restore or remove the env var when the guard leaves scope.
        fn drop(&mut self) {
            unsafe {
                match &self.prev {
                    Some(v) => std::env::set_var(self.key, v),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    /// transcriptions_dir lives under CODESCRIBE_DATA_DIR and includes the day path.
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

    /// save_entry writes a .txt raw artifact with preview equal to the stored text.
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
        assert_eq!(entry.kind, TranscriptKind::Raw);
        assert!(entry.path.to_string_lossy().ends_with(".txt"));
        assert!(entry.path.starts_with(&tmp_canon));

        // Clean up
        let _ = fs::remove_file(&entry.path);
    }

    /// save_entry_with_timestamp stamps the entry with the provided local time.
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

    /// Same-second same-slug raw saves collapse to one recent row (newest wins).
    #[test]
    #[serial]
    fn test_recent_entries_collapse_same_second_save_family() {
        let tmp = TempDir::new().expect("tempdir");
        let _guard = EnvGuard::set_to_temp_dir("CODESCRIBE_DATA_DIR", &tmp);

        // Raw draft and its post-processed final land in the same second with
        // the same slug → the second save collides into `…_raw_1.txt`.
        let now = Local::now();
        let slug_hint = Some("event zwykly nie");
        let draft = save_entry_with_timestamp_and_slug(
            "event zwykły nie może",
            Some(now),
            TranscriptKind::Raw,
            slug_hint,
        );
        let finalized = save_entry_with_timestamp_and_slug(
            "Event zwykły nie może być agentowym.",
            Some(now),
            TranscriptKind::Raw,
            slug_hint,
        );
        assert_ne!(draft.path, finalized.path, "collision suffix expected");

        // A different kind at the same second is a separate history row.
        let interpretation = save_entry_with_timestamp_and_slug(
            "Odpowiedź asystenta.",
            Some(now),
            TranscriptKind::AssistantInterpretation,
            slug_hint,
        );

        let entries = recent_entries(10);
        let raw_rows: Vec<_> = entries
            .iter()
            .filter(|e| e.kind == TranscriptKind::Raw)
            .collect();
        assert_eq!(
            raw_rows.len(),
            1,
            "same-family raw draft + final must collapse to one row: {:?}",
            entries.iter().map(|e| &e.path).collect::<Vec<_>>()
        );
        assert_eq!(
            raw_rows[0].path, finalized.path,
            "the newest (post-processed) file wins the family"
        );
        assert!(
            entries.iter().any(|e| e.path == interpretation.path),
            "different kind survives as its own row"
        );

        for path in [&draft.path, &finalized.path, &interpretation.path] {
            let _ = fs::remove_file(path);
        }
    }

    /// HistoryEntry::label includes the transcript preview for UI display.
    #[test]
    #[serial]
    fn test_entry_label() {
        let entry = HistoryEntry {
            path: PathBuf::from("/tmp/test.txt"),
            timestamp: Local::now(),
            preview: "Hello world".to_string(),
            kind: TranscriptKind::Raw,
        };

        let label = entry.label();
        assert!(label.contains("Hello world"));
    }

    /// A simulated lane failure has no transcript artifact path. The previous
    /// committed user words remain the copy target, while a legitimate empty
    /// take gets the one explicit non-speech title.
    #[test]
    #[serial]
    fn session_archive_outcome_keeps_lane_diagnostic_out_of_history_and_copy() {
        let tmp = TempDir::new().expect("tempdir");
        let _guard = EnvGuard::set_to_temp_dir("CODESCRIBE_DATA_DIR", &tmp);
        let now = Local::now();
        let user_words = "To są słowa Foundera";
        let lane_error = "Tool-enabled response failed (ConnectError: gateway unavailable)";

        let committed =
            save_session_transcript(SessionTranscriptArchive::Committed(user_words), now)
                .expect("committed speech must create a history row");
        assert!(
            save_session_transcript(
                SessionTranscriptArchive::Unavailable(lane_error),
                now + chrono::Duration::seconds(1),
            )
            .is_none(),
            "diagnostics must not create transcript artifacts"
        );
        let no_speech = save_session_transcript(
            SessionTranscriptArchive::NoSpeech,
            now + chrono::Duration::seconds(2),
        )
        .expect("no-speech must create an explicit history row");

        assert_eq!(committed.preview, user_words);
        assert!(committed.label().contains(user_words));
        assert_eq!(no_speech.preview, NO_SPEECH_HISTORY_TITLE);
        assert!(no_speech.label().contains(NO_SPEECH_HISTORY_TITLE));

        let copyable = latest_copyable_entry().expect("last transcript remains available");
        assert_eq!(copyable.path, committed.path);
        assert_eq!(fs::read_to_string(&copyable.path).unwrap(), user_words);
        assert!(recent_entries(8).iter().all(|entry| {
            !fs::read_to_string(&entry.path)
                .unwrap_or_default()
                .contains(lane_error)
        }));
    }

    /// Shared slug_hint aligns base filenames across raw vs formatted kinds.
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
            TranscriptKind::FormattedTranscript,
            Some("shared slug source"),
        );

        let raw_stem = raw.path.file_stem().unwrap().to_string_lossy();
        let ai_stem = ai.path.file_stem().unwrap().to_string_lossy();
        let raw_base = raw_stem.strip_suffix("_raw").unwrap_or(&raw_stem);
        let ai_base = ai_stem.strip_suffix("_formatted").unwrap_or(&ai_stem);

        assert_eq!(raw_base, ai_base, "Slug hint should align base name");
    }

    /// recent_entries recovers TranscriptKind from the on-disk filename suffix.
    #[test]
    #[serial]
    fn test_recent_entries_parses_kind_from_filename_suffix() {
        let tmp = TempDir::new().expect("tempdir");
        let _guard = EnvGuard::set_to_temp_dir("CODESCRIBE_DATA_DIR", &tmp);

        let now = Local::now();
        let _raw = save_entry_with_timestamp_and_slug(
            "raw content",
            Some(now),
            TranscriptKind::Raw,
            Some("shared slug source"),
        );
        let formatted = save_entry_with_timestamp_and_slug(
            "formatted content",
            Some(now),
            TranscriptKind::FormattedTranscript,
            Some("shared slug source"),
        );

        let entries = recent_entries(8);
        assert!(entries.iter().any(|entry| {
            entry.path == formatted.path && entry.kind == TranscriptKind::FormattedTranscript
        }));
    }

    /// Legacy Failed payloads are never title or copy authority.
    #[test]
    #[serial]
    fn test_latest_copyable_entry_skips_failed_artifacts() {
        let tmp = TempDir::new().expect("tempdir");
        let _guard = EnvGuard::set_to_temp_dir("CODESCRIBE_DATA_DIR", &tmp);

        let now = Local::now();
        let raw = save_entry_with_timestamp_and_slug(
            "usable transcript",
            Some(now),
            TranscriptKind::Raw,
            Some("usable transcript"),
        );
        let failed = save_entry_with_timestamp_and_slug(
            "Tool-enabled response failed (ConnectError: gateway unavailable)",
            Some(now + chrono::Duration::seconds(1)),
            TranscriptKind::Failed,
            Some("no-speech"),
        );

        let latest = latest_entry().expect("latest entry");
        assert_eq!(latest.path, failed.path);
        assert_eq!(latest.kind, TranscriptKind::Failed);
        assert_eq!(latest.preview, NO_SPEECH_HISTORY_TITLE);
        assert!(!latest.label().contains("ConnectError"));

        let copyable = latest_copyable_entry().expect("latest copyable entry");
        assert_eq!(copyable.path, raw.path);
        assert_eq!(copyable.kind, TranscriptKind::Raw);
        assert_eq!(
            fs::read_to_string(copyable.path).unwrap(),
            "usable transcript"
        );
    }

    /// Real encoder witness using held files after both public paths are replaced.
    #[test]
    #[cfg(target_os = "macos")]
    fn production_encoder_preserves_held_source_and_destination_and_decodes_m4a() {
        use std::io::{Read, Seek};
        let tmp = TempDir::new().expect("tempdir");
        let source_path = tmp.path().join("source.wav");
        write_pcm16_sine_wav(&source_path, 16_000, 5);
        let original = fs::read(&source_path).expect("original WAV");
        let mut source = daily_archive::admit_source(&source_path).expect("admit source");
        let moved_source = tmp.path().join("held.wav");
        fs::rename(&source_path, &moved_source).expect("move source");
        fs::write(&source_path, b"foreign source").expect("replace source");
        let destination_path = tmp.path().join("destination.m4a");
        let mut destination = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&destination_path)
            .expect("held destination");
        let moved_destination = tmp.path().join("held.m4a");
        fs::rename(&destination_path, &moved_destination).expect("move destination");
        fs::write(&destination_path, b"foreign destination").expect("replace destination");

        crate::audio::archive::encode_wav_to_m4a(&mut source, &mut destination)
            .expect("production afconvert conversion");
        let mut encoded = Vec::new();
        destination.read_to_end(&mut encoded).expect("held result");
        assert!(encoded.len() > 12 && encoded.len() < original.len());
        assert_eq!(&encoded[4..8], b"ftyp", "M4A container, never WAV fallback");
        let (decoded, rate) =
            crate::audio::load_audio_file(&moved_destination).expect("decode production M4A");
        assert!(rate > 0 && !decoded.is_empty());
        let seconds = decoded.len() as f32 / rate as f32;
        assert!((4.0..=6.0).contains(&seconds), "decoded duration {seconds}");
        assert!(
            decoded.iter().any(|sample| sample.abs() > 0.01),
            "non-silent PCM"
        );
        source.rewind().expect("rewind source");
        let mut preserved = Vec::new();
        source.read_to_end(&mut preserved).expect("read source");
        assert_eq!(preserved, original);
        assert_eq!(fs::read(&moved_source).expect("held source"), original);
        assert_eq!(
            fs::read(&source_path).expect("foreign source"),
            b"foreign source"
        );
        assert_eq!(
            fs::read(&destination_path).expect("foreign destination"),
            b"foreign destination"
        );
    }

    /// macOS: save_audio archives to smaller m4a that still decodes near source duration.
    #[test]
    #[serial]
    #[cfg(target_os = "macos")]
    fn test_save_audio_archives_m4a_smaller_and_decodable() {
        let tmp = TempDir::new().expect("tempdir");
        let _guard = EnvGuard::set_to_temp_dir("CODESCRIBE_DATA_DIR", &tmp);
        let src_wav = tmp.path().join("source.wav");
        write_pcm16_sine_wav(&src_wav, 16_000, 5);
        let wav_size = fs::metadata(&src_wav).expect("wav metadata").len();

        let saved = save_audio(
            &src_wav,
            Local::now(),
            Some("archive verifier speech"),
            TranscriptKind::Raw,
        )
        .expect("archive audio");

        assert_eq!(saved.extension().and_then(|ext| ext.to_str()), Some("m4a"));
        assert!(saved.exists(), "archive file should exist");

        let m4a_size = fs::metadata(&saved).expect("m4a metadata").len();
        assert!(
            m4a_size < wav_size,
            "m4a archive should be smaller than source wav (m4a={m4a_size}, wav={wav_size})"
        );

        let (decoded, decoded_rate) =
            crate::audio::load_audio_file(&saved).expect("decode archived m4a");
        assert!(decoded_rate > 0, "decoded sample rate should be known");
        assert!(
            !decoded.is_empty(),
            "decoded archive should produce PCM samples"
        );

        let decoded_sec = decoded.len() as f32 / decoded_rate as f32;
        assert!(
            (4.0..=6.0).contains(&decoded_sec),
            "decoded archive duration should stay near source duration, got {decoded_sec:.2}s"
        );
    }
}
