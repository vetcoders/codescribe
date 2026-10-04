//! HuggingFace cache utilities.
//!
//! Resolves local snapshot paths for repos downloaded via `hf download`.
//! This avoids hardcoded model directories and uses HF cache directly.

use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;

use directories::BaseDirs;

/// Locate a cached snapshot of `repo` containing every file in `required`.
pub fn find_snapshot(repo: &str, required: &[&str]) -> Option<PathBuf> {
    find_snapshot_with_any(repo, required, &[])
}

/// Locate a cached snapshot satisfying both an all-of and an any-of file list.
///
/// The any-of list exists because a repo can ship one of several interchangeable
/// weight formats; demanding a specific one would miss a perfectly usable
/// download. Cache roots are tried in [`cache_bases`] order and the first hit
/// wins.
pub fn find_snapshot_with_any(
    repo: &str,
    required_all: &[&str],
    required_any: &[&str],
) -> Option<PathBuf> {
    find_snapshot_with_any_matching(repo, required_all, required_any, |_| true)
}

/// Locate the first file-complete snapshot accepted by `predicate`.
///
/// Cache-root precedence remains stable. Within each root, candidates are
/// examined newest first so an invalid fresh download can fall back to an
/// older usable revision without allowing a later cache root to jump ahead.
pub fn find_snapshot_with_any_matching<F>(
    repo: &str,
    required_all: &[&str],
    required_any: &[&str],
    predicate: F,
) -> Option<PathBuf>
where
    F: Fn(&Path) -> bool,
{
    find_snapshot_in_bases_matching(cache_bases(), repo, required_all, required_any, &predicate)
}

fn find_snapshot_in_bases_matching<I, F>(
    bases: I,
    repo: &str,
    required_all: &[&str],
    required_any: &[&str],
    predicate: &F,
) -> Option<PathBuf>
where
    I: IntoIterator<Item = PathBuf>,
    F: Fn(&Path) -> bool,
{
    for base in bases {
        if let Some(snapshot) =
            find_snapshot_in_base_matching(&base, repo, required_all, required_any, predicate)
        {
            return Some(snapshot);
        }
    }
    None
}

/// Every directory that might hold a Hugging Face cache, deduplicated.
///
/// Environment overrides come first (`CODESCRIBE_HF_CACHE`,
/// `HUGGINGFACE_HUB_CACHE`, `HF_HUB_CACHE`, `HF_HOME`), then the standard
/// `~/.cache/huggingface/hub`, then the Codescribe-local download locations.
/// Sorting before dedup means the returned order is stable, not
/// insertion-ordered.
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
        // Support local cache used by Codescribe tools (hf download into ~/.codescribe/embeddings)
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

/// Search one cache root for the newest snapshot of `repo` that has the
/// required files.
///
/// The directory name is derived from the repo id, but the cache preserves the
/// original casing, so a miss falls back to a case-insensitive scan rather than
/// reporting the model absent. Among qualifying snapshots the most recently
/// modified wins — the cache can hold several revisions at once.
fn find_snapshot_in_base_matching<F>(
    base: &PathBuf,
    repo: &str,
    required_all: &[&str],
    required_any: &[&str],
    predicate: &F,
) -> Option<PathBuf>
where
    F: Fn(&Path) -> bool,
{
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

    let mut candidates = Vec::new();

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
        candidates.push((modified, path));
    }

    candidates.sort_by(|(left_time, left_path), (right_time, right_path)| {
        right_time
            .cmp(left_time)
            .then_with(|| left_path.cmp(right_path))
    });
    candidates
        .into_iter()
        .map(|(_, path)| path)
        .find(|path| predicate(path))
}

/// Newest cached snapshot for every whisper-family repo in the known caches.
///
/// Listing only, in [`cache_bases`] precedence order: no completeness filtering
/// (callers validate), one entry per repo id (the first root that carries the
/// repo wins, matching resolution precedence), and only snapshots that hold at
/// least one weight file. A tokenizer-only companion repo is not a model and is
/// never listed; a quantized repo keeps its weights file and stays visible so
/// the caller can report the refusal reason.
pub fn whisper_cache_snapshots() -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    for base in cache_bases() {
        let Ok(entries) = fs::read_dir(&base) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Some(repo_slug) = name.strip_prefix("models--") else {
                continue;
            };
            let repo = repo_slug.replace("--", "/");
            if !repo.to_ascii_lowercase().contains("whisper") {
                continue;
            }
            if out.iter().any(|(known, _)| known == &repo) {
                continue;
            }
            let snapshots_dir = entry.path().join("snapshots");
            let Ok(revisions) = fs::read_dir(&snapshots_dir) else {
                continue;
            };
            let newest = revisions
                .flatten()
                .filter(|revision| revision.path().is_dir())
                .map(|revision| {
                    let modified = revision
                        .metadata()
                        .and_then(|metadata| metadata.modified())
                        .unwrap_or(SystemTime::UNIX_EPOCH);
                    (modified, revision.path())
                })
                .filter(|(_, path)| {
                    crate::whisper_weights::SUPPORTED_NAMES
                        .iter()
                        .any(|weights| path.join(weights).is_file())
                })
                .max_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
            if let Some((_, path)) = newest {
                out.push((repo, path));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::FileTimes;
    use std::time::Duration;
    use tempfile::TempDir;

    fn snapshot(base: &Path, repo: &str, name: &str, modified_secs: u64) -> PathBuf {
        let path = base
            .join(format!("models--{}", repo.replace('/', "--")))
            .join("snapshots")
            .join(name);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("config.json"), "{}").unwrap();
        let times = FileTimes::new()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(modified_secs));
        fs::File::open(&path).unwrap().set_times(times).unwrap();
        path
    }

    #[test]
    fn matching_snapshot_falls_back_from_invalid_newest_to_valid_older() {
        let temp = TempDir::new().unwrap();
        let repo = "owner/model";
        let older = snapshot(temp.path(), repo, "older", 10);
        let newer = snapshot(temp.path(), repo, "newer", 20);
        fs::write(older.join("valid.marker"), "yes").unwrap();

        let found = find_snapshot_in_bases_matching(
            [temp.path().to_path_buf()],
            repo,
            &["config.json"],
            &[],
            &|path| path.join("valid.marker").is_file(),
        )
        .unwrap();

        assert_eq!(found, older);
        assert_ne!(found, newer);
    }

    #[test]
    fn matching_snapshot_keeps_newest_valid_candidate() {
        let temp = TempDir::new().unwrap();
        let repo = "owner/model";
        let older = snapshot(temp.path(), repo, "older", 10);
        let newer = snapshot(temp.path(), repo, "newer", 20);
        fs::write(older.join("valid.marker"), "yes").unwrap();
        fs::write(newer.join("valid.marker"), "yes").unwrap();

        let found = find_snapshot_in_bases_matching(
            [temp.path().to_path_buf()],
            repo,
            &["config.json"],
            &[],
            &|path| path.join("valid.marker").is_file(),
        )
        .unwrap();

        assert_eq!(found, newer);
    }

    #[test]
    fn invalid_earlier_cache_root_does_not_shadow_valid_later_root() {
        let first = TempDir::new().unwrap();
        let second = TempDir::new().unwrap();
        let repo = "owner/model";
        snapshot(first.path(), repo, "invalid", 20);
        let valid = snapshot(second.path(), repo, "valid", 10);
        fs::write(valid.join("valid.marker"), "yes").unwrap();

        let found = find_snapshot_in_bases_matching(
            [first.path().to_path_buf(), second.path().to_path_buf()],
            repo,
            &["config.json"],
            &[],
            &|path| path.join("valid.marker").is_file(),
        )
        .unwrap();

        assert_eq!(found, valid);
    }
}
