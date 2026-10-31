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
//! [`CachePathStatus::UnmountedExternalVolume`] using cached mount information,
//! so a detached drive is named instead of an anonymous I/O failure.

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

/// Maximum links followed while resolving one cache path.
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
    /// Nothing exists where the path resolves.
    Missing { path: PathBuf },
    /// A link cycle, or a chain exceeding the bounded resolver budget.
    SymlinkLoop { path: PathBuf },
    /// An existing path cannot be inspected. Permission and I/O errors are
    /// never reported as a missing file or an unmounted drive.
    Io {
        path: PathBuf,
        kind: std::io::ErrorKind,
        message: String,
    },
    /// The path resolves to an existing entry outside every trusted root.
    OutsideCacheRoot { resolved: PathBuf },
}

impl std::fmt::Display for CachePathStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ready { resolved } => write!(f, "Cache path ready at {}", resolved.display()),
            Self::UnmountedExternalVolume {
                symlink_path,
                target_path,
            } => write!(
                f,
                "External volume is not mounted: {} points to {}",
                symlink_path.display(),
                target_path.display()
            ),
            Self::Missing { path } => write!(f, "Missing cache/model path: {}", path.display()),
            Self::SymlinkLoop { path } => write!(f, "Cache/model symlink loop: {}", path.display()),
            Self::Io { path, message, .. } => {
                write!(f, "Cannot read {}: {message}", path.display())
            }
            Self::OutsideCacheRoot { resolved } => write!(
                f,
                "Cache file resolves outside the selected cache root: {}",
                resolved.display()
            ),
        }
    }
}

impl std::error::Error for CachePathStatus {}

fn path_error(path: &Path, error: std::io::Error) -> CachePathStatus {
    if error.kind() == std::io::ErrorKind::NotFound {
        CachePathStatus::Missing {
            path: path.to_path_buf(),
        }
    } else {
        CachePathStatus::Io {
            path: path.to_path_buf(),
            kind: error.kind(),
            message: error.to_string(),
        }
    }
}

/// Resolve the selected cache trust anchor and retain read/permission errors.
pub(crate) fn classify_cache_root(base: &Path) -> CachePathStatus {
    match resolve_links(base) {
        Ok(resolved) => match fs::read_dir(&resolved) {
            Ok(_) => CachePathStatus::Ready { resolved },
            Err(error) => path_error(&resolved, error),
        },
        Err(status) => status,
    }
}

/// Only the explicitly selected root is trusted. A directory named `hub`
/// never implicitly authorizes its parent or sibling caches.
fn trusted_roots(base: &Path) -> Result<Vec<PathBuf>, CachePathStatus> {
    match classify_cache_root(base) {
        CachePathStatus::Ready { resolved } => Ok(vec![resolved]),
        status => Err(status),
    }
}

/// Whether a root was explicitly configured rather than discovered by default.
pub(crate) fn is_configured_cache_base(base: &Path) -> bool {
    [
        "CODESCRIBE_HF_CACHE",
        "HUGGINGFACE_HUB_CACHE",
        "HF_HUB_CACHE",
    ]
    .iter()
    .filter_map(env::var_os)
    .any(|value| Path::new(&value) == base)
        || env::var_os("HF_HOME").is_some_and(|home| Path::new(&home).join("hub") == base)
}

/// Read the kernel's cached mount table; this never requests filesystem I/O.
#[cfg(target_os = "macos")]
fn external_volume_is_mounted(volume: &Path) -> Result<bool, CachePathStatus> {
    use std::os::unix::ffi::OsStrExt;
    // SAFETY: a null buffer and zero size request only the cached mount count.
    let count = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
    if count <= 0 {
        let error = std::io::Error::last_os_error();
        return Err(CachePathStatus::Io {
            path: volume.to_path_buf(),
            kind: error.kind(),
            message: format!("Cannot read cached mount table: {error}"),
        });
    }
    let mut mounts = Vec::<std::mem::MaybeUninit<libc::statfs>>::with_capacity(count as usize + 8);
    let bytes = mounts.capacity() * std::mem::size_of::<libc::statfs>();
    let bytes = libc::c_int::try_from(bytes).map_err(|error| CachePathStatus::Io {
        path: volume.to_path_buf(),
        kind: std::io::ErrorKind::Other,
        message: error.to_string(),
    })?;
    // SAFETY: the allocation holds `bytes` writable bytes. Only returned entries
    // are read below. MNT_NOWAIT uses kernel data, including for stalled mounts.
    let written = unsafe { libc::getfsstat(mounts.as_mut_ptr().cast(), bytes, libc::MNT_NOWAIT) };
    if written < 0 {
        let error = std::io::Error::last_os_error();
        return Err(CachePathStatus::Io {
            path: volume.to_path_buf(),
            kind: error.kind(),
            message: format!("Cannot read cached mount table: {error}"),
        });
    }
    if written as usize >= mounts.capacity() {
        return Err(CachePathStatus::Io {
            path: volume.to_path_buf(),
            kind: std::io::ErrorKind::Other,
            message: "Cached mount table changed during inspection; retry discovery".into(),
        });
    }
    let initialized = (written as usize).min(mounts.capacity());
    // SAFETY: getfsstat initialized the first `initialized` entries.
    unsafe { mounts.set_len(initialized) };
    Ok(mounts.iter().any(|mount| {
        // SAFETY: only initialized entries are present in this vector.
        let mount = unsafe { mount.assume_init_ref() };
        let end = mount
            .f_mntonname
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(mount.f_mntonname.len());
        mount.f_mntonname[..end]
            .iter()
            .map(|byte| *byte as u8)
            .eq(volume.as_os_str().as_bytes().iter().copied())
    }))
}

#[cfg(not(target_os = "macos"))]
fn external_volume_is_mounted(_volume: &Path) -> Result<bool, CachePathStatus> {
    // /Volumes is a macOS namespace; do not probe it on another platform.
    Ok(false)
}

fn guard_external_target(
    path: &Path,
    last_link: Option<&(PathBuf, PathBuf)>,
) -> Result<(), CachePathStatus> {
    let Ok(relative) = path.strip_prefix("/Volumes") else {
        return Ok(());
    };
    let Some(Component::Normal(name)) = relative.components().next() else {
        return Ok(());
    };
    let volume = Path::new("/Volumes").join(name);
    if external_volume_is_mounted(&volume)? {
        return Ok(());
    }
    // A local mount-namespace entry may itself be a symlink (for example the
    // startup disk alias). read_link does not follow its target; let the normal
    // component walker resolve it rather than falsely calling it unmounted.
    // Read failures other than absence/not-a-link remain distinct I/O errors.
    match fs::read_link(&volume) {
        Ok(_) => return Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidInput
            ) => {}
        Err(error) => return Err(path_error(&volume, error)),
    }
    let (symlink_path, target_path) = last_link
        .cloned()
        .unwrap_or_else(|| (path.to_path_buf(), path.to_path_buf()));
    Err(CachePathStatus::UnmountedExternalVolume {
        symlink_path,
        target_path,
    })
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
/// Mount presence is checked before lstat enters `/Volumes/<name>`. A mounted
/// filesystem can still stall; callers retain the existing background I/O owner.
fn resolve_links(path: &Path) -> Result<PathBuf, CachePathStatus> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .map_err(|error| path_error(path, error))?
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
        guard_external_target(&candidate, last_link.as_ref())?;
        match fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                hops += 1;
                let target =
                    fs::read_link(&candidate).map_err(|error| path_error(&candidate, error))?;
                if hops > MAX_LINK_HOPS {
                    return Err(CachePathStatus::SymlinkLoop { path: candidate });
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
            Err(error) => return Err(path_error(&candidate, error)),
        }
    }
    Ok(resolved)
}

/// Canonical location only; this makes no claim about physical media or NVMe.
pub(crate) fn resolve_local_path(path: &Path) -> Result<PathBuf, CachePathStatus> {
    resolve_links(path)
}

/// Preflight all bundle entries, including optional tokenizer/metadata links,
/// before the loader's validator can follow one into an unavailable volume.
pub(crate) fn validate_model_directory(path: &Path) -> Result<PathBuf, CachePathStatus> {
    // Keep the declared cache boundary when the caller passed a snapshot path.
    // Explicit local model paths remain their own selection, not a guessed HF home.
    if let Some(base) = cache_bases()
        .into_iter()
        .filter(|base| path.starts_with(base))
        .max_by_key(|base| base.components().count())
    {
        return validate_snapshot_directory(&base, path);
    }
    let resolved = resolve_links(path)?;
    for entry in fs::read_dir(&resolved).map_err(|error| path_error(&resolved, error))? {
        let entry = entry.map_err(|error| path_error(&resolved, error))?;
        resolve_links(&entry.path())?;
    }
    Ok(resolved)
}

fn validate_snapshot_directory(base: &Path, path: &Path) -> Result<PathBuf, CachePathStatus> {
    let trusted = trusted_roots(base)?;
    let resolved = match classify_snapshot_file(&trusted, path) {
        CachePathStatus::Ready { resolved } => resolved,
        status => return Err(status),
    };
    for entry in fs::read_dir(&resolved).map_err(|error| path_error(&resolved, error))? {
        let entry = entry.map_err(|error| path_error(&resolved, error))?;
        match classify_snapshot_file(&trusted, &entry.path()) {
            CachePathStatus::Ready { .. } => {}
            status => return Err(status),
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

/// The same finder with full-validation errors retained when no healthy root
/// supplies a model. A broken earlier root never hides a valid later one.
/// `anyhow` preserves the original CachePathStatus and validator error types.
pub(crate) fn find_snapshot_with_any_matching_checked<F>(
    repo: &str,
    required_all: &[&str],
    required_any: &[&str],
    predicate: F,
) -> anyhow::Result<Option<PathBuf>>
where
    F: Fn(&Path) -> anyhow::Result<bool>,
{
    find_snapshot_in_bases_matching_checked(
        cache_bases(),
        repo,
        required_all,
        required_any,
        &predicate,
    )
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
    find_snapshot_in_bases_matching_checked(bases, repo, required_all, required_any, &|path| {
        Ok(predicate(path))
    })
    .ok()
    .flatten()
}

/// A concrete refusal must not be hidden by a missing artifact in another
/// incomplete snapshot/root. Among equally specific failures retain the first,
/// preserving selection order without replacing the original error by text.
fn retain_snapshot_failure(failure: &mut Option<anyhow::Error>, error: anyhow::Error) {
    let is_missing = |error: &anyhow::Error| {
        matches!(
            error.downcast_ref::<CachePathStatus>(),
            Some(CachePathStatus::Missing { .. })
        )
    };
    if failure.is_none()
        || failure
            .as_ref()
            .is_some_and(|previous| is_missing(previous) && !is_missing(&error))
    {
        *failure = Some(error);
    }
}

fn find_snapshot_in_bases_matching_checked<I, F>(
    bases: I,
    repo: &str,
    required_all: &[&str],
    required_any: &[&str],
    predicate: &F,
) -> anyhow::Result<Option<PathBuf>>
where
    I: IntoIterator<Item = PathBuf>,
    F: Fn(&Path) -> anyhow::Result<bool>,
{
    let mut failure = None;
    for base in bases {
        match find_snapshot_in_base_matching(&base, repo, required_all, required_any, predicate) {
            Ok(Some(snapshot)) => return Ok(Some(snapshot)),
            Ok(None) => {}
            Err(error) => retain_snapshot_failure(&mut failure, error),
        }
    }
    match failure {
        Some(status) => Err(status),
        None => Ok(None),
    }
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
) -> anyhow::Result<Option<PathBuf>>
where
    F: Fn(&Path) -> anyhow::Result<bool>,
{
    let trusted = match trusted_roots(base) {
        Ok(trusted) => trusted,
        Err(status) => {
            note_unusable_root(base, &status);
            return if matches!(status, CachePathStatus::Missing { .. })
                && !is_configured_cache_base(base)
            {
                Ok(None)
            } else {
                Err(status.into())
            };
        }
    };
    let ready = |file: PathBuf| match classify_snapshot_file(&trusted, &file) {
        CachePathStatus::Ready { resolved } => fs::metadata(&resolved)
            .map(|metadata| metadata.is_file())
            .map_err(|error| path_error(&resolved, error)),
        status => Err(status),
    };
    let repo_dir = base.join(format!("models--{}", repo.replace('/', "--")));
    let snapshots_dir = repo_dir.join("snapshots");

    let snapshots_dir = match classify_snapshot_file(&trusted, &snapshots_dir) {
        CachePathStatus::Ready { .. } => snapshots_dir,
        CachePathStatus::Missing { .. } => {
            // Case-insensitive repo match fallback (HF cache uses original casing)
            let target = repo.to_ascii_lowercase();
            let mut matched: Option<PathBuf> = None;
            for entry in fs::read_dir(base).map_err(|error| path_error(base, error))? {
                let entry = entry.map_err(|error| path_error(base, error))?;
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
            let Some(matched) = matched else {
                return Ok(None);
            };
            match classify_snapshot_file(&trusted, &matched) {
                CachePathStatus::Ready { .. } => matched,
                CachePathStatus::Missing { .. } => return Ok(None),
                status => return Err(status.into()),
            }
        }
        status => return Err(status.into()),
    };

    let entries =
        fs::read_dir(&snapshots_dir).map_err(|error| path_error(&snapshots_dir, error))?;

    let mut candidates = Vec::new();
    let mut failure = None;

    for entry in entries {
        let entry = entry.map_err(|error| path_error(&snapshots_dir, error))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| path_error(&path, error))?;
        if !file_type.is_dir() && !file_type.is_symlink() {
            continue;
        }
        match classify_snapshot_file(&trusted, &path) {
            CachePathStatus::Ready { resolved } => {
                if !fs::metadata(&resolved)
                    .map_err(|error| path_error(&resolved, error))?
                    .is_dir()
                {
                    continue;
                }
            }
            status => {
                retain_snapshot_failure(&mut failure, status.into());
                continue;
            }
        }
        let mut complete = true;
        for file in required_all {
            match ready(path.join(file)) {
                Ok(true) => {}
                Ok(false) => complete = false,
                Err(status) => {
                    retain_snapshot_failure(&mut failure, status.into());
                    complete = false;
                }
            }
        }
        if !required_any.is_empty() {
            let mut any_ready = false;
            let mut any_failure = None;
            for file in required_any {
                match ready(path.join(file)) {
                    Ok(true) => any_ready = true,
                    Ok(false) => {}
                    Err(status) => {
                        retain_snapshot_failure(&mut any_failure, status.into());
                    }
                }
            }
            if !any_ready {
                complete = false;
                if let Some(error) = any_failure {
                    retain_snapshot_failure(&mut failure, error);
                }
            }
        }
        if !complete {
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
    for (_, path) in candidates {
        match predicate(&path) {
            Ok(true) => return Ok(Some(path)),
            Ok(false) => {}
            Err(error) => retain_snapshot_failure(&mut failure, error),
        }
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(None),
    }
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
        match classify_snapshot_file(&trusted, &snapshots_dir) {
            CachePathStatus::Ready { .. } => {}
            CachePathStatus::Missing { .. } => continue,
            _ => {
                // Keep a refused Whisper row instead of silently dropping the
                // repo when its snapshots directory points at a detached disk.
                out.push((repo, snapshots_dir));
                continue;
            }
        }
        let revisions = match fs::read_dir(&snapshots_dir) {
            Ok(revisions) => revisions,
            Err(_) => {
                // The canonical validator will retain the typed read failure.
                out.push((repo, snapshots_dir));
                continue;
            }
        };
        let newest = revisions
            .flatten()
            .filter(|revision| {
                revision
                    .file_type()
                    .map(|kind| kind.is_dir() || kind.is_symlink())
                    .unwrap_or(true)
            })
            .map(|revision| {
                let modified = revision
                    .path()
                    .symlink_metadata()
                    .and_then(|metadata| metadata.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                (modified, revision.path())
            })
            .filter(|(_, path)| match classify_snapshot_file(&trusted, path) {
                CachePathStatus::Ready { resolved } => {
                    // lstat retains broken/inaccessible weight links for the
                    // caller's typed refusal; no detached target is followed.
                    crate::whisper_weights::SUPPORTED_NAMES
                        .iter()
                        .any(
                            |weights| match fs::symlink_metadata(resolved.join(weights)) {
                                Ok(_) => true,
                                Err(error) => error.kind() != std::io::ErrorKind::NotFound,
                            },
                        )
                }
                _ => true,
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
    fn checked_finder_refuses_hub_sibling_escape_and_accepts_later_root() {
        let temp = TempDir::new().unwrap();
        let hub = temp.path().join("hub");
        let invalid = snapshot(&hub, "org/repo", "rev", 10);
        let sibling = temp.path().join("outside.bin");
        fs::write(&sibling, b"outside").unwrap();
        std::os::unix::fs::symlink(&sibling, invalid.join("model.bin")).unwrap();
        let found = find_snapshot_in_bases_matching_checked(
            [hub.clone()],
            "org/repo",
            &["model.bin"],
            &[],
            &|_| Ok(true),
        );
        assert!(
            matches!(
                found
                    .as_ref()
                    .err()
                    .and_then(|error| error.downcast_ref::<CachePathStatus>()),
                Some(CachePathStatus::OutsideCacheRoot { .. })
            ),
            "{found:?}"
        );
        let later = temp.path().join("later");
        let valid = snapshot(&later, "org/repo", "good", 10);
        fs::write(valid.join("model.bin"), b"valid").unwrap();
        assert_eq!(
            find_snapshot_in_bases_matching_checked(
                [hub, later],
                "org/repo",
                &["model.bin"],
                &[],
                &|_| Ok(true)
            )
            .unwrap(),
            Some(valid)
        );
    }

    #[test]
    fn checked_finder_does_not_accept_directory_as_required_file() {
        let temp = TempDir::new().unwrap();
        let invalid = snapshot(temp.path(), "org/repo", "rev", 10);
        fs::create_dir(invalid.join("model.bin")).unwrap();
        assert_eq!(
            find_snapshot_in_bases_matching_checked(
                [temp.path().to_path_buf()],
                "org/repo",
                &["model.bin"],
                &[],
                &|_| Ok(true)
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn resolution_retains_loop_and_non_directory_io_causes() {
        let temp = TempDir::new().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        std::os::unix::fs::symlink(&second, &first).unwrap();
        std::os::unix::fs::symlink(&first, &second).unwrap();
        assert!(matches!(
            resolve_local_path(&first),
            Err(CachePathStatus::SymlinkLoop { .. })
        ));
        let file = temp.path().join("file");
        fs::write(&file, b"not a directory").unwrap();
        assert!(matches!(
            resolve_local_path(&file.join("child")),
            Err(CachePathStatus::Io {
                kind: std::io::ErrorKind::NotADirectory,
                ..
            })
        ));
    }

    #[test]
    fn permission_denial_is_not_reported_as_missing_or_detached() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new().unwrap();
        let locked = temp.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0)).unwrap();
        let status = classify_cache_root(&locked);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            matches!(
                status,
                CachePathStatus::Io {
                    kind: std::io::ErrorKind::PermissionDenied,
                    ..
                }
            ),
            "{status:?}"
        );
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
