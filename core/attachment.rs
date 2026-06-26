//! Attachment model and on-disk store for CodeScribe.
//!
//! Provides a thin wrapper around file paths with metadata (kind, source,
//! display name) used by the Agent chat UI and LLM context pipeline.
//!
//! Files from drag&drop and file picker are referenced in-place.
//! Files from clipboard images, GitHub, and URL fetch are persisted to
//! `~/.codescribe/attachments/`.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use tracing::{debug, warn};

// ═══════════════════════════════════════════════════════════
// Constants
// ═══════════════════════════════════════════════════════════

/// Maximum attachment size (50 MB). Files larger than this are allowed but
/// treated as oversized: they are logged with a warning and can be detected
/// via `Attachment::is_oversized()`.
const MAX_ATTACHMENT_BYTES: u64 = 50 * 1024 * 1024;

// ═══════════════════════════════════════════════════════════
// Types
// ═══════════════════════════════════════════════════════════

/// Classification of an attached file for UI display and LLM context building.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentKind {
    /// PNG, JPEG, HEIC, WebP, GIF, BMP, TIFF
    Image,
    /// PDF document
    Pdf,
    /// UTF-8 text file (.txt, .md, .rs, .py, .json, etc.)
    Text,
    /// Binary or unknown file type (fallback)
    File,
    /// Snapshot of a web page fetched by URL connector
    UrlSnapshot,
    /// File fetched from a GitHub repository
    GitHubBlob,
}

/// How the attachment was ingested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachmentSource {
    /// Pasted via Cmd+V (image or file URL from clipboard)
    Clipboard,
    /// Dragged onto the input bar
    DragDrop,
    /// Selected via NSOpenPanel file picker
    FilePicker,
    /// Fetched by a named connector (e.g. "github", "web")
    Connector(String),
}

/// A single attachment in the Agent chat context.
///
/// The UI works with lightweight `Attachment` handles — the actual file
/// content lives on disk at `path`. This avoids holding large blobs in
/// memory on the main thread.
#[derive(Debug, Clone)]
pub struct Attachment {
    /// On-disk location (original or stored in `~/.codescribe/attachments/`)
    pub path: PathBuf,
    /// Detected file type
    pub kind: AttachmentKind,
    /// How this attachment was added
    pub source: AttachmentSource,
    /// Human-readable name for UI display (file name or derived label)
    pub display_name: String,
    /// File size in bytes (0 if unknown)
    pub size_bytes: u64,
}

impl Attachment {
    /// Create an attachment from a file path, inferring kind from extension.
    ///
    /// Logs a warning if the file exceeds `MAX_ATTACHMENT_BYTES` (50 MB).
    pub fn from_path(path: PathBuf, source: AttachmentSource) -> Self {
        let display_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());

        let size_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if size_bytes > MAX_ATTACHMENT_BYTES {
            warn!(
                "Attachment too large: {} ({} bytes, max {})",
                display_name, size_bytes, MAX_ATTACHMENT_BYTES
            );
        }

        let kind = kind_from_extension(&path);

        Self {
            path,
            kind,
            source,
            display_name,
            size_bytes,
        }
    }

    /// Create an attachment with an explicit kind (for connectors).
    pub fn with_kind(path: PathBuf, kind: AttachmentKind, source: AttachmentSource) -> Self {
        let display_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());

        let size_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

        Self {
            path,
            kind,
            source,
            display_name,
            size_bytes,
        }
    }

    /// Extract paths from a slice of attachments (for `build_attachments_block`).
    pub fn paths(attachments: &[Attachment]) -> Vec<PathBuf> {
        attachments.iter().map(|a| a.path.clone()).collect()
    }

    /// SF Symbol name for this attachment's kind (for UI chip icon).
    pub fn sf_symbol(&self) -> &'static str {
        match self.kind {
            AttachmentKind::Image => "photo",
            AttachmentKind::Pdf => "doc.richtext",
            AttachmentKind::Text => "doc.text",
            AttachmentKind::File => "doc",
            AttachmentKind::UrlSnapshot => "globe",
            AttachmentKind::GitHubBlob => "chevron.left.forwardslash.chevron.right",
        }
    }

    /// Truncated display name for chip labels (max `limit` chars).
    pub fn chip_label(&self, limit: usize) -> String {
        if limit == 0 {
            return String::new();
        }
        if self.display_name.chars().count() <= limit {
            self.display_name.clone()
        } else {
            let truncated: String = self
                .display_name
                .chars()
                .take(limit.saturating_sub(1))
                .collect();
            format!("{truncated}…")
        }
    }

    /// Check if this attachment has the same path as another.
    pub fn same_path(&self, other: &Path) -> bool {
        self.path == other
    }

    /// Returns `true` if the file exceeds the maximum attachment size.
    pub fn is_oversized(&self) -> bool {
        self.size_bytes > MAX_ATTACHMENT_BYTES
    }
}

// ═══════════════════════════════════════════════════════════
// Kind detection
// ═══════════════════════════════════════════════════════════

/// Infer `AttachmentKind` from file extension.
fn kind_from_extension(path: &Path) -> AttachmentKind {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "heic" | "heif" | "webp" | "gif" | "bmp" | "tif" | "tiff"
        | "svg" | "ico" | "raw" | "cr2" | "nef" | "arw" => AttachmentKind::Image,
        "pdf" => AttachmentKind::Pdf,
        "txt" | "md" | "markdown" | "rst" | "org" | "csv" | "tsv" | "log" | "json" | "jsonl"
        | "yaml" | "yml" | "toml" | "xml" | "html" | "htm" | "css" | "js" | "ts" | "jsx"
        | "tsx" | "rs" | "py" | "rb" | "go" | "java" | "kt" | "swift" | "c" | "cpp" | "h"
        | "hpp" | "sh" | "bash" | "zsh" | "fish" | "ps1" | "bat" | "sql" | "r" | "lua" | "pl"
        | "ex" | "exs" | "erl" | "hs" | "ml" | "mli" | "lisp" | "clj" | "scala" | "dart"
        | "vue" | "svelte" | "astro" | "env" | "ini" | "cfg" | "conf" | "diff" | "patch"
        | "tex" | "bib" | "dockerfile" | "makefile" | "cmake" => AttachmentKind::Text,
        _ => AttachmentKind::File,
    }
}

// ═══════════════════════════════════════════════════════════
// Vision attachment parsing (shared by agent + legacy send paths)
// ═══════════════════════════════════════════════════════════

/// Marker line emitted by `build_attachments_block` that introduces the list of
/// image file paths appended to a chat payload as text.
pub const IMAGE_PATHS_MARKER: &str = "ATTACHMENTS (image paths)";

/// Default per-image byte cap honored when loading images for vision input.
pub const MAX_VISION_IMAGE_BYTES: u64 = 8 * 1024 * 1024;

/// MIME media type for a vision-supported image, inferred from extension.
///
/// Returns `None` for extensions the model APIs do not accept as image input
/// (e.g. `svg`, `heic`, `raw`), even though [`AttachmentKind::Image`] classifies
/// them as images for UI purposes.
pub fn image_media_type(path: &Path) -> Option<&'static str> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        "bmp" => Some("image/bmp"),
        "tif" | "tiff" => Some("image/tiff"),
        _ => None,
    }
}

/// Split a chat payload into its visible text and the image paths listed under
/// the `ATTACHMENTS (image paths)` marker appended by `build_attachments_block`.
///
/// The marker block (and a dangling `---`/`—` separator directly above it) is
/// removed from the returned text so the model never sees raw file paths where a
/// real vision input belongs. The original payload is returned verbatim in
/// `.0` when no marker is present, so non-attachment messages pass through
/// unchanged.
pub fn parse_image_attachment_block(text: &str) -> (String, Vec<PathBuf>) {
    let mut out_lines: Vec<String> = Vec::new();
    let mut image_paths: Vec<PathBuf> = Vec::new();
    let mut in_block = false;

    for line in text.lines() {
        let trimmed = line.trim();

        if trimmed == IMAGE_PATHS_MARKER {
            // Drop a preceding separator if present to avoid leaving a dangling "---".
            if out_lines
                .last()
                .is_some_and(|l| l.trim() == "---" || l.trim() == "—")
            {
                out_lines.pop();
            }
            in_block = true;
            continue;
        }

        if in_block {
            if trimmed.is_empty() {
                in_block = false;
                continue;
            }
            if let Some(rest) = trimmed.strip_prefix("- ") {
                let p = rest.trim();
                if !p.is_empty() {
                    image_paths.push(PathBuf::from(p));
                }
                continue;
            }
            // Unexpected line → end block, keep the line.
            in_block = false;
            out_lines.push(line.to_string());
            continue;
        }

        out_lines.push(line.to_string());
    }

    (out_lines.join("\n"), image_paths)
}

/// Load an image file as `(bytes, media_type)` for vision input.
///
/// Returns `None` (with a warning) when the extension is not a vision-supported
/// image, the file is unreadable, or it exceeds `max_bytes`.
pub fn load_image_for_vision(path: &Path, max_bytes: u64) -> Option<(Vec<u8>, String)> {
    let media_type = image_media_type(path)?;

    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > max_bytes {
        warn!(
            "Skipping image attachment (too large, {} bytes > {} max): {}",
            meta.len(),
            max_bytes,
            path.display()
        );
        return None;
    }

    match std::fs::read(path) {
        Ok(bytes) if bytes.is_empty() => {
            // An empty/zero-byte file would encode to an empty base64 string,
            // which providers reject ("empty base64-encoded bytes") and which
            // fails the whole request. Drop it instead.
            warn!("Skipping image attachment (empty file): {}", path.display());
            None
        }
        Ok(bytes) => Some((bytes, media_type.to_string())),
        Err(e) => {
            warn!("Failed to read image attachment {}: {}", path.display(), e);
            None
        }
    }
}

// ═══════════════════════════════════════════════════════════
// Attachment Store
// ═══════════════════════════════════════════════════════════

/// Manages on-disk storage for attachments that don't have a stable
/// external path (clipboard images, connector downloads).
pub struct AttachmentStore;

impl AttachmentStore {
    /// Root directory for stored attachments: `~/.codescribe/attachments/`
    pub fn store_dir() -> PathBuf {
        crate::config::Config::config_dir().join("attachments")
    }

    /// Ensure the store directory exists.
    fn ensure_dir() -> Result<PathBuf> {
        let dir = Self::store_dir();
        if !dir.exists() {
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("Failed to create attachments dir: {}", dir.display()))?;
            debug!("Created attachments directory: {}", dir.display());
        }
        Ok(dir)
    }

    /// Save clipboard image data to disk and return the path.
    ///
    /// File name: `clipboard_{timestamp}.{ext}`
    pub fn save_clipboard_image(data: &[u8], ext: &str) -> Result<PathBuf> {
        let dir = Self::ensure_dir()?;
        let ts = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        // Sanitize ext: only allow alphanumeric chars (no path separators).
        let safe_ext: String = ext
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(10)
            .collect();
        let safe_ext = if safe_ext.is_empty() {
            "bin"
        } else {
            &safe_ext
        };
        let name = format!("clipboard_{ts}.{safe_ext}");
        let path = dir.join(&name);
        std::fs::write(&path, data)
            .with_context(|| format!("Failed to save clipboard image: {}", path.display()))?;
        debug!(
            "Saved clipboard image: {} ({} bytes)",
            path.display(),
            data.len()
        );
        Ok(path)
    }

    /// Save fetched content (GitHub blob, URL snapshot) to disk.
    ///
    /// File name: `{prefix}_{sanitized_name}`
    pub fn save_fetched(data: &[u8], name: &str, prefix: &str) -> Result<PathBuf> {
        let dir = Self::ensure_dir()?;
        let sanitized = sanitize_filename(name);
        let ts = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let full_name = format!("{prefix}_{ts}_{sanitized}");
        let path = dir.join(&full_name);
        std::fs::write(&path, data)
            .with_context(|| format!("Failed to save fetched content: {}", path.display()))?;
        debug!(
            "Saved fetched content: {} ({} bytes)",
            path.display(),
            data.len()
        );
        Ok(path)
    }

    /// Save text content to disk (convenience for URL snapshots).
    pub fn save_text(text: &str, name: &str, prefix: &str) -> Result<PathBuf> {
        Self::save_fetched(text.as_bytes(), name, prefix)
    }

    /// Delete old stored attachments (files older than `max_age_days`).
    pub fn cleanup_old(max_age_days: u32) {
        let dir = Self::store_dir();
        if !dir.exists() {
            return;
        }

        let cutoff = std::time::Duration::from_secs(max_age_days as u64 * 86400);
        let now = SystemTime::now();

        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) => {
                warn!("Failed to read attachments dir for cleanup: {}", e);
                return;
            }
        };

        let mut removed = 0u32;
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            let Ok(modified) = meta.modified() else {
                continue;
            };
            if let Ok(age) = now.duration_since(modified)
                && age > cutoff
            {
                let path = entry.path();
                if std::fs::remove_file(&path).is_ok() {
                    removed += 1;
                } else {
                    tracing::warn!("Attachment cleanup: failed to delete {}", path.display());
                }
            }
        }

        if removed > 0 {
            debug!(
                "Attachment cleanup: removed {} files older than {} days",
                removed, max_age_days
            );
        }
    }
}

/// Sanitize a filename by replacing unsafe characters.
fn sanitize_filename(name: &str) -> String {
    let sanitized = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>();
    // Strip leading dots to prevent hidden files / path traversal.
    let trimmed = sanitized.trim_start_matches('.');
    let trimmed: String = trimmed.chars().take(100).collect(); // cap length
    if trimmed.is_empty() {
        "attachment".to_string()
    } else {
        trimmed
    }
}

// ═══════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kind_from_extension() {
        assert_eq!(
            kind_from_extension(Path::new("photo.png")),
            AttachmentKind::Image
        );
        assert_eq!(
            kind_from_extension(Path::new("photo.JPEG")),
            AttachmentKind::Image
        );
        assert_eq!(
            kind_from_extension(Path::new("photo.heic")),
            AttachmentKind::Image
        );
        assert_eq!(
            kind_from_extension(Path::new("doc.pdf")),
            AttachmentKind::Pdf
        );
        assert_eq!(
            kind_from_extension(Path::new("readme.md")),
            AttachmentKind::Text
        );
        assert_eq!(
            kind_from_extension(Path::new("main.rs")),
            AttachmentKind::Text
        );
        assert_eq!(
            kind_from_extension(Path::new("data.json")),
            AttachmentKind::Text
        );
        assert_eq!(
            kind_from_extension(Path::new("app.exe")),
            AttachmentKind::File
        );
        assert_eq!(
            kind_from_extension(Path::new("noext")),
            AttachmentKind::File
        );
    }

    #[test]
    fn test_attachment_from_path() {
        let a = Attachment::from_path(PathBuf::from("/tmp/test.png"), AttachmentSource::Clipboard);
        assert_eq!(a.kind, AttachmentKind::Image);
        assert_eq!(a.display_name, "test.png");
        assert_eq!(a.sf_symbol(), "photo");
    }

    #[test]
    fn test_chip_label_truncation() {
        let a = Attachment::from_path(
            PathBuf::from("/tmp/very_long_filename_that_exceeds_limit.png"),
            AttachmentSource::FilePicker,
        );
        let label = a.chip_label(20);
        assert!(label.len() <= 23); // 20 chars + possible multi-byte …
        assert!(label.ends_with('…'));
    }

    #[test]
    fn test_chip_label_short() {
        let a = Attachment::from_path(PathBuf::from("/tmp/a.txt"), AttachmentSource::DragDrop);
        let label = a.chip_label(20);
        assert_eq!(label, "a.txt");
    }

    #[test]
    fn test_chip_label_zero_limit() {
        let a = Attachment::from_path(PathBuf::from("/tmp/a.txt"), AttachmentSource::DragDrop);
        let label = a.chip_label(0);
        assert_eq!(label, "");
    }

    #[test]
    fn test_chip_label_limit_one() {
        let a = Attachment::from_path(
            PathBuf::from("/tmp/longname.txt"),
            AttachmentSource::DragDrop,
        );
        let label = a.chip_label(1);
        assert_eq!(label, "…");
    }

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("hello world.txt"), "hello_world.txt");
        assert_eq!(sanitize_filename("a/b/c.rs"), "a_b_c.rs");
        assert_eq!(sanitize_filename("résumé.pdf"), "résumé.pdf");
        assert_eq!(sanitize_filename(".hidden"), "hidden");
        assert_eq!(sanitize_filename("...."), "attachment");
        let many_dots = format!("{}abc.txt", ".".repeat(120));
        assert_eq!(sanitize_filename(&many_dots), "abc.txt");
    }

    #[test]
    fn test_image_media_type() {
        assert_eq!(image_media_type(Path::new("a.png")), Some("image/png"));
        assert_eq!(image_media_type(Path::new("a.JPG")), Some("image/jpeg"));
        assert_eq!(image_media_type(Path::new("a.jpeg")), Some("image/jpeg"));
        assert_eq!(image_media_type(Path::new("a.webp")), Some("image/webp"));
        assert_eq!(image_media_type(Path::new("a.tiff")), Some("image/tiff"));
        // Not accepted as vision input despite being "images" for the UI.
        assert_eq!(image_media_type(Path::new("a.svg")), None);
        assert_eq!(image_media_type(Path::new("a.heic")), None);
        assert_eq!(image_media_type(Path::new("a.txt")), None);
        assert_eq!(image_media_type(Path::new("noext")), None);
    }

    #[test]
    fn test_parse_image_attachment_block_passthrough() {
        // No marker → text returned unchanged, no paths.
        let text = "just a normal message\nwith two lines";
        let (cleaned, paths) = parse_image_attachment_block(text);
        assert_eq!(cleaned, text);
        assert!(paths.is_empty());
    }

    #[test]
    fn test_parse_image_attachment_block_extracts_paths() {
        let text = "Look at these\n\n---\nATTACHMENTS (image paths)\n- /tmp/a.png\n- /tmp/b.jpg\n";
        let (cleaned, paths) = parse_image_attachment_block(text);
        assert_eq!(
            paths,
            vec![PathBuf::from("/tmp/a.png"), PathBuf::from("/tmp/b.jpg")]
        );
        // The marker block and the dangling separator are stripped.
        assert!(!cleaned.contains(IMAGE_PATHS_MARKER));
        assert!(!cleaned.contains("/tmp/a.png"));
        assert!(cleaned.contains("Look at these"));
        assert!(!cleaned.trim_end().ends_with("---"));
    }

    #[test]
    fn test_parse_image_attachment_block_stops_at_blank_line() {
        let text = "msg\nATTACHMENTS (image paths)\n- /tmp/a.png\n\ntrailing text kept";
        let (cleaned, paths) = parse_image_attachment_block(text);
        assert_eq!(paths, vec![PathBuf::from("/tmp/a.png")]);
        assert!(cleaned.contains("trailing text kept"));
    }

    #[test]
    fn test_load_image_for_vision_rejects_oversize_and_nonimage() {
        let dir = std::env::temp_dir().join(format!("cs_vision_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);

        let png = dir.join("small.png");
        std::fs::write(&png, b"\x89PNG\r\n\x1a\nfake").unwrap();
        let loaded = load_image_for_vision(&png, MAX_VISION_IMAGE_BYTES);
        assert!(loaded.is_some());
        let (bytes, mt) = loaded.unwrap();
        assert_eq!(mt, "image/png");
        assert!(!bytes.is_empty());

        // Oversize → rejected.
        assert!(load_image_for_vision(&png, 2).is_none());

        // Non-vision extension → rejected even if file exists.
        let txt = dir.join("note.txt");
        std::fs::write(&txt, b"hello").unwrap();
        assert!(load_image_for_vision(&txt, MAX_VISION_IMAGE_BYTES).is_none());

        // Empty (0-byte) image → rejected: an empty base64 payload would make
        // the provider reject the whole request.
        let empty = dir.join("empty.png");
        std::fs::write(&empty, b"").unwrap();
        assert!(load_image_for_vision(&empty, MAX_VISION_IMAGE_BYTES).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_paths_extraction() {
        let attachments = vec![
            Attachment::from_path(PathBuf::from("/a.txt"), AttachmentSource::Clipboard),
            Attachment::from_path(PathBuf::from("/b.png"), AttachmentSource::DragDrop),
        ];
        let paths = Attachment::paths(&attachments);
        assert_eq!(
            paths,
            vec![PathBuf::from("/a.txt"), PathBuf::from("/b.png")]
        );
    }
}
