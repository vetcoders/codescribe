//! HuggingFace cache utilities.
//!
//! Resolves local snapshot paths for repos downloaded via `hf download`.
//! This avoids hardcoded model directories and uses HF cache directly.
//!
//! Created by M&K (c)2026 VetCoders

use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

use directories::BaseDirs;

pub fn find_snapshot(repo: &str, required: &[&str]) -> Option<PathBuf> {
    let base = cache_base()?;
    let repo_dir = base.join(format!("models--{}", repo.replace('/', "--")));
    let snapshots_dir = repo_dir.join("snapshots");
    let entries = fs::read_dir(&snapshots_dir).ok()?;

    let mut best: Option<(SystemTime, PathBuf)> = None;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if !required.iter().all(|f| path.join(f).exists()) {
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

fn cache_base() -> Option<PathBuf> {
    if let Ok(path) = env::var("HUGGINGFACE_HUB_CACHE") {
        return Some(PathBuf::from(path));
    }
    if let Ok(path) = env::var("HF_HUB_CACHE") {
        return Some(PathBuf::from(path));
    }
    if let Ok(path) = env::var("HF_HOME") {
        return Some(PathBuf::from(path).join("hub"));
    }
    BaseDirs::new().map(|d| d.home_dir().join(".cache").join("huggingface").join("hub"))
}
