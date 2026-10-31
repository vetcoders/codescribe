//! HuggingFace cache utilities.
//!
//! Resolves local snapshot paths for repos downloaded via `hf download`.
//! This avoids hardcoded model directories and uses HF cache directly.
//!
//! Every root and every required snapshot file is resolved link by link before
//! it is trusted. `hf download` (1.32 Xet storage) writes two-level links,
//! `snapshots/<rev>/<file> -> ../../blobs/<sha> -> ../../blobs/<2ch>/<sha>`;
//! they stay valid as long as the final target remains inside the cache root.
//! A link into `/Volumes/<name>` whose volume is not mounted is reported as
//! [`CachePathStatus::UnmountedExternalVolume`] from the link text alone, so a
//! detached drive is named instead of surfacing as an anonymous I/O failure.

use std::collections::BTreeSet;
use std::collections::VecDeque;
use std::env;
use std::fs;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;

use directories::BaseDirs;

/// Links followed while resolving one cache path before it counts as missing.
const MAX_LINK_HOPS: usize = 16;

/// Where a Hugging Face cache path really lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CachePathStatus {
    /// The path resolves to an existing entry inside a trusted cache root.
    Ready { resolved: PathBuf },
    /// The path, or a link on the way, points under `/Volumes/<name>` and that
    /// volume is not mounted. `symlink_path` is the last link followed (the
    /// path itself when no link was involved) and `target_path` its target.
    UnmountedExternalVolume {
        symlink_path: PathBuf,
        target_path: PathBuf,
    },
    /// Nothing exists where the path resolves, or the link chain is too long.
    Missing { path: PathBuf },
    /// The path resolves to an existing entry outside every trusted root.
    OutsideCacheRoot { resolved: PathBuf },
}

impl CachePathStatus {
    /// Whether the path can be read as part of the cache.
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }
}

/// Resolve a cache root itself. The root is the trust anchor, so any existing
/// destination is accepted; only absence and a detached volume are refused.
fn classify_cache_root(base: &Path) -> CachePathStatus {
    match resolve_links(base) {
        Ok(resolved) => CachePathStatus::Ready { resolved },
        Err(status) => status,
    }
}

/// The resolved `base` and, for a `<home>/hub` base, the resolved `<home>`,
/// which is where `hf` keeps its other storage.
fn trusted_roots(base: &Path) -> Result<Vec<PathBuf>, CachePathStatus> {
    let mut trusted = match classify_cache_root(base) {
        CachePathStatus::Ready { resolved } => vec![resolved],
        status => return Err(status),
    };
    if base.file_name().is_some_and(|name| name == "hub")
        && let Some(home) = base.parent()
        && let Ok(home) = resolve_links(home)
    {
        trusted.push(home);
    }
    Ok(trusted)
}

/// Resolve one snapshot file and require its final target to stay inside a
/// trusted root.
fn classify_snapshot_file(trusted: &[PathBuf], file: &Path) -> CachePathStatus {
    match resolve_links(file) {
        Ok(resolved) if trusted.iter().any(|root| resolved.starts_with(root)) => {
            CachePathStatus::Ready { resolved }
        }
        Ok(resolved) => CachePathStatus::OutsideCacheRoot { resolved },
        Err(status) => status,
    }
}

/// Follow `path` one component and one link at a time.
///
/// Each step costs one `lstat` on an already-resolved parent, so a link into an
/// absent `/Volumes/<name>` is recognised when `/Volumes` is the parent and the
/// detached volume is never asked for anything below its mount point.
fn resolve_links(path: &Path) -> Result<PathBuf, CachePathStatus> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .map_err(|_| CachePathStatus::Missing {
                path: path.to_path_buf(),
            })?
            .join(path)
    };
    let mut queue: VecDeque<PathBuf> = absolute
        .components()
        .map(|component| PathBuf::from(component.as_os_str()))
        .collect();
    let mut resolved = PathBuf::new();
    let mut last_link: Option<(PathBuf, PathBuf)> = None;
    let mut hops = 0;
    while let Some(part) = queue.pop_front() {
        match part.components().next() {
            Some(Component::RootDir | Component::Prefix(_)) => {
                resolved = part;
                continue;
            }
            Some(Component::CurDir) | None => continue,
            Some(Component::ParentDir) => {
                resolved.pop();
                continue;
            }
            Some(Component::Normal(_)) => {}
        }
        let candidate = resolved.join(&part);
        match fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                hops += 1;
                let target = fs::read_link(&candidate).map_err(|_| CachePathStatus::Missing {
                    path: candidate.clone(),
                })?;
                if hops > MAX_LINK_HOPS {
                    return Err(CachePathStatus::Missing { path: candidate });
                }
                let rest: Vec<PathBuf> = queue.drain(..).collect();
                queue.extend(
                    target
                        .components()
                        .map(|component| PathBuf::from(component.as_os_str())),
                );
                queue.extend(rest);
                last_link = Some((candidate, target));
            }
            Ok(_) => resolved = candidate,
            Err(_) if resolved == Path::new("/Volumes") => {
                let (symlink_path, target_path) =
                    last_link.unwrap_or_else(|| (path.to_path_buf(), path.to_path_buf()));
                return Err(CachePathStatus::UnmountedExternalVolume {
                    symlink_path,
                    target_path,
                });
            }
            Err(_) => return Err(CachePathStatus::Missing { path: candidate }),
        }
    }
    Ok(resolved)
}

/// Report each detached cache volume once per process instead of on every scan.
fn note_unusable_root(base: &Path, status: &CachePathStatus) {
    static REPORTED: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());
    let CachePathStatus::UnmountedExternalVolume {
        symlink_path,
        target_path,
    } = status
    else {
        return;
    };
    let first = REPORTED
        .lock()
        .map(|mut reported| reported.insert(base.to_path_buf()))
        .unwrap_or(false);
    if first {
        tracing::warn!(
            cache_root = %base.display(),
            symlink = %symlink_path.display(),
            target = %target_path.display(),
            "hf cache root skipped: external volume is not mounted"
        );
    }
}

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
pub(crate) fn cache_bases() -> Vec<PathBuf> {
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
/// A detached external root is skipped (and reported once); a required file
/// counts only when its link chain ends inside the cache root.
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
    let trusted = match trusted_roots(base) {
        Ok(trusted) => trusted,
        Err(status) => {
            note_unusable_root(base, &status);
            return None;
        }
    };
    let ready = |file: PathBuf| classify_snapshot_file(&trusted, &file).is_ready();
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
        if !required_all.iter().all(|f| ready(path.join(f))) {
            continue;
        }
        if !required_any.is_empty() && !required_any.iter().any(|f| ready(path.join(f))) {
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

/// Newest cached snapshot for every whisper-family repo in one cache root.
///
/// Listing only: no completeness filtering (callers validate), and only snapshots that hold at
/// least one weight file. A tokenizer-only companion repo is not a model and is
/// never listed; a quantized repo keeps its weights file and stays visible so
/// the caller can report the refusal reason.
pub(crate) fn whisper_cache_snapshots(base: &Path) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    let trusted = match trusted_roots(base) {
        Ok(trusted) => trusted,
        Err(status) => {
            note_unusable_root(base, &status);
            return out;
        }
    };
    let Ok(entries) = fs::read_dir(base) else {
        return out;
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
                    .any(
                        |weights| match classify_snapshot_file(&trusted, &path.join(weights)) {
                            CachePathStatus::Ready { resolved } => resolved.is_file(),
                            _ => false,
                        },
                    )
            })
            .max_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
        if let Some((_, path)) = newest {
            out.push((repo, path));
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

    /// `hf` 1.32 Xet layout: `snapshots/<rev>/<file> -> ../../blobs/<sha>`,
    /// whose blob is itself `-> ../../blobs/<2ch>/<sha>` under the hub.
    fn xet_snapshot(hub: &Path, repo: &str, rev: &str, file: &str, sha: &str) -> PathBuf {
        use std::os::unix::fs::symlink;
        let repo_dir = hub.join(format!("models--{}", repo.replace('/', "--")));
        let snapshot = repo_dir.join("snapshots").join(rev);
        let shard = hub.join("blobs").join(&sha[..2]);
        fs::create_dir_all(&snapshot).unwrap();
        fs::create_dir_all(repo_dir.join("blobs")).unwrap();
        fs::create_dir_all(&shard).unwrap();
        fs::write(shard.join(sha), b"weights").unwrap();
        symlink(
            format!("../../blobs/{}/{sha}", &sha[..2]),
            repo_dir.join("blobs").join(sha),
        )
        .unwrap();
        symlink(format!("../../blobs/{sha}"), snapshot.join(file)).unwrap();
        snapshot
    }

    #[test]
    fn two_level_xet_symlink_resolves_inside_the_cache_root() {
        let temp = TempDir::new().unwrap();
        let hub = temp.path().join("huggingface").join("hub");
        let sha = "ab12cd34";
        let snapshot = xet_snapshot(&hub, "org/repo", "rev", "model.bin", sha);

        let status =
            classify_snapshot_file(&trusted_roots(&hub).unwrap(), &snapshot.join("model.bin"));
        let expected = hub
            .join("blobs")
            .join("ab")
            .join(sha)
            .canonicalize()
            .unwrap();
        assert_eq!(status, CachePathStatus::Ready { resolved: expected });

        let found = find_snapshot_in_bases_matching(
            [hub.clone()],
            "org/repo",
            &["model.bin"],
            &[],
            &|_| true,
        );
        assert_eq!(found, Some(snapshot));
    }

    #[test]
    fn snapshot_file_linked_outside_the_cache_root_is_not_ready() {
        let temp = TempDir::new().unwrap();
        let elsewhere = TempDir::new().unwrap();
        let hub = temp.path().join("hub");
        let snapshot = hub.join("models--org--repo").join("snapshots").join("rev");
        fs::create_dir_all(&snapshot).unwrap();
        fs::write(elsewhere.path().join("model.bin"), b"weights").unwrap();
        std::os::unix::fs::symlink(
            elsewhere.path().join("model.bin"),
            snapshot.join("model.bin"),
        )
        .unwrap();

        let status =
            classify_snapshot_file(&trusted_roots(&hub).unwrap(), &snapshot.join("model.bin"));
        assert!(
            matches!(status, CachePathStatus::OutsideCacheRoot { .. }),
            "{status:?}"
        );
        let found =
            find_snapshot_in_bases_matching([hub], "org/repo", &["model.bin"], &[], &|_| true);
        assert_eq!(found, None);
    }

    const DETACHED_VOLUME: &str = "/Volumes/NonExistentExternalDrive";

    #[test]
    fn dangling_link_into_a_detached_volume_is_classified_by_name() {
        assert!(fs::symlink_metadata(DETACHED_VOLUME).is_err());
        let temp = TempDir::new().unwrap();
        let hub = temp.path().canonicalize().unwrap().join("hub");
        let snapshot = hub.join("models--org--repo").join("snapshots").join("rev");
        fs::create_dir_all(&snapshot).unwrap();
        let link = snapshot.join("model.bin");
        let target = PathBuf::from(DETACHED_VOLUME).join("model.bin");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        assert_eq!(
            classify_snapshot_file(&trusted_roots(&hub).unwrap(), &link),
            CachePathStatus::UnmountedExternalVolume {
                symlink_path: link,
                target_path: target,
            }
        );
        let found =
            find_snapshot_in_bases_matching([hub], "org/repo", &["model.bin"], &[], &|_| true);
        assert_eq!(found, None);
    }

    #[test]
    fn cache_root_linked_to_a_detached_volume_is_skipped() {
        assert!(fs::symlink_metadata(DETACHED_VOLUME).is_err());
        let temp = TempDir::new().unwrap();
        let home = temp.path().canonicalize().unwrap().join("huggingface");
        let target = PathBuf::from(DETACHED_VOLUME).join("huggingface");
        std::os::unix::fs::symlink(&target, &home).unwrap();
        let hub = home.join("hub");

        assert_eq!(
            classify_cache_root(&hub),
            CachePathStatus::UnmountedExternalVolume {
                symlink_path: home,
                target_path: target,
            }
        );
        assert!(whisper_cache_snapshots(&hub).is_empty());
        let found =
            find_snapshot_in_bases_matching([hub], "org/repo", &["config.json"], &[], &|_| true);
        assert_eq!(found, None);
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
