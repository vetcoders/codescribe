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

#[derive(Debug)]
pub(crate) struct ContextBucket {
    selections_dir: PathBuf,
    inline_limit_bytes: usize,
    items: Vec<SelectionItem>,
}

impl ContextBucket {
    pub(crate) fn for_codescribe_data_dir(data_dir: impl AsRef<Path>) -> Self {
        Self::new(
            data_dir.as_ref().join("context").join("selections"),
            DEFAULT_INLINE_LIMIT_BYTES,
        )
    }

    pub(crate) fn new(selections_dir: PathBuf, inline_limit_bytes: usize) -> Self {
        Self {
            selections_dir,
            inline_limit_bytes,
            items: Vec::new(),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.items.clear();
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.items.len()
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

    pub(crate) fn append_to_message(&self, message: &str) -> String {
        if self.items.is_empty() {
            return message.to_string();
        }

        let message = message.trim_end();
        let mut out = String::with_capacity(message.len() + 128);
        out.push_str(message);
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
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_selections_keep_order_and_explicit_tags() {
        let temp = tempfile::tempdir().expect("temp dir");
        let mut bucket = ContextBucket::new(temp.path().join("selections"), 1024);

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
        let mut bucket = ContextBucket::new(temp.path().join("selections"), 4);
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
        let mut bucket = ContextBucket::new(temp.path().join("selections"), 3);

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
        let mut bucket = ContextBucket::new(temp.path().join("selections"), 4);

        assert_eq!(
            bucket
                .add_selection(0, "  \n".to_string())
                .expect("no-op capture"),
            None
        );
        assert!(bucket.is_empty());
        assert_eq!(bucket.append_to_message("voice"), "voice");
    }
}
