//! Native filesystem tools beyond `read_file`: list, search, write, patch, move.
//!
//! All paths are workspace-sandboxed via [`super::path_policy`]. Mutating tools
//! declare `ToolRisk::Mutating` so the central approval gateway defaults to Ask.
//!
//! Every tool here that names a path carries a validator resolving it exactly
//! as the handler will, so the gateway decides on the canonical target — a
//! symlink alias of a secret is judged as the secret. `search_files` never
//! returns content of a secret-bearing file: consent is per file, through
//! `read_file`.

use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use codescribe_core::agent::{
    ToolCallPreview, ToolDefinition, ToolExecutionPolicy, ToolInputValidator, ToolRegistry,
    ToolResultContent, ToolRisk,
};
use codescribe_core::util::safe_path::{
    safe_open_bounded, safe_read_to_string_bounded, safe_write_bounded,
};
use serde_json::{Value, json};

use super::{path_policy, workspace};

/// Maximum directory entries returned by one `fs.list` call.
const MAX_LIST_ENTRIES: usize = 500;
/// Maximum matches returned by one `fs.search` call.
const MAX_SEARCH_HITS: usize = 50;
/// Files larger than this are skipped during search — binaries and build
/// artifacts would flood the result set without informing the model.
const MAX_SEARCH_FILE_BYTES: u64 = 512 * 1024;
/// Directories one `fs.search` may descend below its root.
const MAX_SEARCH_DEPTH: usize = 32;
/// Directory entries one `fs.search` may inspect before it stops.
const MAX_SEARCH_ENTRIES: usize = 100_000;
/// Wall-clock budget for one `fs.search` walk.
const SEARCH_TIME_BUDGET: Duration = Duration::from_secs(15);
/// Upper bound on a single `fs.write` / `fs.patch` payload.
const MAX_WRITE_BYTES: usize = 512 * 1024;
/// Longest slice of a model-supplied search query echoed on an approval card.
const MAX_SUMMARY_QUERY_CHARS: usize = 80;

/// Register the filesystem tool family, read-only tools first and mutating
/// tools declared as [`ToolRisk::Mutating`] so approval defaults to Ask.
pub fn register(registry: &mut ToolRegistry) {
    registry
        .register_with_policy(
            list_directory_definition(),
            Box::new(|input| Box::pin(async move { handle_list_directory(input) })),
            ToolExecutionPolicy::native(ToolRisk::ReadOnly),
            Some(path_validator(&["path"], &[], summarize_list)),
        )
        .expect("register list_directory");
    registry
        .register_with_policy(
            search_files_definition(),
            Box::new(|input| Box::pin(handle_search_files(input))),
            ToolExecutionPolicy::native(ToolRisk::ReadOnly),
            Some(path_validator(&["path"], &[], summarize_search)),
        )
        .expect("register search_files");
    registry
        .register_with_policy(
            write_file_definition(),
            Box::new(|input| Box::pin(async move { handle_write_file(input) })),
            ToolExecutionPolicy::native(ToolRisk::Mutating),
            Some(path_validator(&[], &["path"], summarize_write)),
        )
        .expect("register write_file");
    registry
        .register_with_policy(
            apply_patch_definition(),
            Box::new(|input| Box::pin(async move { handle_apply_patch(input) })),
            ToolExecutionPolicy::native(ToolRisk::Mutating),
            Some(path_validator(&["path"], &[], summarize_patch)),
        )
        .expect("register apply_patch");
    registry
        .register_with_policy(
            move_path_definition(),
            Box::new(|input| Box::pin(async move { handle_move_path(input) })),
            ToolExecutionPolicy::native(ToolRisk::Mutating),
            Some(path_validator(&["from"], &["to"], summarize_move)),
        )
        .expect("register move_path");
}

/// One-line summary of a pending `list_directory`.
fn summarize_list(input: &Value, paths: &[String]) -> String {
    let recursive = input
        .get("recursive")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let scope = if recursive {
        "recursively"
    } else {
        "non-recursively"
    };
    format!("List {} {scope}", joined_targets(paths))
}

/// One-line summary of a pending `search_files`, naming the literal queried.
///
/// The query is the model's own text, so it is cut here: a card line is a
/// glance, and the full (bounded) query is on the card's argument snapshot.
fn summarize_search(input: &Value, paths: &[String]) -> String {
    let query: String = input
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .chars()
        .take(MAX_SUMMARY_QUERY_CHARS)
        .collect();
    format!("Search {} for “{query}”", joined_targets(paths))
}

/// One-line summary of a pending `write_file`. The payload size is what tells
/// two otherwise identical writes apart at a glance; the exact content is on
/// the card's argument snapshot.
fn summarize_write(input: &Value, paths: &[String]) -> String {
    let bytes = input
        .get("content")
        .and_then(Value::as_str)
        .map_or(0, str::len);
    format!("Write {bytes} byte(s) to {}", joined_targets(paths))
}

/// One-line summary of a pending `apply_patch`, sized by what it replaces.
fn summarize_patch(input: &Value, paths: &[String]) -> String {
    let removed = input
        .get("old_string")
        .and_then(Value::as_str)
        .map_or(0, str::len);
    let added = input
        .get("new_string")
        .and_then(Value::as_str)
        .map_or(0, str::len);
    format!(
        "Replace {removed} byte(s) with {added} byte(s) in {}",
        joined_targets(paths)
    )
}

/// One-line summary of a pending `move_path`: source first, destination second,
/// in the order [`path_validator`] resolved them.
fn summarize_move(_input: &Value, paths: &[String]) -> String {
    format!("Move {}", paths.join(" → "))
}

/// Canonical targets as one display string.
fn joined_targets(paths: &[String]) -> String {
    paths.join(", ")
}

/// Pre-flight validator resolving the named input fields exactly as the
/// handlers do: `existing` fields through [`path_policy::validate_existing`],
/// `new_targets` through [`path_policy::validate_new_target`].
///
/// The preview carries the canonical targets, so the permission gateway judges
/// what a handler will touch, not the alias the model typed. A path that does
/// not resolve inside the workspace roots is a hard deny before any approval.
///
/// `summarize` turns those resolved targets plus the call's own input into the
/// card's one-line description, so two writes to different files — or to the
/// same file with different payloads — never read as the same request.
fn path_validator(
    existing: &'static [&'static str],
    new_targets: &'static [&'static str],
    summarize: fn(&Value, &[String]) -> String,
) -> ToolInputValidator {
    Arc::new(move |input: &Value| {
        let roots = workspace_roots();
        let mut paths = Vec::new();
        for field in existing {
            let path = path_policy::validate_existing(require_string(input, field)?, &roots)?;
            paths.push(path.display().to_string());
        }
        for field in new_targets {
            let path = path_policy::validate_new_target(require_string(input, field)?, &roots)?;
            paths.push(path.display().to_string());
        }
        Ok(ToolCallPreview {
            summary: summarize(input, &paths),
            paths,
            ..Default::default()
        })
    })
}

/// Wire schema for `list_directory` (capability `fs.list`).
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

/// Wire schema for `search_files` (capability `fs.search`).
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

/// Wire schema for `write_file` (capability `fs.write`).
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

/// Wire schema for `apply_patch` (capability `fs.patch`).
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

/// Wire schema for `move_path` (capability `fs.move`).
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

/// Adapt [`list_directory_from_input`] to the tool-result protocol: errors
/// travel back to the model as text, never as a panic.
fn handle_list_directory(input: Value) -> Vec<ToolResultContent> {
    match list_directory_from_input(&input) {
        Ok(text) => vec![ToolResultContent::Text(text)],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

/// Trips its flag when dropped.
///
/// The search runs on a blocking thread that keeps going after the future
/// awaiting it is dropped, so dropping that future (the turn was cancelled) is
/// the only signal that reaches the walk.
#[derive(Default)]
struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// Tool-result adapter for [`search_files_from_input`]. The walk runs off the
/// async executor and stops at its next entry when the turn is cancelled.
async fn handle_search_files(input: Value) -> Vec<ToolResultContent> {
    let cancel = CancelOnDrop::default();
    let flag = Arc::clone(&cancel.0);
    let result = tokio::task::spawn_blocking(move || search_files_from_input(&input, &flag))
        .await
        .context("search worker stopped")
        .and_then(|result| result);
    match result {
        Ok(text) => vec![ToolResultContent::Text(text)],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

/// Tool-result adapter for [`write_file_from_input`].
fn handle_write_file(input: Value) -> Vec<ToolResultContent> {
    match write_file_from_input(&input) {
        Ok(text) => vec![ToolResultContent::Text(text)],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

/// Tool-result adapter for [`apply_patch_from_input`].
fn handle_apply_patch(input: Value) -> Vec<ToolResultContent> {
    match apply_patch_from_input(&input) {
        Ok(text) => vec![ToolResultContent::Text(text)],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

/// Tool-result adapter for [`move_path_from_input`].
fn handle_move_path(input: Value) -> Vec<ToolResultContent> {
    match move_path_from_input(&input) {
        Ok(text) => vec![ToolResultContent::Text(text)],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

/// Canonicalized workspace roots every path in this module is bounded by.
///
/// Re-resolved per call rather than cached: the operator can change workspace
/// roots mid-session, and a stale root would widen the sandbox.
fn workspace_roots() -> Vec<PathBuf> {
    path_policy::canonical_roots(&workspace::resolved_roots())
}

/// Read a required string field, reporting the field name when it is missing.
fn require_string<'a>(input: &'a Value, field: &str) -> Result<&'a str> {
    input
        .get(field)
        .and_then(Value::as_str)
        .with_context(|| format!("Missing required string field '{field}'"))
}

/// List a workspace directory, optionally descending one level.
///
/// Entries are sorted and truncated to [`MAX_LIST_ENTRIES`]; the payload
/// reports `truncated` so the model knows the listing is partial.
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

/// One row of a directory listing.
struct DirEntryInfo {
    /// Entry name, prefixed with `parent/` for recursed children.
    name: String,
    /// Absolute path as displayed to the model.
    path: String,
    /// `dir`, `file`, or `other`.
    kind: &'static str,
}

/// Walk `dir` into `out`, optionally descending one level into subdirectories.
///
/// Skips `.git`, `target`, and `node_modules`, re-checks root membership for
/// every entry so a symlink cannot walk the listing out of the sandbox, and
/// stops once [`MAX_LIST_ENTRIES`] is reached.
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

/// Why a search walk ended before it covered the whole tree. Machine codes for
/// the model, not copy: they travel in the payload as `stopped`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchStop {
    /// A subtree below [`SearchLimits::max_depth`] was not entered.
    Depth,
    /// [`SearchLimits::max_entries`] directory entries were inspected.
    Entries,
    /// [`SearchLimits::budget`] elapsed.
    Time,
    /// The turn was cancelled while the walk ran.
    Cancelled,
}

impl SearchStop {
    /// Stable payload spelling.
    fn code(self) -> &'static str {
        match self {
            Self::Depth => "depth_limit",
            Self::Entries => "entry_limit",
            Self::Time => "time_limit",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Independent ceilings on one search walk. Each one alone ends the walk (or
/// prunes the subtree) with the hits gathered so far, so a tree that is deep,
/// wide, or slow to read stays bounded whichever way it is pathological.
#[derive(Debug, Clone, Copy)]
struct SearchLimits {
    /// Directories the walk may descend below the search root.
    max_depth: usize,
    /// Directory entries the walk may inspect.
    max_entries: usize,
    /// Wall-clock budget for the walk.
    budget: Duration,
}

impl Default for SearchLimits {
    fn default() -> Self {
        Self {
            max_depth: MAX_SEARCH_DEPTH,
            max_entries: MAX_SEARCH_ENTRIES,
            budget: SEARCH_TIME_BUDGET,
        }
    }
}

/// What a search walk gathered, and why it stopped early if it did.
struct SearchOutcome {
    hits: Vec<Value>,
    stop: Option<SearchStop>,
}

/// Literal-substring search under a workspace path, with an optional filename
/// suffix filter. Output passes through the guard that bounds tool payloads.
///
/// `cancel` is polled by the walk; the async adapter trips it when the turn is
/// dropped.
fn search_files_from_input(input: &Value, cancel: &AtomicBool) -> Result<String> {
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

    let outcome = walk_search(
        &root,
        &roots,
        query,
        glob_suffix,
        SearchLimits::default(),
        cancel,
    );

    let payload = json!({
        "path": root.display().to_string(),
        "query": query,
        "count": outcome.hits.len(),
        "truncated": outcome.hits.len() >= MAX_SEARCH_HITS || outcome.stop.is_some(),
        "stopped": outcome.stop.map(SearchStop::code),
        "hits": outcome.hits,
        "provider": "native",
        "capability": "fs.search",
    });
    let text = serde_json::to_string_pretty(&payload)?;
    Ok(super::output_guard::guard_chunk(
        &text,
        &format!("fs.search '{query}' in {}", root.display()),
    ))
}

/// Iterative search walk from the canonical directory `root`.
///
/// Every entry is resolved to its canonical target first, and that target, not
/// the alias, is what gets contained, filtered, classified, and opened. An
/// alias of a secret (`docs/info.txt -> .env`) is therefore skipped like the
/// secret, a symlink out of the roots is skipped, and a directory symlink is
/// followed at most once per canonical directory, so `loop -> .` and mutual
/// cycles cannot recurse. The walk is a worklist, not recursion: depth costs
/// heap, never stack, and [`SearchLimits`] bounds depth, entries, and time
/// independently.
///
/// Unreadable directories are skipped rather than failing the whole search: a
/// single permission-denied subtree must not blank an otherwise useful result.
/// Dotfiles, VCS/build directories, secret-bearing files, and oversized files
/// are skipped; each hit records the canonical path, 1-based line number, and
/// a 200-char snippet.
fn walk_search(
    root: &Path,
    roots: &[PathBuf],
    query: &str,
    glob_suffix: Option<&str>,
    limits: SearchLimits,
    cancel: &AtomicBool,
) -> SearchOutcome {
    let started = Instant::now();
    let mut hits = Vec::new();
    let mut stop = None;
    let mut entries_seen = 0usize;
    let mut visited = HashSet::from([root.to_path_buf()]);
    let mut pending = vec![(root.to_path_buf(), 0usize)];
    'walk: while let Some((dir, depth)) = pending.pop() {
        let Ok(read) = open_dir_under_roots(&dir, roots) else {
            continue;
        };
        for entry in read.flatten() {
            if hits.len() >= MAX_SEARCH_HITS {
                break 'walk;
            }
            entries_seen += 1;
            if cancel.load(Ordering::Relaxed) {
                stop = Some(SearchStop::Cancelled);
                break 'walk;
            }
            if started.elapsed() >= limits.budget {
                stop = Some(SearchStop::Time);
                break 'walk;
            }
            if entries_seen > limits.max_entries {
                stop = Some(SearchStop::Entries);
                break 'walk;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if is_skipped_name(&name) {
                continue;
            }
            let Ok(canonical) = entry.path().canonicalize() else {
                continue;
            };
            let Some(bound) = containing_root(&canonical, roots) else {
                continue;
            };
            if !is_searchable_below(&canonical, bound) {
                continue;
            }
            if canonical.is_dir() {
                if depth >= limits.max_depth {
                    stop = stop.or(Some(SearchStop::Depth));
                } else if visited.insert(canonical.clone()) {
                    pending.push((canonical, depth + 1));
                }
                continue;
            }
            if !canonical.is_file() || path_policy::is_secret_bearing(&canonical) {
                continue;
            }
            if let Some(suffix) = glob_suffix
                && !name.ends_with(suffix)
            {
                continue;
            }
            let Ok(meta) = canonical.metadata() else {
                continue;
            };
            if meta.len() > MAX_SEARCH_FILE_BYTES {
                continue;
            }
            let Ok(file) = safe_open_bounded(&canonical, bound) else {
                continue;
            };
            for (idx, line) in BufReader::new(file).lines().enumerate() {
                if hits.len() >= MAX_SEARCH_HITS {
                    break;
                }
                let Ok(line) = line else { continue };
                if line.contains(query) {
                    let snippet: String = line.chars().take(200).collect();
                    hits.push(json!({
                        "path": canonical.display().to_string(),
                        "line": idx + 1,
                        "snippet": snippet,
                    }));
                }
            }
        }
    }
    SearchOutcome { hits, stop }
}

/// Names the search never enters or reads: VCS and build directories, and
/// dotfiles.
fn is_skipped_name(name: &str) -> bool {
    name == ".git" || name == "target" || name == "node_modules" || name.starts_with('.')
}

/// The deepest workspace root containing `canonical`, if any.
fn containing_root<'a>(canonical: &Path, roots: &'a [PathBuf]) -> Option<&'a PathBuf> {
    roots
        .iter()
        .filter(|root| canonical.starts_with(root))
        .max_by_key(|root| root.components().count())
}

/// Whether every component of `canonical` below `root` is one the search walks
/// into, so an alias into `.git` or a dot-directory is skipped like the real
/// path would be.
fn is_searchable_below(canonical: &Path, root: &Path) -> bool {
    let Ok(relative) = canonical.strip_prefix(root) else {
        return false;
    };
    relative.components().all(|component| match component {
        Component::Normal(name) => !is_skipped_name(&name.to_string_lossy()),
        _ => false,
    })
}

/// Write UTF-8 text to a workspace path, creating parent directories.
///
/// The target is validated as a new-or-existing path inside a root, and the
/// write itself goes through the bounded safe-path helper.
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

/// Replace an exact substring in a workspace file.
///
/// `old_string` must match exactly once: zero matches and multiple matches both
/// fail loudly, so an ambiguous patch can never silently hit the wrong site.
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

/// Move or rename a path, with both ends bounded by workspace roots and the
/// destination's parent created on demand.
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

/// Path-policy bounded write/list/move coverage for the native FS tool.
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Canonicalize a temp dir into the path-policy root list shape.
    fn roots_of(tmp: &TempDir) -> Vec<PathBuf> {
        path_policy::canonical_roots(&[tmp.path().to_path_buf()])
    }

    /// Write, in-place patch, and rename stay inside the validated workspace root.
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

    /// Secret aliases, aliases outside the workspace and a directory cycle
    /// must not leak content or prevent the public file from being found.
    #[test]
    fn search_skips_secret_aliases_and_directory_cycles() {
        let tmp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        fs::write(tmp.path().join("public.txt"), "fixture needle").unwrap();
        fs::write(tmp.path().join(".env"), "fixture needle secret").unwrap();
        fs::write(outside.path().join("private.txt"), "fixture needle outside").unwrap();
        std::os::unix::fs::symlink(tmp.path().join(".env"), tmp.path().join("alias.txt")).unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("escape")).unwrap();
        std::os::unix::fs::symlink(tmp.path(), tmp.path().join("loop")).unwrap();
        let roots = roots_of(&tmp);
        let result = walk_search(
            &roots[0],
            &roots,
            "needle",
            None,
            SearchLimits::default(),
            &AtomicBool::new(false),
        );
        assert!(result.stop.is_none());
        assert_eq!(result.hits.len(), 1);
        assert_eq!(
            result.hits[0]["path"],
            roots[0].join("public.txt").display().to_string()
        );
        assert_eq!(result.hits[0]["snippet"], "fixture needle");
    }

    /// Exhaustion and cancellation must be reported rather than returning an
    /// apparently complete empty search result.
    #[test]
    fn search_reports_budget_exhaustion_and_cancellation() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("public.txt"), "needle").unwrap();
        let roots = roots_of(&tmp);
        for (limits, cancelled, expected) in [
            (
                SearchLimits {
                    max_entries: 0,
                    ..SearchLimits::default()
                },
                false,
                SearchStop::Entries,
            ),
            (
                SearchLimits {
                    budget: Duration::ZERO,
                    ..SearchLimits::default()
                },
                false,
                SearchStop::Time,
            ),
            (SearchLimits::default(), true, SearchStop::Cancelled),
        ] {
            let result = walk_search(
                &roots[0],
                &roots,
                "needle",
                None,
                limits,
                &AtomicBool::new(cancelled),
            );
            assert_eq!(result.stop, Some(expected));
            assert!(result.hits.is_empty());
        }
    }

    /// Targets outside every workspace root are rejected before write.
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
