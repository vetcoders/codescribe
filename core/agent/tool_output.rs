//! Durable spill for oversized tool results in conversation history.
//!
//! A single large tool result, replayed on every subsequent turn, is what turns
//! a working agent session into an unaffordable one. Above
//! [`TOOL_OUTPUT_INLINE_LIMIT_BYTES`] the text is written to a content-addressed
//! file under the config dir and the history keeps only a one-line reference.
//!
//! Nothing is destroyed: the reference names the path and the original byte
//! count, so the agent can read it back. Even the failure path stays honest —
//! if the spill cannot be written, the block says so rather than silently
//! re-feeding the payload.
//!
//! Sibling guard: `app/agent/tools/output_guard.rs` caps a single *live* tool
//! chunk in characters; this one caps *history* in bytes, because provider cost
//! tracks encoded bytes.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rand::distributions::Alphanumeric;
use rand::{Rng, thread_rng};
use sha2::{Digest, Sha256};

use super::{ContentBlock, Message};

/// Tool text larger than 64 KiB is persisted outside conversation history.
///
/// The limit is byte-based because provider payload and on-disk context costs
/// track encoded bytes, not Unicode scalar values. Settings intentionally do
/// not expose this operational safety valve.
pub const TOOL_OUTPUT_INLINE_LIMIT_BYTES: usize = 64 * 1024;

/// Content-addressed store for spilled tool output.
#[derive(Debug, Clone)]
pub(crate) struct ToolOutputStore {
    /// Directory holding the spilled `tool-output-<sha256>.txt` files.
    root: PathBuf,
    /// Byte threshold above which a result is spilled instead of kept inline.
    inline_limit_bytes: usize,
}

impl ToolOutputStore {
    /// Store rooted at `<config_dir>/context/tool_outputs` with the shipped limit.
    pub(crate) fn new() -> Self {
        Self::new_in(
            crate::config::Config::config_dir()
                .join("context")
                .join("tool_outputs"),
            TOOL_OUTPUT_INLINE_LIMIT_BYTES,
        )
    }

    /// Store with an explicit root and threshold — the seam the tests drive.
    pub(crate) fn new_in(root: PathBuf, inline_limit_bytes: usize) -> Self {
        Self {
            root,
            inline_limit_bytes,
        }
    }

    /// Replace oversized textual tool results with one durable reference.
    /// Non-text blocks (for example disk-backed images) remain in place.
    pub(crate) fn spill_content(&self, content: &mut Vec<ContentBlock>) -> Result<bool> {
        let size_bytes = content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text.len()),
                _ => None,
            })
            .sum::<usize>();
        if size_bytes <= self.inline_limit_bytes {
            return Ok(false);
        }

        let body = content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        let persisted = self.persist(body.as_bytes());
        let reference = match &persisted {
            Ok(path) => ContentBlock::Text(format!(
                "[tool output stored: {} ({} bytes)]",
                path.display(),
                body.len()
            )),
            Err(error) => ContentBlock::Text(format!(
                "[tool output omitted: failed to store {} bytes: {error}]",
                body.len()
            )),
        };

        let first_text = content
            .iter()
            .position(|block| matches!(block, ContentBlock::Text(_)))
            .unwrap_or(0);
        let mut replacement = Vec::with_capacity(content.len());
        for (index, block) in content.drain(..).enumerate() {
            match block {
                ContentBlock::Text(_) if index == first_text => replacement.push(reference.clone()),
                ContentBlock::Text(_) => {}
                other => replacement.push(other),
            }
        }
        *content = replacement;
        persisted.map(|_| true)
    }

    /// Rehydration is a full-history replay boundary. Normalizing here keeps
    /// legacy inline tool dumps from re-inflating after degraded-runtime
    /// recovery, while already-stored references remain unchanged.
    pub(crate) fn spill_messages(&self, messages: &mut [Message]) -> Result<usize> {
        let mut spilled = 0;
        let mut first_error = None;
        for message in messages {
            for block in &mut message.content {
                if let ContentBlock::ToolResult { content, .. } = block {
                    match self.spill_content(content) {
                        Ok(true) => spilled += 1,
                        Ok(false) => {}
                        Err(error) => {
                            first_error.get_or_insert(error);
                        }
                    }
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(spilled),
        }
    }

    /// Write `body` to a file named by its SHA-256 and return the path.
    ///
    /// Content-addressed, so an identical payload spilled twice reuses the
    /// existing file instead of writing a duplicate.
    fn persist(&self, body: &[u8]) -> Result<PathBuf> {
        fs::create_dir_all(&self.root).with_context(|| {
            format!(
                "Failed to create tool-output context dir {}",
                self.root.display()
            )
        })?;

        let hash = Sha256::digest(body)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let path = self.root.join(format!("tool-output-{hash}.txt"));
        if path.exists() {
            return Ok(path);
        }

        atomic_write_private(&path, body)?;
        Ok(path)
    }
}

/// Write `body` to `path` atomically, owner-readable only.
///
/// Goes through a randomly-suffixed temp file created with `create_new` (mode
/// `0600` on Unix), synced, then renamed — so a concurrent reader never sees a
/// partial file, and tool output never lands world-readable.
fn atomic_write_private(path: &Path, body: &[u8]) -> Result<()> {
    let suffix = thread_rng()
        .sample_iter(&Alphanumeric)
        .take(8)
        .map(char::from)
        .collect::<String>()
        .to_ascii_lowercase();
    let tmp = path.with_extension(format!("{suffix}.tmp"));

    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&tmp)
        .with_context(|| format!("Failed to create temporary tool output {}", tmp.display()))?;
    file.write_all(body)
        .with_context(|| format!("Failed to write temporary tool output {}", tmp.display()))?;
    file.sync_all()
        .with_context(|| format!("Failed to sync temporary tool output {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| {
        format!(
            "Failed to store tool output {} -> {}",
            tmp.display(),
            path.display()
        )
    })?;
    Ok(())
}

/// Spill threshold, under-threshold passthrough, and storage-failure honesty.
#[cfg(test)]
mod tests {
    use super::*;

    /// Oversized text is written to disk and replaced by a path reference only.
    #[test]
    fn oversized_output_is_written_and_replaced_without_inline_body() {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path().join("tool_outputs");
        let store = ToolOutputStore::new_in(root.clone(), 8);
        let mut content = vec![ContentBlock::Text("monster-output".to_string())];

        assert!(store.spill_content(&mut content).expect("spill output"));

        let reference = match &content[0] {
            ContentBlock::Text(text) => text,
            block => panic!("expected text reference, got {block:?}"),
        };
        assert!(reference.starts_with("[tool output stored: "));
        assert!(reference.contains("(14 bytes)]"));
        assert!(!reference.contains("monster-output"));

        let files = fs::read_dir(&root)
            .expect("overflow directory")
            .collect::<std::result::Result<Vec<_>, _>>()
            .expect("overflow entries");
        assert_eq!(files.len(), 1);
        assert_eq!(
            fs::read_to_string(files[0].path()).unwrap(),
            "monster-output"
        );
    }

    /// Small results stay inline and must not create the spill directory.
    #[test]
    fn under_threshold_output_stays_inline_and_does_not_touch_disk() {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path().join("tool_outputs");
        let store = ToolOutputStore::new_in(root.clone(), 8);
        let original = vec![ContentBlock::Text("small".to_string())];
        let mut content = original.clone();

        assert!(!store.spill_content(&mut content).expect("inline output"));
        assert_eq!(content, original);
        assert!(!root.exists());
    }

    /// On spill failure, history gets an omit marker — never the raw body again.
    #[test]
    fn storage_failure_omits_oversized_body_instead_of_refeeding_it() {
        let temp = tempfile::tempdir().expect("temp dir");
        let blocked = temp.path().join("not-a-directory");
        fs::write(&blocked, "file").expect("blocking file");
        let store = ToolOutputStore::new_in(blocked.join("tool_outputs"), 8);
        let mut content = vec![ContentBlock::Text("monster-output".to_string())];

        assert!(store.spill_content(&mut content).is_err());

        let marker = match &content[0] {
            ContentBlock::Text(text) => text,
            block => panic!("expected omission marker, got {block:?}"),
        };
        assert!(marker.starts_with("[tool output omitted: failed to store 14 bytes:"));
        assert!(!marker.contains("monster-output"));
    }
}
