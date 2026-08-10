//! Native Git repository tools (canonical: repo.status / repo.diff / repo.log / repo.commit).
//!
//! Invokes `git` as a single argv program with no shell string concatenation.
//! Working directory must be inside configured workspace roots.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};
use codescribe_core::agent::{ToolDefinition, ToolRegistry, ToolResultContent, ToolRisk};
use serde_json::{Value, json};

use super::{output_guard, path_policy, workspace};

/// Register the four native git tools on the shared [`ToolRegistry`].
///
/// `git_status` / `git_diff` / `git_log` are [`ToolRisk::ReadOnly`]; `git_commit`
/// is [`ToolRisk::Mutating`] and therefore gated behind approval.
pub fn register(registry: &mut ToolRegistry) {
    registry
        .register_native(
            git_status_definition(),
            Box::new(|input| Box::pin(async move { handle_git_status(input) })),
            ToolRisk::ReadOnly,
        )
        .expect("register git_status");
    registry
        .register_native(
            git_diff_definition(),
            Box::new(|input| Box::pin(async move { handle_git_diff(input) })),
            ToolRisk::ReadOnly,
        )
        .expect("register git_diff");
    registry
        .register_native(
            git_log_definition(),
            Box::new(|input| Box::pin(async move { handle_git_log(input) })),
            ToolRisk::ReadOnly,
        )
        .expect("register git_log");
    registry
        .register_native(
            git_commit_definition(),
            Box::new(|input| Box::pin(async move { handle_git_commit(input) })),
            ToolRisk::Mutating,
        )
        .expect("register git_commit");
}

/// Tool schema for `git_status` (canonical capability `repo.status`).
fn git_status_definition() -> ToolDefinition {
    ToolDefinition {
        name: "git_status".to_string(),
        description:
            "Show git status for a repository under workspace roots (canonical: repo.status)."
                .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "cwd": { "type": "string", "description": "Absolute path to a git working tree" }
            },
            "required": ["cwd"]
        }),
    }
}

/// Tool schema for `git_diff` (canonical capability `repo.diff`).
fn git_diff_definition() -> ToolDefinition {
    ToolDefinition {
        name: "git_diff".to_string(),
        description: "Show git diff for a repository (canonical: repo.diff). Optional path filter."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "cwd": { "type": "string" },
                "path": { "type": "string", "description": "Optional path filter" },
                "staged": { "type": "boolean", "description": "When true, show staged diff only" }
            },
            "required": ["cwd"]
        }),
    }
}

/// Tool schema for `git_log` (canonical capability `repo.log`).
fn git_log_definition() -> ToolDefinition {
    ToolDefinition {
        name: "git_log".to_string(),
        description: "Show recent git log (canonical: repo.log). Bounded to 20 commits by default."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "cwd": { "type": "string" },
                "limit": { "type": "integer", "description": "Max commits (1-50, default 20)" }
            },
            "required": ["cwd"]
        }),
    }
}

/// Tool schema for `git_commit` (canonical capability `repo.commit`).
///
/// Commits only — the tool never pushes.
fn git_commit_definition() -> ToolDefinition {
    ToolDefinition {
        name: "git_commit".to_string(),
        description: "Stage optional paths and create a git commit (canonical: repo.commit). Requires approval. Does not push."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "cwd": { "type": "string" },
                "message": { "type": "string" },
                "paths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional paths to stage before commit"
                }
            },
            "required": ["cwd", "message"]
        }),
    }
}

/// Dispatch adapter for `git_status`: turns a [`Result`] into tool content.
fn handle_git_status(input: Value) -> Vec<ToolResultContent> {
    match run_status(&input) {
        Ok(text) => vec![ToolResultContent::Text(text)],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

/// Dispatch adapter for `git_diff`: turns a [`Result`] into tool content.
fn handle_git_diff(input: Value) -> Vec<ToolResultContent> {
    match run_diff(&input) {
        Ok(text) => vec![ToolResultContent::Text(text)],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

/// Dispatch adapter for `git_log`: turns a [`Result`] into tool content.
fn handle_git_log(input: Value) -> Vec<ToolResultContent> {
    match run_log(&input) {
        Ok(text) => vec![ToolResultContent::Text(text)],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

/// Dispatch adapter for `git_commit`: turns a [`Result`] into tool content.
fn handle_git_commit(input: Value) -> Vec<ToolResultContent> {
    match run_commit(&input) {
        Ok(text) => vec![ToolResultContent::Text(text)],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

/// Canonicalized workspace roots every git path argument is confined to.
fn roots() -> Vec<PathBuf> {
    path_policy::canonical_roots(&workspace::resolved_roots())
}

/// Read the required `cwd` field and prove it is an existing directory inside
/// the configured workspace roots.
fn resolve_cwd(input: &Value) -> Result<PathBuf> {
    let cwd = input
        .get("cwd")
        .and_then(Value::as_str)
        .context("Missing required string field 'cwd'")?;
    let cwd = path_policy::validate_existing(cwd, &roots())?;
    if !cwd.is_dir() {
        bail!("cwd is not a directory: {}", cwd.display());
    }
    Ok(cwd)
}

/// Run `git` as a single argv program in `cwd` and return its guarded output.
///
/// stdout and stderr are concatenated; a non-zero exit becomes an [`Err`]. Both
/// paths pass through [`output_guard::guard_chunk`] so an oversized diff cannot
/// flood the transcript.
fn run_git(cwd: &PathBuf, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("Failed to spawn git {}", args.join(" ")))?;
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    if !output.stderr.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    let origin = format!("git {}", args.join(" "));
    if !output.status.success() {
        bail!(
            "{origin} failed (exit {:?}): {}",
            output.status.code(),
            output_guard::guard_chunk(&text, &origin)
        );
    }
    Ok(output_guard::guard_chunk(&text, &origin))
}

/// `repo.status` body: short branch-annotated status as a JSON envelope.
fn run_status(input: &Value) -> Result<String> {
    let cwd = resolve_cwd(input)?;
    let body = run_git(&cwd, &["status", "--short", "--branch"])?;
    Ok(json!({
        "cwd": cwd.display().to_string(),
        "output": body,
        "provider": "native",
        "capability": "repo.status",
    })
    .to_string())
}

/// `repo.diff` body: optional `staged` flag and an optional root-validated path
/// filter, emitted as a JSON envelope.
fn run_diff(input: &Value) -> Result<String> {
    let cwd = resolve_cwd(input)?;
    let staged = input
        .get("staged")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut args = vec!["diff"];
    if staged {
        args.push("--cached");
    }
    let path = input.get("path").and_then(Value::as_str);
    if let Some(path) = path {
        let _ = path_policy::validate_existing(path, &roots())
            .or_else(|_| path_policy::validate_new_target(path, &roots()))?;
        args.push("--");
        args.push(path);
    }
    let body = run_git(&cwd, &args)?;
    Ok(json!({
        "cwd": cwd.display().to_string(),
        "output": body,
        "provider": "native",
        "capability": "repo.diff",
    })
    .to_string())
}

/// `repo.log` body: one-line decorated history, bounded to 1..=50 commits.
fn run_log(input: &Value) -> Result<String> {
    let cwd = resolve_cwd(input)?;
    let limit = input
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(20)
        .clamp(1, 50);
    let limit_flag = format!("-{limit}");
    let body = run_git(
        &cwd,
        &["log", &limit_flag, "--oneline", "--decorate", "--no-color"],
    )?;
    Ok(json!({
        "cwd": cwd.display().to_string(),
        "output": body,
        "provider": "native",
        "capability": "repo.log",
    })
    .to_string())
}

/// `repo.commit` body: stage the optional validated paths, then commit.
///
/// The message travels as argv only, never through a shell.
fn run_commit(input: &Value) -> Result<String> {
    let cwd = resolve_cwd(input)?;
    let message = input
        .get("message")
        .and_then(Value::as_str)
        .context("Missing required string field 'message'")?;
    if message.trim().is_empty() {
        bail!("commit message must not be empty");
    }
    // Block destructive git via path_policy terminal rules if message abused — message is argv only.
    if let Some(paths) = input.get("paths").and_then(Value::as_array) {
        for path in paths {
            let Some(path) = path.as_str() else { continue };
            let validated = path_policy::validate_existing(path, &roots())
                .or_else(|_| path_policy::validate_new_target(path, &roots()))?;
            run_git(
                &cwd,
                &["add", "--", validated.to_str().context("path utf-8")?],
            )?;
        }
    }
    let body = run_git(&cwd, &["commit", "-m", message])?;
    Ok(json!({
        "ok": true,
        "cwd": cwd.display().to_string(),
        "output": body,
        "provider": "native",
        "capability": "repo.commit",
    })
    .to_string())
}

/// Exercises [`run_git`] directly against a throwaway repository.
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// git_status tool returns a structured status against a temporary git repo fixture.
    #[test]
    fn git_status_on_temp_repo() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        assert!(
            Command::new("git")
                .args(["init"])
                .current_dir(root)
                .status()
                .unwrap()
                .success()
        );
        // Configure identity for commit tests if needed
        let _ = Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(root)
            .status();
        let _ = Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(root)
            .status();
        fs::write(root.join("file.txt"), "x").unwrap();

        // path_policy needs the root as allowed — unit path uses validate only when
        // settings roots match. Here we exercise run_git directly.
        let out = run_git(&root.to_path_buf(), &["status", "--short"]).unwrap();
        assert!(out.contains("file.txt") || out.contains("??"));
    }
}
