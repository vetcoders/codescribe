//! HuggingFace cache utilities.
//!
//! Resolves local snapshot paths for repos downloaded via `hf download`.
//! This avoids hardcoded model directories and uses HF cache directly.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

use directories::BaseDirs;

pub fn find_snapshot(repo: &str, required: &[&str]) -> Option<PathBuf> {
    find_snapshot_with_any(repo, required, &[])
}

pub fn find_snapshot_with_any(
    repo: &str,
    required_all: &[&str],
    required_any: &[&str],
) -> Option<PathBuf> {
    for base in cache_bases() {
        if let Some(snapshot) = find_snapshot_in_base(&base, repo, required_all, required_any) {
            return Some(snapshot);
        }
    }
    None
}

fn cache_bases() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(path) = env::var("CODESCRIBE_HF_CACHE") {
        out.push(PathBuf::from(path));
    }
    if let Ok(path) = env::var("HUGGINGFACE_HUB_CACHE") {
        out.push(PathBuf::from(path));
    }
    if let Ok(path) = env::var("HF_HUB_CACHE") {
        out.push(PathBuf::from(path));
    }
    if let Ok(path) = env::var("HF_HOME") {
        out.push(PathBuf::from(path).join("hub"));
    }
    if let Some(dirs) = BaseDirs::new() {
        out.push(
            dirs.home_dir()
                .join(".cache")
                .join("huggingface")
                .join("hub"),
        );
        // Support local cache used by CodeScribe tools (hf download into ~/.codescribe/embeddings)
        out.push(dirs.home_dir().join(".codescribe").join("embeddings"));
        out.push(
            dirs.home_dir()
                .join(".codescribe")
                .join("embeddings")
                .join("hub"),
        );
    }
    out.sort();
    out.dedup();
    out
}

fn find_snapshot_in_base(
    base: &PathBuf,
    repo: &str,
    required_all: &[&str],
    required_any: &[&str],
) -> Option<PathBuf> {
    let repo_dir = base.join(format!("models--{}", repo.replace('/', "--")));
    let snapshots_dir = repo_dir.join("snapshots");

    let snapshots_dir = if snapshots_dir.exists() {
        snapshots_dir
    } else {
        // Case-insensitive repo match fallback (HF cache uses original casing)
        let target = repo.to_ascii_lowercase();
        let mut matched: Option<PathBuf> = None;
        if let Ok(entries) = fs::read_dir(base) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if !name.starts_with("models--") {
                    continue;
                }
                let repo_id = name
                    .strip_prefix("models--")
                    .unwrap_or("")
                    .replace("--", "/");
                if repo_id.to_ascii_lowercase() == target {
                    matched = Some(entry.path().join("snapshots"));
                    break;
                }
            }
        }
        matched?
    };

    let entries = fs::read_dir(&snapshots_dir).ok()?;

    let mut best: Option<(SystemTime, PathBuf)> = None;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if !required_all.iter().all(|f| path.join(f).exists()) {
            continue;
        }
        if !required_any.is_empty() && !required_any.iter().any(|f| path.join(f).exists()) {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        match &best {
            Some((best_time, _)) if *best_time >= modified => {}
            _ => best = Some((modified, path)),
        }
    }

    best.map(|(_, p)| p)
}
