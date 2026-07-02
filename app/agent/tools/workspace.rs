//! `list_projects` native tool + workspace-root resolution shared with the agent
//! system prompt.
//!
//! The agent must resolve a project *name* ("vista") to an absolute path before
//! calling path-hungry tools (prview / loctree / vc_*). Without this it guesses
//! (`~/vista`, `~/dev/vista`, …) and misses the operator's convention that repos
//! live under `~/Git`. This tool enumerates the git checkouts under the
//! configured workspace roots so the model never has to guess.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::{env, fs};

use codescribe_core::agent::{ToolDefinition, ToolRegistry, ToolResultContent};
use serde_json::{Value, json};

/// Built-in default workspace root when `AGENT_WORKSPACE_ROOTS` is unset. Mirrors
/// the bridge default (`bridge/src/config.rs`) and the operator convention that
/// repositories live under `~/Git`.
const DEFAULT_WORKSPACE_ROOT: &str = "~/Git";

/// Upper bound on returned repositories — keeps the tool result bounded even if a
/// root holds hundreds of checkouts.
const MAX_PROJECTS: usize = 100;

pub fn register(registry: &mut ToolRegistry) {
    registry
        .register(
            list_projects_definition(),
            Box::new(|input| Box::pin(handle_list_projects(input))),
        )
        .expect("register list_projects tool");
}

fn list_projects_definition() -> ToolDefinition {
    ToolDefinition {
        name: "list_projects".to_string(),
        description:
            "List the user's local project repositories (git checkouts) under the configured \
             workspace roots. Use this to resolve a project name to an absolute path before \
             calling tools that need a repo path (prview, loctree, vc_*)."
                .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {}
        }),
    }
}

async fn handle_list_projects(_input: Value) -> Vec<ToolResultContent> {
    let roots = resolved_roots();
    let projects = scan_projects(&roots, MAX_PROJECTS);
    let payload = json!({
        "roots": roots
            .iter()
            .map(|root| root.display().to_string())
            .collect::<Vec<_>>(),
        "count": projects.len(),
        "projects": projects
            .iter()
            .map(|project| json!({ "name": project.name, "path": project.path }))
            .collect::<Vec<_>>(),
    });
    match serde_json::to_string_pretty(&payload) {
        Ok(text) => vec![ToolResultContent::Text(text)],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

/// A resolved local project repository.
struct Project {
    name: String,
    path: String,
}

/// Configured workspace roots parsed from `AGENT_WORKSPACE_ROOTS`
/// (colon-separated, PATH-style), falling back to the built-in default. Returns
/// raw, unexpanded strings (tilde left intact) so they can be shown verbatim in
/// the system prompt.
pub(crate) fn configured_roots() -> Vec<String> {
    let roots: Vec<String> = env::var("AGENT_WORKSPACE_ROOTS")
        .ok()
        .map(|value| {
            value
                .split(':')
                .map(|segment| segment.trim().to_string())
                .filter(|segment| !segment.is_empty())
                .collect()
        })
        .unwrap_or_default();

    if roots.is_empty() {
        vec![DEFAULT_WORKSPACE_ROOT.to_string()]
    } else {
        roots
    }
}

/// Expanded, absolute workspace roots (tilde → `$HOME`).
fn resolved_roots() -> Vec<PathBuf> {
    configured_roots()
        .iter()
        .map(|root| expand_tilde(root))
        .collect()
}

/// Expand a leading `~` / `~/` to `$HOME`. Non-tilde paths pass through unchanged.
fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    } else if path == "~"
        && let Ok(home) = env::var("HOME")
    {
        return PathBuf::from(home);
    }
    PathBuf::from(path)
}

/// Scan each root one level deep and collect directories that are git checkouts
/// (contain a `.git` entry — dir OR file, to catch worktrees/submodules). No
/// recursion; bounded by `limit`; duplicate paths across roots are de-duped.
fn scan_projects(roots: &[PathBuf], limit: usize) -> Vec<Project> {
    let mut projects = Vec::new();
    let mut seen = HashSet::new();

    for root in roots {
        let Ok(entries) = fs::read_dir(root) else {
            // Missing / unreadable root is not an error — just contributes nothing.
            continue;
        };

        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        dirs.sort();

        for dir in dirs {
            if projects.len() >= limit {
                return projects;
            }
            if !is_git_checkout(&dir) {
                continue;
            }
            let path = dir.display().to_string();
            if !seen.insert(path.clone()) {
                continue;
            }
            let name = dir
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| path.clone());
            projects.push(Project { name, path });
        }
    }

    projects
}

/// A directory is a git checkout when it holds a `.git` entry (a directory for a
/// normal clone, or a file for a worktree/submodule gitlink).
fn is_git_checkout(dir: &Path) -> bool {
    dir.join(".git").exists()
}

/// A concise workspace section for the agent system prompt: the configured roots
/// plus the instruction to resolve names via `list_projects` instead of guessing
/// filesystem paths.
pub(crate) fn workspace_prompt_section() -> String {
    let roots = configured_roots().join(", ");
    format!(
        "WORKSPACE\n\
         The user's local project repositories live under: {roots}.\n\
         Before calling any tool that needs a repository path (prview, loctree, vc_*), \
         call `list_projects` to resolve a project name to its absolute path. \
         Do not guess or invent filesystem paths."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn scan_finds_only_git_checkouts_one_level_deep() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();

        // repo A: classic clone (.git directory)
        fs::create_dir_all(root.join("alpha").join(".git")).unwrap();
        // repo B: worktree/submodule style (.git file)
        fs::create_dir_all(root.join("beta")).unwrap();
        fs::write(root.join("beta").join(".git"), "gitdir: /elsewhere").unwrap();
        // plain dir without .git → skipped
        fs::create_dir_all(root.join("plain")).unwrap();
        // repo nested TWO levels deep → must NOT be found (no recursion)
        fs::create_dir_all(root.join("plain").join("nested").join(".git")).unwrap();

        let projects = scan_projects(&[root.to_path_buf()], 100);
        let names: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta"]);
        assert!(
            projects
                .iter()
                .all(|p| p.path.starts_with(root.to_str().unwrap()))
        );
    }

    #[test]
    fn scan_respects_limit() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        for i in 0..5 {
            fs::create_dir_all(root.join(format!("repo{i}")).join(".git")).unwrap();
        }
        let projects = scan_projects(&[root.to_path_buf()], 3);
        assert_eq!(projects.len(), 3);
    }

    #[test]
    fn scan_skips_missing_root_without_error() {
        let projects = scan_projects(&[PathBuf::from("/nonexistent/xyzzy-workspace")], 100);
        assert!(projects.is_empty());
    }

    #[test]
    fn scan_dedupes_paths_across_overlapping_roots() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::create_dir_all(root.join("solo").join(".git")).unwrap();

        // Same root listed twice: the checkout must appear once.
        let projects = scan_projects(&[root.to_path_buf(), root.to_path_buf()], 100);
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "solo");
    }

    #[test]
    fn expand_tilde_replaces_home_prefix() {
        if let Ok(home) = env::var("HOME") {
            assert_eq!(expand_tilde("~/Git"), PathBuf::from(&home).join("Git"));
            assert_eq!(expand_tilde("~"), PathBuf::from(&home));
        }
        // Non-tilde absolute path passes through untouched.
        assert_eq!(expand_tilde("/abs/path"), PathBuf::from("/abs/path"));
    }
}
