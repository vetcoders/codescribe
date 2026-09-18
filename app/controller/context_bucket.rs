//! Captured context carried alongside a voice turn: text selections and
//! pasteboard images.
//!
//! Each capture is labelled (`selection_1`, `image_1`, …) and referenced from
//! the outgoing message rather than inlined blindly. Payloads above
//! [`DEFAULT_INLINE_LIMIT_BYTES`] spill to disk and travel as a path, so a large
//! selection cannot bloat the prompt.
//!
//! Context loss is never an acceptable failure mode: archiving keeps in-memory
//! items when the write fails, and oversized images still persist and degrade
//! honestly instead of being dropped.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use uuid::Uuid;

/// Payload size above which a capture spills to disk instead of going inline.
pub(crate) const DEFAULT_INLINE_LIMIT_BYTES: usize = 16 * 1024;

/// Reference handed back to the caller after a successful capture, so the
/// transcript can point at the stored item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContextMarker {
    /// Character position in the transcript the capture was anchored at.
    pub position: usize,
    /// Stable label (`selection_1`, `image_1`, …) used in the wire block.
    pub label: String,
}

/// How a captured selection is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SelectionPayload {
    /// Small enough to travel inside the message body.
    Inline(String),
    /// Spilled to disk; the message carries only the path.
    Path(PathBuf),
}

/// One labelled text selection.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SelectionItem {
    /// Wire label for this selection.
    label: String,
    /// Inline text or a spill path.
    payload: SelectionPayload,
}

/// How a captured pasteboard image is stored on disk for the vision path.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ImagePayload {
    /// Stored under `context/images/`; referenced via vision marker block.
    Path(PathBuf),
    /// Oversized: keep path reference for honest degrade, vision load may drop.
    OversizedPath(PathBuf),
}

/// One labelled captured image.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ImageItem {
    /// Wire label for this image.
    label: String,
    /// Durable path, flagged when it exceeded the inline limit.
    payload: ImagePayload,
}

/// Per-turn accumulator of captured selections and images.
///
/// Holds no transcript state of its own: callers add captures, then archive
/// them. Wire rendering of the `<codescribe_context>` block belongs to
/// `app/os/selection.rs`, which is the one live producer.
#[derive(Debug)]
pub(crate) struct ContextBucket {
    selections_dir: PathBuf,
    images_dir: PathBuf,
    inline_limit_bytes: usize,
    items: Vec<SelectionItem>,
    images: Vec<ImageItem>,
}

impl ContextBucket {
    /// Bucket rooted at `<data_dir>/context/`, with the default inline limit.
    pub(crate) fn for_codescribe_data_dir(data_dir: impl AsRef<Path>) -> Self {
        let root = data_dir.as_ref().join("context");
        Self::new(
            root.join("selections"),
            root.join("images"),
            DEFAULT_INLINE_LIMIT_BYTES,
        )
    }

    /// Bucket with explicit spill directories and inline limit. Directories are
    /// created lazily, on first capture that actually needs them.
    pub(crate) fn new(
        selections_dir: PathBuf,
        images_dir: PathBuf,
        inline_limit_bytes: usize,
    ) -> Self {
        Self {
            selections_dir,
            images_dir,
            inline_limit_bytes,
            items: Vec::new(),
            images: Vec::new(),
        }
    }

    /// Test helper: selections-only bucket (images dir sibling of selections).
    #[cfg(test)]
    pub(crate) fn new_selections_only(selections_dir: PathBuf, inline_limit_bytes: usize) -> Self {
        let images_dir = selections_dir
            .parent()
            .map(|p| p.join("images"))
            .unwrap_or_else(|| selections_dir.join("images"));
        Self::new(selections_dir, images_dir, inline_limit_bytes)
    }

    /// Archive the bucket's current truth under `context/archive/<stamp>-<id>/`
    /// and reset the in-memory state. Nothing is destroyed: inline selections
    /// are written out as files next to a `manifest.json`; spilled selections
    /// and images already live as durable files and are referenced by absolute
    /// path. Returns the archive dir (`None` when the bucket was empty — no
    /// empty archives). On write failure the in-memory items are KEPT and the
    /// error is returned — context loss is never an acceptable failure mode.
    pub(crate) fn archive_and_reset(&mut self, channel: &str) -> Result<Option<PathBuf>> {
        if self.items.is_empty() && self.images.is_empty() {
            return Ok(None);
        }

        let archive_root = self
            .selections_dir
            .parent()
            .map(|parent| parent.join("archive"))
            .unwrap_or_else(|| self.selections_dir.join("archive"));
        let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ");
        let short_id = Uuid::new_v4().simple().to_string();
        let dir = archive_root.join(format!("{stamp}-{}", &short_id[..8]));
        fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create context archive dir {}", dir.display()))?;

        let mut selections = Vec::with_capacity(self.items.len());
        for item in &self.items {
            match &item.payload {
                SelectionPayload::Inline(text) => {
                    let file_name = format!("{}.txt", item.label);
                    let file = dir.join(&file_name);
                    fs::write(&file, text.as_bytes()).with_context(|| {
                        format!("failed to archive inline selection to {}", file.display())
                    })?;
                    selections.push(serde_json::json!({
                        "label": item.label,
                        "kind": "inline",
                        "file": file_name,
                    }));
                }
                SelectionPayload::Path(path) => {
                    selections.push(serde_json::json!({
                        "label": item.label,
                        "kind": "spill",
                        "path": path.to_string_lossy(),
                    }));
                }
            }
        }

        let images: Vec<serde_json::Value> = self
            .images
            .iter()
            .map(|image| {
                let (path, oversized) = match &image.payload {
                    ImagePayload::Path(p) => (p, false),
                    ImagePayload::OversizedPath(p) => (p, true),
                };
                serde_json::json!({
                    "label": image.label,
                    "kind": "image",
                    "path": path.to_string_lossy(),
                    "oversized": oversized,
                })
            })
            .collect();

        let manifest = serde_json::json!({
            "schema_version": "context_archive.v1",
            "archived_at": chrono::Utc::now().to_rfc3339(),
            "channel": channel,
            "selections": selections,
            "images": images,
        });
        let manifest_path = dir.join("manifest.json");
        fs::write(
            &manifest_path,
            serde_json::to_string_pretty(&manifest)
                .context("failed to serialize archive manifest")?,
        )
        .with_context(|| format!("failed to write {}", manifest_path.display()))?;

        self.items.clear();
        self.images.clear();
        Ok(Some(dir))
    }

    /// Test helper: whether nothing at all has been captured.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty() && self.images.is_empty()
    }

    /// Test helper: number of captured images.
    #[cfg(test)]
    pub(crate) fn image_count(&self) -> usize {
        self.images.len()
    }

    /// Capture a text selection anchored at `position`.
    ///
    /// Returns `None` for whitespace-only input — an empty capture is a silent
    /// no-op, not an error. Selections above `inline_limit_bytes` (counted in
    /// UTF-8 bytes) spill to `selections_dir` and travel as a path.
    pub(crate) fn add_selection(
        &mut self,
        position: usize,
        selected_text: String,
    ) -> Result<Option<ContextMarker>> {
        let selected_text = selected_text.trim().to_string();
        if selected_text.is_empty() {
            return Ok(None);
        }

        let label = format!("selection_{}", self.items.len() + 1);
        let payload = if selected_text.len() <= self.inline_limit_bytes {
            SelectionPayload::Inline(selected_text)
        } else {
            fs::create_dir_all(&self.selections_dir).with_context(|| {
                format!(
                    "failed to create context selection directory {}",
                    self.selections_dir.display()
                )
            })?;
            let path = self
                .selections_dir
                .join(format!("{label}-{}.txt", Uuid::new_v4()));
            fs::write(&path, selected_text.as_bytes()).with_context(|| {
                format!(
                    "failed to persist oversized selection to {}",
                    path.display()
                )
            })?;
            SelectionPayload::Path(path)
        };

        self.items.push(SelectionItem {
            label: label.clone(),
            payload,
        });
        Ok(Some(ContextMarker { position, label }))
    }

    /// Capture a clipboard/pasteboard image into `context/images/` and record a
    /// vision marker path. Reuses `ATTACHMENTS (image paths)` via append.
    /// Size valve mirrors selection policy: above `inline_limit_bytes` still
    /// persists the file but marks it oversized (honest degrade, no crash).
    pub(crate) fn add_image_png(&mut self, png_bytes: &[u8]) -> Result<Option<ContextMarker>> {
        if png_bytes.is_empty() {
            return Ok(None);
        }

        fs::create_dir_all(&self.images_dir).with_context(|| {
            format!(
                "failed to create context images directory {}",
                self.images_dir.display()
            )
        })?;
        let label = format!("image_{}", self.images.len() + 1);
        let path = self
            .images_dir
            .join(format!("{label}-{}.png", Uuid::new_v4()));
        fs::write(&path, png_bytes)
            .with_context(|| format!("failed to persist context image to {}", path.display()))?;

        let payload = if png_bytes.len() <= self.inline_limit_bytes {
            ImagePayload::Path(path)
        } else {
            ImagePayload::OversizedPath(path)
        };
        self.images.push(ImageItem {
            label: label.clone(),
            payload,
        });
        Ok(Some(ContextMarker { position: 0, label }))
    }
}

/// Selection/image capture, spill, vision markers, and archive contracts.
#[cfg(test)]
mod tests {
    use super::*;

    /// Empty PNG byte slice is a silent no-op — no file, no marker.
    #[test]
    fn empty_image_is_noop() {
        let temp = tempfile::tempdir().expect("temp dir");
        let mut bucket = ContextBucket::new_selections_only(temp.path().join("selections"), 1024);
        assert_eq!(bucket.add_image_png(&[]).expect("empty"), None);
        // Assert the image lane specifically: `is_empty` also passes when a
        // selection was silently dropped instead.
        assert_eq!(bucket.image_count(), 0);
        assert!(bucket.is_empty());
    }

    /// Archive writes inline files + spill/image paths, then clears memory.
    #[test]
    fn archive_preserves_inline_and_spill_truth_then_resets() {
        let temp = tempfile::tempdir().expect("temp dir");
        let mut bucket = ContextBucket::new_selections_only(temp.path().join("selections"), 8);
        bucket
            .add_selection(3, "inline".to_string())
            .expect("inline capture");
        bucket
            .add_selection(9, "definitely oversized selection".to_string())
            .expect("spill capture");
        bucket
            .add_image_png(b"\x89PNG fake")
            .expect("image capture");

        let dir = bucket
            .archive_and_reset("paste-delivery")
            .expect("archive")
            .expect("non-empty bucket archives");

        assert!(bucket.is_empty(), "in-memory state resets after archive");
        let manifest: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(dir.join("manifest.json")).expect("manifest exists"),
        )
        .expect("manifest parses");
        assert_eq!(manifest["schema_version"], "context_archive.v1");
        assert_eq!(manifest["channel"], "paste-delivery");
        assert_eq!(
            manifest["selections"].as_array().expect("selections").len(),
            2
        );
        assert_eq!(manifest["images"].as_array().expect("images").len(), 1);

        // Inline body is reproducible from the archive itself.
        assert_eq!(
            fs::read_to_string(dir.join("selection_1.txt")).expect("archived inline body"),
            "inline"
        );
        // Spilled body stays at its durable path, referenced by the manifest.
        let spill_path = manifest["selections"][1]["path"]
            .as_str()
            .expect("spill path");
        assert_eq!(
            fs::read_to_string(spill_path).expect("spill body survives archive"),
            "definitely oversized selection"
        );
    }

    /// Empty bucket archives to `None` and creates no archive directory.
    #[test]
    fn archive_of_empty_bucket_creates_nothing() {
        let temp = tempfile::tempdir().expect("temp dir");
        let mut bucket = ContextBucket::new_selections_only(temp.path().join("selections"), 8);
        assert_eq!(
            bucket
                .archive_and_reset("session-start-discard")
                .expect("noop"),
            None
        );
        assert!(
            !temp.path().join("archive").exists(),
            "no empty archive dirs"
        );
    }
}
