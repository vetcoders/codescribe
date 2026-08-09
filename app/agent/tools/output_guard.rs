//! Input-side bloat guard for agent tool results (operator doctrine, 2026-08-05):
//! the agent's OUTPUT is never capped — protection lives on the INPUT side, and a
//! guard must never destroy knowledge. A single tool chunk above ~25 KB is cut,
//! but the full content is first spilled to a Codescribe-owned file (inside
//! `Config::config_dir()`, which is an allowed `read_file` root) and the chunk
//! ends with a pointer to that file, so the agent can always follow up instead
//! of silently losing the tail.

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use codescribe_core::config::Config;

/// Maximum characters a single tool-result chunk may occupy in the agent's
/// context before the spill-and-point guard engages (~25 KB of ASCII).
pub(crate) const MAX_CHUNK_CHARS: usize = 25_000;

/// Guard a generated tool output (process stdout, git output, search JSON) that
/// has no on-disk source of its own. Oversized text is spilled to a file and the
/// returned chunk carries a pointer to it.
pub(crate) fn guard_chunk(text: &str, origin: &str) -> String {
    guard_chunk_into(text, origin, &spill_dir())
}

/// Guard content that already lives on disk (e.g. `read_file`): no spill needed,
/// the pointer is the source path itself plus how much of it was shown.
pub(crate) fn truncate_with_source(text: &str, source: &Path) -> String {
    let total = text.chars().count();
    if total <= MAX_CHUNK_CHARS {
        return text.to_string();
    }
    let mut out: String = text.chars().take(MAX_CHUNK_CHARS).collect();
    out.push_str(&format!(
        "\n…[truncated: showing first {MAX_CHUNK_CHARS} of {total} chars from {} — \
         the file continues on disk; use process.run (e.g. sed/rg with line ranges) to read the rest]",
        source.display()
    ));
    out
}

/// [`guard_chunk`] against an explicit spill directory — the testable seam.
fn guard_chunk_into(text: &str, origin: &str, spill_dir: &Path) -> String {
    let total = text.chars().count();
    if total <= MAX_CHUNK_CHARS {
        return text.to_string();
    }
    let mut out: String = text.chars().take(MAX_CHUNK_CHARS).collect();
    match spill(text, origin, spill_dir) {
        Ok(path) => out.push_str(&format!(
            "\n…[truncated: showing first {MAX_CHUNK_CHARS} of {total} chars — full {origin} \
             output saved to {} — use read_file or process.run on that path for the rest]",
            path.display()
        )),
        // The guard protects context, never the tool call itself: if the spill
        // cannot be written, stay honest about what was lost instead of failing.
        Err(_) => out.push_str(&format!(
            "\n…[truncated: showing first {MAX_CHUNK_CHARS} of {total} chars of {origin} output; \
             the remainder could not be preserved — re-run with a narrower scope if it matters]"
        )),
    }
    out
}

/// Write the full `text` to a new file in `dir` and return its path.
///
/// The name carries a millisecond timestamp, a sanitised `origin` slug, and a
/// content hash, so concurrent spills from the same tool cannot collide.
fn spill(text: &str, origin: &str, dir: &Path) -> std::io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    let slug: String = origin
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(24)
        .collect();
    let path = dir.join(format!(
        "{millis}-{slug}-{:08x}.txt",
        hasher.finish() as u32
    ));
    fs::write(&path, text)?;
    Ok(path)
}

/// Spill location: `<config_dir>/agent/spill`, inside an allowed `read_file` root
/// so the agent can follow the pointer it was given.
fn spill_dir() -> PathBuf {
    Config::config_dir().join("agent").join("spill")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn small_chunk_passes_untouched() {
        let dir = TempDir::new().expect("tempdir");
        let text = "short output";
        assert_eq!(guard_chunk_into(text, "process", dir.path()), text);
        assert!(
            fs::read_dir(dir.path())
                .map(|mut d| d.next().is_none())
                .unwrap_or(true),
            "no spill file may be created for small chunks"
        );
    }

    #[test]
    fn oversized_chunk_is_cut_but_knowledge_survives_via_spill_pointer() {
        let dir = TempDir::new().expect("tempdir");
        let text = "x".repeat(MAX_CHUNK_CHARS + 5_000);
        let guarded = guard_chunk_into(&text, "process `cargo test`", dir.path());

        assert!(guarded.len() < text.len(), "oversized chunk must be cut");
        assert!(
            guarded.contains("…[truncated"),
            "cut must be visible, never silent"
        );

        // The pointer names a real file holding the COMPLETE original output.
        let spilled: Vec<_> = fs::read_dir(dir.path())
            .expect("spill dir")
            .filter_map(Result::ok)
            .collect();
        assert_eq!(spilled.len(), 1, "exactly one spill file");
        let spill_path = spilled[0].path();
        assert!(
            guarded.contains(&spill_path.display().to_string()),
            "chunk must point at the spill file"
        );
        assert_eq!(
            fs::read_to_string(&spill_path).expect("read spill"),
            text,
            "spill must hold the full, unabridged output"
        );
    }

    #[test]
    fn on_disk_source_gets_a_path_pointer_not_a_spill() {
        let source = Path::new("/workspace/big.log");
        let text = "y".repeat(MAX_CHUNK_CHARS + 1);
        let guarded = truncate_with_source(&text, source);
        assert!(
            guarded.contains("/workspace/big.log"),
            "pointer must name the source"
        );
        assert!(guarded.contains("…[truncated"), "cut must be visible");

        let small = truncate_with_source("tiny", source);
        assert_eq!(small, "tiny");
    }
}
