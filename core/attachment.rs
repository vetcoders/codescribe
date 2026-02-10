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
    pub fn from_path(path: PathBuf, source: AttachmentSource) -> Self {
        let display_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());

        let size_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

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
        if self.display_name.len() <= limit {
            self.display_name.clone()
        } else {
            let truncated: String = self.display_name.chars().take(limit - 1).collect();
            format!("{truncated}…")
        }
    }

    /// Check if this attachment has the same path as another.
    pub fn same_path(&self, other: &Path) -> bool {
        self.path == other
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
        let name = format!("clipboard_{ts}.{ext}");
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
                && std::fs::remove_file(entry.path()).is_ok()
            {
                removed += 1;
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
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .chars()
        .take(100) // cap length
        .collect()
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
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("hello world.txt"), "hello_world.txt");
        assert_eq!(sanitize_filename("a/b/c.rs"), "a_b_c.rs");
        assert_eq!(sanitize_filename("résumé.pdf"), "résumé.pdf");
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
