use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use uuid::Uuid;

pub(crate) const DEFAULT_INLINE_LIMIT_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContextMarker {
    pub position: usize,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SelectionPayload {
    Inline(String),
    Path(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SelectionItem {
    label: String,
    payload: SelectionPayload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ImagePayload {
    /// Stored under `context/images/`; referenced via vision marker block.
    Path(PathBuf),
    /// Oversized: keep path reference for honest degrade, vision load may drop.
    OversizedPath(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ImageItem {
    label: String,
    payload: ImagePayload,
}

#[derive(Debug)]
pub(crate) struct ContextBucket {
    selections_dir: PathBuf,
    images_dir: PathBuf,
    inline_limit_bytes: usize,
    items: Vec<SelectionItem>,
    images: Vec<ImageItem>,
}

impl ContextBucket {
    pub(crate) fn for_codescribe_data_dir(data_dir: impl AsRef<Path>) -> Self {
        let root = data_dir.as_ref().join("context");
        Self::new(
            root.join("selections"),
            root.join("images"),
            DEFAULT_INLINE_LIMIT_BYTES,
        )
    }

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

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty() && self.images.is_empty()
    }

    pub(crate) fn has_selection_items(&self) -> bool {
        !self.items.is_empty()
    }

    /// Number of captured selections — feeds the wire header's honest
    /// `carried in <codescribe_context> (N selections)` count.
    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    #[cfg(test)]
    pub(crate) fn image_count(&self) -> usize {
        self.images.len()
    }

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

    pub(crate) fn append_to_message(&self, message: &str) -> String {
        if self.items.is_empty() && self.images.is_empty() {
            return message.to_string();
        }

        let message = message.trim_end();
        let mut out = String::with_capacity(message.len() + 128);
        out.push_str(message);

        if !self.items.is_empty() {
            out.push_str("\n\n<codescribe_context>\n");
            for item in &self.items {
                out.push('<');
                out.push_str(&item.label);
                out.push_str(">\n");
                match &item.payload {
                    SelectionPayload::Inline(text) => out.push_str(text),
                    SelectionPayload::Path(path) => {
                        out.push_str("PATH: ");
                        out.push_str(&path.to_string_lossy());
                    }
                }
                out.push_str("\n</");
                out.push_str(&item.label);
                out.push_str(">\n");
            }
            out.push_str("</codescribe_context>");
        }

        // Vision marker block consumed by build_image_attachments_from_text.
        if !self.images.is_empty() {
            out.push_str("\n\n---\n");
            out.push_str(codescribe_core::attachment::IMAGE_PATHS_MARKER);
            out.push('\n');
            for image in &self.images {
                let path = match &image.payload {
                    ImagePayload::Path(p) | ImagePayload::OversizedPath(p) => p,
                };
                out.push_str("- ");
                out.push_str(&path.to_string_lossy());
                out.push('\n');
            }
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_selections_keep_order_and_explicit_tags() {
        let temp = tempfile::tempdir().expect("temp dir");
        let mut bucket = ContextBucket::new_selections_only(temp.path().join("selections"), 1024);

        for (position, text) in [(5, "alpha"), (11, "beta"), (17, "gamma")] {
            bucket
                .add_selection(position, text.to_string())
                .expect("selection capture");
        }

        assert_eq!(bucket.len(), 3);
        assert_eq!(
            bucket.append_to_message("say {selection_1} then {selection_2} and {selection_3}"),
            "say {selection_1} then {selection_2} and {selection_3}\n\n\
<codescribe_context>\n\
<selection_1>\nalpha\n</selection_1>\n\
<selection_2>\nbeta\n</selection_2>\n\
<selection_3>\ngamma\n</selection_3>\n\
</codescribe_context>"
        );
    }

    #[test]
    fn oversized_selection_is_persisted_and_message_contains_path_only() {
        let temp = tempfile::tempdir().expect("temp dir");
        let mut bucket = ContextBucket::new_selections_only(temp.path().join("selections"), 4);
        let original = "five bytes and more";

        let marker = bucket
            .add_selection(0, original.to_string())
            .expect("selection capture")
            .expect("marker");
        let message = bucket.append_to_message(&marker.label);
        let path_line = message
            .lines()
            .find_map(|line| line.strip_prefix("PATH: "))
            .expect("persisted path");

        assert!(Path::new(path_line).is_file());
        assert_eq!(
            fs::read_to_string(path_line).expect("persisted body"),
            original
        );
        assert!(!message.contains(original));
        assert!(message.contains("<selection_1>"));
    }

    #[test]
    fn byte_limit_counts_utf8_bytes() {
        let temp = tempfile::tempdir().expect("temp dir");
        let mut bucket = ContextBucket::new_selections_only(temp.path().join("selections"), 3);

        bucket
            .add_selection(0, "żż".to_string())
            .expect("selection capture");
        let message = bucket.append_to_message("voice");

        assert!(message.contains("PATH: "), "four UTF-8 bytes exceed limit");
        assert!(!message.contains("żż"));
    }

    #[test]
    fn empty_selection_is_a_silent_noop() {
        let temp = tempfile::tempdir().expect("temp dir");
        let mut bucket = ContextBucket::new_selections_only(temp.path().join("selections"), 4);

        assert_eq!(
            bucket
                .add_selection(0, "  \n".to_string())
                .expect("no-op capture"),
            None
        );
        assert!(bucket.is_empty());
        assert_eq!(bucket.append_to_message("voice"), "voice");
    }

    #[test]
    fn image_capture_stores_file_and_emits_vision_marker() {
        let temp = tempfile::tempdir().expect("temp dir");
        let mut bucket = ContextBucket::new_selections_only(temp.path().join("selections"), 1024);
        let png = b"\x89PNG\r\n\x1a\nfake-image";
        let marker = bucket
            .add_image_png(png)
            .expect("image capture")
            .expect("marker");
        assert_eq!(marker.label, "image_1");
        assert_eq!(bucket.image_count(), 1);
        let message = bucket.append_to_message("describe this");
        assert!(message.contains(codescribe_core::attachment::IMAGE_PATHS_MARKER));
        assert!(message.contains("image_1-"));
        assert!(message.contains(".png"));
        // Path on disk under context/images/
        let images_dir = temp.path().join("images");
        assert!(images_dir.is_dir());
        let entries: Vec<_> = std::fs::read_dir(&images_dir)
            .expect("list images")
            .collect();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn oversized_image_still_persists_and_appends_path() {
        let temp = tempfile::tempdir().expect("temp dir");
        let mut bucket = ContextBucket::new_selections_only(temp.path().join("selections"), 4);
        let big = vec![0u8; 32];
        bucket.add_image_png(&big).expect("oversized image");
        let message = bucket.append_to_message("see");
        assert!(message.contains(codescribe_core::attachment::IMAGE_PATHS_MARKER));
        assert!(message.contains("- "));
        assert_eq!(bucket.image_count(), 1);
    }

    #[test]
    fn empty_image_is_noop() {
        let temp = tempfile::tempdir().expect("temp dir");
        let mut bucket = ContextBucket::new_selections_only(temp.path().join("selections"), 1024);
        assert_eq!(bucket.add_image_png(&[]).expect("empty"), None);
        assert!(bucket.is_empty());
    }

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
