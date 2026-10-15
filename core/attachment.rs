//! Attachment model and on-disk store for Codescribe.
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
use sha2::{Digest, Sha256};
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

    /// Extract paths from a slice of attachments.
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
// Vision attachment loading (typed, user-selected attachments only)
// ═══════════════════════════════════════════════════════════

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

/// Load an image file as `(bytes, media_type)` for vision input.
///
/// The path must name an attachment the user selected and the caller holds as
/// typed data (the composer's staged list); text such as a transcript never names
/// files to load. Returns `None` (with a warning) when the extension is not a
/// vision-supported image, the file cannot be opened, it is not a regular file,
/// or it is empty or longer than `max_bytes`.
///
/// One descriptor carries every decision, so the limit holds against a file that
/// grows after a check and against a device that reports length 0. It is opened
/// without following a final symlink (`O_NOFOLLOW`: the named file is the file
/// read), without blocking (`O_NONBLOCK`: a FIFO named like an image cannot stall
/// the caller) and without adopting a terminal (`O_NOCTTY`). File type and size
/// come from `fstat` on that same handle, and the read stops at `max_bytes + 1`
/// bytes, so one byte past the limit is enough to refuse.
pub fn load_image_for_vision(path: &Path, max_bytes: u64) -> Option<(Vec<u8>, String)> {
    use std::os::unix::fs::OpenOptionsExt;

    let media_type = image_media_type(path)?;

    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(path)
    {
        Ok(file) => file,
        Err(e) => {
            warn!("Failed to open image attachment {}: {}", path.display(), e);
            return None;
        }
    };

    let meta = match file.metadata() {
        Ok(meta) => meta,
        Err(e) => {
            warn!("Failed to stat image attachment {}: {}", path.display(), e);
            return None;
        }
    };
    if !meta.is_file() {
        warn!(
            "Skipping image attachment (not a regular file): {}",
            path.display()
        );
        return None;
    }
    if meta.len() > max_bytes {
        warn!(
            "Skipping image attachment (too large, {} bytes > {} max): {}",
            meta.len(),
            max_bytes,
            path.display()
        );
        return None;
    }

    let bytes = match read_capped(file, max_bytes, meta.len()) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => {
            warn!(
                "Skipping image attachment (grew past {} bytes while reading): {}",
                max_bytes,
                path.display()
            );
            return None;
        }
        Err(e) => {
            warn!("Failed to read image attachment {}: {}", path.display(), e);
            return None;
        }
    };
    if bytes.is_empty() {
        // An empty/zero-byte file would encode to an empty base64 string,
        // which providers reject ("empty base64-encoded bytes") and which
        // fails the whole request. Drop it instead.
        warn!("Skipping image attachment (empty file): {}", path.display());
        return None;
    }
    Some((bytes, media_type.to_string()))
}

/// Read at most `max_bytes` from `reader`. `Ok(None)` means the source held more:
/// the read stops one byte past the limit, so an endless or growing source costs
/// at most `max_bytes + 1` bytes of memory. `size_hint` only presizes the buffer.
fn read_capped<R: std::io::Read>(
    reader: R,
    max_bytes: u64,
    size_hint: u64,
) -> std::io::Result<Option<Vec<u8>>> {
    use std::io::Read;

    let capacity = usize::try_from(size_hint.min(max_bytes)).unwrap_or_default();
    let mut bytes = Vec::with_capacity(capacity);
    reader
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)?;
    Ok((max_bytes >= bytes.len() as u64).then_some(bytes))
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

    /// Persist pasted bytes before publishing their pointer. Content identity
    /// prevents a later paste with the same name from changing an earlier one.
    pub fn save_clipboard(data: &[u8], name: &str) -> Result<PathBuf> {
        anyhow::ensure!(
            !data.is_empty() && data.len() as u64 <= MAX_ATTACHMENT_BYTES,
            "Pasted attachment must contain 1..=50 MiB of data"
        );
        let dir = Self::store_dir();
        crate::test_isolation::assert_test_write_allowed(&dir);
        let dir = Self::ensure_dir()?;
        let hash = Sha256::digest(data)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let mut name = sanitize_filename(name);
        while name.len() > 160 {
            name.pop();
        }
        let path = dir.join(format!("clipboard_{hash}_{name}"));
        if path.exists() {
            anyhow::ensure!(std::fs::read(&path)? == data, "Stored attachment changed");
            return Ok(path);
        }
        let temporary = dir.join(format!(".paste_{:032x}.tmp", rand::random::<u128>()));
        let result = (|| -> Result<()> {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(data)?;
            file.sync_all()?;
            std::fs::rename(&temporary, &path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result?;
        Ok(path)
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

/// Kind detection, chip labels, sanitization, vision load, and path parse.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[serial_test::serial]
    fn clipboard_store_preserves_bytes_and_does_not_overwrite_same_named_pastes() {
        let sandbox = tempfile::tempdir().unwrap();
        let _data_dir = crate::test_isolation::EnvGuard::set("CODESCRIBE_DATA_DIR", sandbox.path());
        let first = AttachmentStore::save_clipboard(b"\0\xffZIP\0", "clip.bin").unwrap();
        assert_eq!(std::fs::read(&first).unwrap(), b"\0\xffZIP\0");
        assert_eq!(
            AttachmentStore::save_clipboard(b"\0\xffZIP\0", "clip.bin").unwrap(),
            first
        );
        let second = AttachmentStore::save_clipboard(b"different", "clip.bin").unwrap();
        assert_ne!(first, second);
        assert_eq!(std::fs::read(&first).unwrap(), b"\0\xffZIP\0");
        assert_eq!(std::fs::read(&second).unwrap(), b"different");
    }

    #[test]
    #[serial_test::serial]
    fn clipboard_store_bounds_names_payloads_and_detects_changed_files() {
        let sandbox = tempfile::tempdir().unwrap();
        let _data_dir = crate::test_isolation::EnvGuard::set("CODESCRIBE_DATA_DIR", sandbox.path());
        assert!(AttachmentStore::save_clipboard(b"", "empty").is_err());
        assert!(
            AttachmentStore::save_clipboard(
                &vec![0; MAX_ATTACHMENT_BYTES as usize + 1],
                "oversized.bin"
            )
            .is_err()
        );
        let name = format!("../../{}\n.txt", "ż".repeat(200));
        let path = AttachmentStore::save_clipboard(b"safe name", &name).unwrap();
        assert_eq!(path.parent(), Some(AttachmentStore::store_dir().as_path()));
        assert!(path.file_name().unwrap().len() < 256);
        assert!(!path.file_name().unwrap().to_string_lossy().contains('\n'));
        assert_eq!(std::fs::read(&path).unwrap(), b"safe name");
        let changed = AttachmentStore::save_clipboard(b"original", "changed.bin").unwrap();
        std::fs::write(&changed, b"tampered").unwrap();
        assert!(AttachmentStore::save_clipboard(b"original", "changed.bin").is_err());
        std::fs::remove_file(changed).unwrap();
    }

    /// Extension → AttachmentKind matrix including image, pdf, text, and fallback.
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

    /// from_path infers Image kind and display_name from the file basename.
    #[test]
    fn test_attachment_from_path() {
        let a = Attachment::from_path(PathBuf::from("/tmp/test.png"), AttachmentSource::Clipboard);
        assert_eq!(a.kind, AttachmentKind::Image);
        assert_eq!(a.display_name, "test.png");
        assert_eq!(a.sf_symbol(), "photo");
    }

    /// Long names truncate with an ellipsis under the chip width budget.
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

    /// Short names pass through chip_label unchanged.
    #[test]
    fn test_chip_label_short() {
        let a = Attachment::from_path(PathBuf::from("/tmp/a.txt"), AttachmentSource::DragDrop);
        let label = a.chip_label(20);
        assert_eq!(label, "a.txt");
    }

    /// Zero max width yields an empty chip label.
    #[test]
    fn test_chip_label_zero_limit() {
        let a = Attachment::from_path(PathBuf::from("/tmp/a.txt"), AttachmentSource::DragDrop);
        let label = a.chip_label(0);
        assert_eq!(label, "");
    }

    /// Width of one yields only the ellipsis character.
    #[test]
    fn test_chip_label_limit_one() {
        let a = Attachment::from_path(
            PathBuf::from("/tmp/longname.txt"),
            AttachmentSource::DragDrop,
        );
        let label = a.chip_label(1);
        assert_eq!(label, "…");
    }

    /// Sanitizer replaces path separators, strips leading dots, caps length.
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

    /// Vision MIME map accepts PNG/JPEG/WebP/TIFF; rejects SVG/HEIC/text.
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

    /// Vision load rejects oversize, non-image, and empty PNG payloads.
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

    /// A selected image must be the regular file named by the caller; aliases
    /// and a source growing beyond the byte budget cannot become vision input.
    #[test]
    fn vision_input_refuses_symlinks_and_bounds_an_endless_reader() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("image.png");
        let alias = dir.path().join("alias.png");
        std::fs::write(&image, b"image bytes").unwrap();
        std::os::unix::fs::symlink(&image, &alias).unwrap();
        assert!(load_image_for_vision(&image, 32).is_some());
        assert!(load_image_for_vision(&alias, 32).is_none());
        assert!(read_capped(std::io::repeat(7), 8, 0).unwrap().is_none());
        assert_eq!(
            read_capped(&b"exact"[..], 5, 0).unwrap(),
            Some(b"exact".to_vec())
        );
    }

    /// Attachment::paths preserves order of the input handles.
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
