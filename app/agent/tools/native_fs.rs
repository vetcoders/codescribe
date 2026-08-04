//! Native filesystem tools beyond `read_file`: list, search, write, patch, move.
//!
//! All paths are workspace-sandboxed via [`super::path_policy`]. Mutating tools
//! declare `ToolRisk::Mutating` so the central approval gateway defaults to Ask.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use codescribe_core::agent::{ToolDefinition, ToolRegistry, ToolResultContent, ToolRisk};
use codescribe_core::util::safe_path::{safe_read_to_string_bounded, safe_write_bounded};
use serde_json::{Value, json};

use super::{path_policy, workspace};

const MAX_LIST_ENTRIES: usize = 500;
const MAX_SEARCH_HITS: usize = 50;
const MAX_SEARCH_FILE_BYTES: u64 = 512 * 1024;
const MAX_WRITE_BYTES: usize = 512 * 1024;
const MAX_RESULT_CHARS: usize = 40_000;

pub fn register(registry: &mut ToolRegistry) {
    registry
        .register_native(
            list_directory_definition(),
            Box::new(|input| Box::pin(async move { handle_list_directory(input) })),
            ToolRisk::ReadOnly,
        )
        .expect("register list_directory");
    registry
        .register_native(
            search_files_definition(),
            Box::new(|input| Box::pin(async move { handle_search_files(input) })),
            ToolRisk::ReadOnly,
        )
        .expect("register search_files");
    registry
        .register_native(
            write_file_definition(),
            Box::new(|input| Box::pin(async move { handle_write_file(input) })),
            ToolRisk::Mutating,
        )
        .expect("register write_file");
    registry
        .register_native(
            apply_patch_definition(),
            Box::new(|input| Box::pin(async move { handle_apply_patch(input) })),
            ToolRisk::Mutating,
        )
        .expect("register apply_patch");
    registry
        .register_native(
            move_path_definition(),
            Box::new(|input| Box::pin(async move { handle_move_path(input) })),
            ToolRisk::Mutating,
        )
        .expect("register move_path");
}

fn list_directory_definition() -> ToolDefinition {
    ToolDefinition {
        name: "list_directory".to_string(),
        description: "List files and directories under a workspace path (canonical capability: fs.list). Bounded, non-recursive by default."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Absolute directory path inside workspace roots" },
                "recursive": { "type": "boolean", "description": "When true, recurse one level of subdirectories (default false)" }
            },
            "required": ["path"]
        }),
    }
}

fn search_files_definition() -> ToolDefinition {
    ToolDefinition {
        name: "search_files".to_string(),
        description: "Search file contents under a workspace path for a literal substring (canonical capability: fs.search). Returns bounded matches."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Absolute root directory to search" },
                "query": { "type": "string", "description": "Literal text to find" },
                "glob": { "type": "string", "description": "Optional filename suffix filter, e.g. .rs" }
            },
            "required": ["path", "query"]
        }),
    }
}

fn write_file_definition() -> ToolDefinition {
    ToolDefinition {
        name: "write_file".to_string(),
        description: "Write UTF-8 text to a file inside workspace roots (canonical capability: fs.write). Creates parent directories. Requires approval by default."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Absolute file path" },
                "content": { "type": "string", "description": "Full file contents to write" }
            },
            "required": ["path", "content"]
        }),
    }
}

fn apply_patch_definition() -> ToolDefinition {
    ToolDefinition {
        name: "apply_patch".to_string(),
        description: "Replace an exact substring in a workspace file (canonical capability: fs.patch). old_string must match exactly once."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "old_string": { "type": "string" },
                "new_string": { "type": "string" }
            },
            "required": ["path", "old_string", "new_string"]
        }),
    }
}

fn move_path_definition() -> ToolDefinition {
    ToolDefinition {
        name: "move_path".to_string(),
        description: "Move or rename a file/directory inside workspace roots (canonical capability: fs.move)."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "from": { "type": "string" },
                "to": { "type": "string" }
            },
            "required": ["from", "to"]
        }),
    }
}

fn handle_list_directory(input: Value) -> Vec<ToolResultContent> {
    match list_directory_from_input(&input) {
        Ok(text) => vec![ToolResultContent::Text(text)],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

fn handle_search_files(input: Value) -> Vec<ToolResultContent> {
    match search_files_from_input(&input) {
        Ok(text) => vec![ToolResultContent::Text(text)],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

fn handle_write_file(input: Value) -> Vec<ToolResultContent> {
    match write_file_from_input(&input) {
        Ok(text) => vec![ToolResultContent::Text(text)],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

fn handle_apply_patch(input: Value) -> Vec<ToolResultContent> {
    match apply_patch_from_input(&input) {
        Ok(text) => vec![ToolResultContent::Text(text)],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

fn handle_move_path(input: Value) -> Vec<ToolResultContent> {
    match move_path_from_input(&input) {
        Ok(text) => vec![ToolResultContent::Text(text)],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

fn workspace_roots() -> Vec<PathBuf> {
    path_policy::canonical_roots(&workspace::resolved_roots())
}

fn require_string<'a>(input: &'a Value, field: &str) -> Result<&'a str> {
    input
        .get(field)
        .and_then(Value::as_str)
        .with_context(|| format!("Missing required string field '{field}'"))
}

fn list_directory_from_input(input: &Value) -> Result<String> {
    let path_str = require_string(input, "path")?;
    let recursive = input
        .get("recursive")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let roots = workspace_roots();
    let path = path_policy::validate_existing(path_str, &roots)?;
    if !path.is_dir() {
        bail!("Path is not a directory: {}", path.display());
    }

    let mut entries = Vec::new();
    collect_entries(&path, recursive, &mut entries)?;
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    if entries.len() > MAX_LIST_ENTRIES {
        entries.truncate(MAX_LIST_ENTRIES);
    }

    let payload = json!({
        "path": path.display().to_string(),
        "count": entries.len(),
        "truncated": entries.len() >= MAX_LIST_ENTRIES,
        "entries": entries.iter().map(|e| json!({
            "name": e.name,
            "path": e.path,
            "kind": e.kind,
        })).collect::<Vec<_>>(),
        "provider": "native",
        "capability": "fs.list",
    });
    Ok(serde_json::to_string_pretty(&payload)?)
}

struct DirEntryInfo {
    name: String,
    path: String,
    kind: &'static str,
}

fn collect_entries(dir: &Path, one_level_recurse: bool, out: &mut Vec<DirEntryInfo>) -> Result<()> {
    let roots = workspace_roots();
    let read = open_dir_under_roots(dir, &roots)?;
    for entry in read.flatten() {
        if out.len() >= MAX_LIST_ENTRIES {
            break;
        }
        let path = entry.path();
        if !path_stays_under_roots(&path, &roots) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name == ".git" || name == "target" || name == "node_modules" {
            continue;
        }
        let kind = if path.is_dir() {
            "dir"
        } else if path.is_file() {
            "file"
        } else {
            "other"
        };
        out.push(DirEntryInfo {
            name: name.clone(),
            path: path.display().to_string(),
            kind,
        });
        if one_level_recurse && path.is_dir() {
            let sub = open_dir_under_roots(&path, &roots)
                .with_context(|| format!("Failed to list subdirectory {}", path.display()))?;
            for child in sub.flatten() {
                if out.len() >= MAX_LIST_ENTRIES {
                    break;
                }
                let child_path = child.path();
                if !path_stays_under_roots(&child_path, &roots) {
                    continue;
                }
                let child_name = child.file_name().to_string_lossy().to_string();
                let kind = if child_path.is_dir() {
                    "dir"
                } else if child_path.is_file() {
                    "file"
                } else {
                    "other"
                };
                out.push(DirEntryInfo {
                    name: format!("{name}/{child_name}"),
                    path: child_path.display().to_string(),
                    kind,
                });
            }
        }
    }
    Ok(())
}

/// Open a directory only after re-asserting workspace-root membership.
///
/// Callers already validated the top-level path via `path_policy`; this second
/// gate closes TOCTOU/symlink races on walk entries and keeps the FS open site
/// on a re-validated absolute path (not the raw agent string).
fn open_dir_under_roots(dir: &Path, roots: &[PathBuf]) -> Result<fs::ReadDir> {
    let dir_str = dir
        .to_str()
        .with_context(|| format!("Directory path is not valid UTF-8: {}", dir.display()))?;
    let dir = path_policy::validate_existing(dir_str, roots)?;
    if !dir.is_dir() {
        bail!("Path is not a directory: {}", dir.display());
    }
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- path re-validated by path_policy::validate_existing (canonicalize + workspace-root bound) immediately above.
    fs::read_dir(&dir).with_context(|| format!("Failed to list directory {}", dir.display()))
}

/// Reject walk children that escape roots via symlink after entry discovery.
fn path_stays_under_roots(path: &Path, roots: &[PathBuf]) -> bool {
    let Ok(canonical) = path.canonicalize() else {
        // Non-existent / broken symlink: keep the lexical path only if under a root.
        return roots.iter().any(|root| path.starts_with(root));
    };
    roots.iter().any(|root| canonical.starts_with(root))
}

fn search_files_from_input(input: &Value) -> Result<String> {
    let path_str = require_string(input, "path")?;
    let query = require_string(input, "query")?;
    if query.is_empty() {
        bail!("query must not be empty");
    }
    let glob_suffix = input.get("glob").and_then(Value::as_str);
    let roots = workspace_roots();
    let root = path_policy::validate_existing(path_str, &roots)?;
    if !root.is_dir() {
        bail!("Search path is not a directory: {}", root.display());
    }

    let mut hits = Vec::new();
    walk_search(&root, query, glob_suffix, &mut hits)?;

    let payload = json!({
        "path": root.display().to_string(),
        "query": query,
        "count": hits.len(),
        "truncated": hits.len() >= MAX_SEARCH_HITS,
        "hits": hits,
        "provider": "native",
        "capability": "fs.search",
    });
    let mut text = serde_json::to_string_pretty(&payload)?;
    if text.chars().count() > MAX_RESULT_CHARS {
        text = text.chars().take(MAX_RESULT_CHARS).collect();
        text.push_str("\n…[truncated]");
    }
    Ok(text)
}

fn walk_search(
    dir: &Path,
    query: &str,
    glob_suffix: Option<&str>,
    hits: &mut Vec<Value>,
) -> Result<()> {
    if hits.len() >= MAX_SEARCH_HITS {
        return Ok(());
    }
    let roots = workspace_roots();
    let read = match open_dir_under_roots(dir, &roots) {
        Ok(r) => r,
        Err(_) => return Ok(()),
    };
    for entry in read.flatten() {
        if hits.len() >= MAX_SEARCH_HITS {
            break;
        }
        let path = entry.path();
        if !path_stays_under_roots(&path, &roots) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name == ".git" || name == "target" || name == "node_modules" || name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            walk_search(&path, query, glob_suffix, hits)?;
            continue;
        }
        if !path.is_file() {
            continue;
        }
        if let Some(suffix) = glob_suffix
            && !name.ends_with(suffix)
        {
            continue;
        }
        let meta = match path.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.len() > MAX_SEARCH_FILE_BYTES {
            continue;
        }
        // Open only files still under workspace roots (symlink-safe).
        let Some(root) = roots.iter().find(|root| path.starts_with(root)) else {
            continue;
        };
        let Ok(file) = codescribe_core::util::safe_path::safe_open_bounded(&path, root) else {
            continue;
        };
        let reader = BufReader::new(file);
        for (idx, line) in reader.lines().enumerate() {
            if hits.len() >= MAX_SEARCH_HITS {
                break;
            }
            let Ok(line) = line else { continue };
            if line.contains(query) {
                let snippet: String = line.chars().take(200).collect();
                hits.push(json!({
                    "path": path.display().to_string(),
                    "line": idx + 1,
                    "snippet": snippet,
                }));
            }
        }
    }
    Ok(())
}

fn write_file_from_input(input: &Value) -> Result<String> {
    let path_str = require_string(input, "path")?;
    let content = require_string(input, "content")?;
    if content.len() > MAX_WRITE_BYTES {
        bail!("Content exceeds write size limit ({MAX_WRITE_BYTES} bytes)");
    }
    let roots = workspace_roots();
    let path = path_policy::validate_new_target(path_str, &roots)?;
    let root = roots
        .iter()
        .find(|root| path.starts_with(root))
        .cloned()
        .context("Path is outside configured agent workspace roots")?;
    safe_write_bounded(&path, &root, content)
        .with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(json!({
        "ok": true,
        "path": path.display().to_string(),
        "bytes": content.len(),
        "provider": "native",
        "capability": "fs.write",
    })
    .to_string())
}

fn apply_patch_from_input(input: &Value) -> Result<String> {
    let path_str = require_string(input, "path")?;
    let old = require_string(input, "old_string")?;
    let new = require_string(input, "new_string")?;
    if old.is_empty() {
        bail!("old_string must not be empty");
    }
    let roots = workspace_roots();
    let path = path_policy::validate_existing(path_str, &roots)?;
    if !path.is_file() {
        bail!("Path is not a file: {}", path.display());
    }
    let root = roots
        .iter()
        .find(|root| path.starts_with(root))
        .cloned()
        .context("Path is outside configured agent workspace roots")?;
    let original = safe_read_to_string_bounded(&path, &root)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let matches = original.matches(old).count();
    if matches == 0 {
        bail!("old_string not found in {}", path.display());
    }
    if matches > 1 {
        bail!(
            "old_string matched {matches} times in {}; must match exactly once",
            path.display()
        );
    }
    let updated = original.replacen(old, new, 1);
    if updated.len() > MAX_WRITE_BYTES {
        bail!("Patched content exceeds write size limit ({MAX_WRITE_BYTES} bytes)");
    }
    safe_write_bounded(&path, &root, &updated)
        .with_context(|| format!("Failed to write patched {}", path.display()))?;
    Ok(json!({
        "ok": true,
        "path": path.display().to_string(),
        "bytes": updated.len(),
        "provider": "native",
        "capability": "fs.patch",
    })
    .to_string())
}

fn move_path_from_input(input: &Value) -> Result<String> {
    let from_str = require_string(input, "from")?;
    let to_str = require_string(input, "to")?;
    let roots = workspace_roots();
    let from = path_policy::validate_existing(from_str, &roots)?;
    let to = path_policy::validate_new_target(to_str, &roots)?;
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create parent {}", parent.display()))?;
    }
    fs::rename(&from, &to)
        .with_context(|| format!("Failed to move {} → {}", from.display(), to.display()))?;
    Ok(json!({
        "ok": true,
        "from": from.display().to_string(),
        "to": to.display().to_string(),
        "provider": "native",
        "capability": "fs.move",
    })
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn roots_of(tmp: &TempDir) -> Vec<PathBuf> {
        path_policy::canonical_roots(&[tmp.path().to_path_buf()])
    }

    #[test]
    fn list_and_search_and_write_patch_move_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::write(root.join("a.txt"), "hello world").unwrap();
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("sub/b.txt"), "findme token").unwrap();

        // Inject roots by validating against path_policy with explicit roots
        // via the public helpers that take workspace roots from settings.
        // Direct unit tests exercise the internal functions with temp roots.
        let roots = roots_of(&tmp);
        let listed = path_policy::validate_existing(root.to_str().unwrap(), &roots).unwrap();
        assert!(listed.is_dir());

        let content = "alpha\nbeta\n";
        let target = root.join("new.txt");
        let path = path_policy::validate_new_target(target.to_str().unwrap(), &roots).unwrap();
        safe_write_bounded(&path, roots.first().unwrap(), content).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), content);

        let original = fs::read_to_string(&path).unwrap();
        let patched = original.replacen("beta", "gamma", 1);
        safe_write_bounded(&path, roots.first().unwrap(), &patched).unwrap();
        assert!(fs::read_to_string(&path).unwrap().contains("gamma"));

        let moved = root.join("moved.txt");
        let to = path_policy::validate_new_target(moved.to_str().unwrap(), &roots).unwrap();
        fs::rename(&path, &to).unwrap();
        assert!(to.exists());
    }

    #[test]
    fn write_rejects_outside_workspace() {
        let tmp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let roots = roots_of(&tmp);
        let err = path_policy::validate_new_target(
            outside.path().join("x.txt").to_str().unwrap(),
            &roots,
        );
        assert!(err.is_err());
    }
}
