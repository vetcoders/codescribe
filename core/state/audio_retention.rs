//! Completed-take audio expiration, owned by history. No filename-age inference.
//! Capture/processing leases exclude maintenance across processes. A receipt
//! admits only the exact files published by one application capture.

use super::daily_archive::{open_at, parent, subdir};
use crate::config::{AudioRetention, Config};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{error, info, warn};

const SCHEMA: &str = "codescribe.audio-retention.v1";
const SUFFIX: &str = ".audio-retention.json";
const PASS_ENTRIES: usize = 128;
const MAX_RECEIPT_BYTES: u64 = 64 * 1024;

/// Shared lease for an owned audio read or retry. Hold until all readers and
/// path-based retry work have settled. External input files are never admitted.
#[derive(Debug)]
pub struct AudioReadLease(File);

impl AudioReadLease {
    /// Pin an existing input while holding the configured root's shared lock.
    /// Resolve aliases only after acquisition. External inputs release the
    /// temporary lock and are never registered as captured audio.
    pub fn acquire_for_path(root: &Path, path: &Path) -> Result<(PathBuf, Option<Self>)> {
        let lease = match Self::acquire_existing(root) {
            Ok(lease) => lease,
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                return Ok((path.canonicalize()?, None));
            }
            Err(error) => return Err(error.context("acquire audio input lease")),
        };
        let pinned = lease.0.metadata()?;
        let resolved_root = root.canonicalize()?;
        let resolved_path = path.canonicalize()?;
        let current = std::fs::metadata(&resolved_root)?;
        anyhow::ensure!(
            pinned.dev() == current.dev() && pinned.ino() == current.ino(),
            "audio input root changed during resolution"
        );
        if resolved_path.starts_with(&resolved_root) {
            Ok((resolved_path, Some(lease)))
        } else {
            Ok((resolved_path, None))
        }
    }

    /// Acquire on a blocking worker without creating any storage directory.
    /// Use before resolving a session id or enumerating archived audio names.
    pub fn acquire_existing(root: &Path) -> Result<Self> {
        let (directory, leaf) = parent(root)?;
        let file = open_at(&directory, &leaf, libc::O_RDONLY | libc::O_DIRECTORY)?;
        // SAFETY: flock operates on this owned descriptor.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH) } < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(Self(file))
    }

    /// Acquire in a blocking worker before opening owned audio. The process's
    /// captures use the same lock; deletion takes its exclusive, nonwaiting side.
    pub fn acquire(root: &Path) -> Result<Self> {
        // Lock the pinned root inode itself; a replaceable lock-file name
        // cannot bypass an active reader/capture lease.
        let file = root_dir(root)?;
        // SAFETY: flock operates on this owned descriptor.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH) } < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(Self(file))
    }
}

impl Drop for AudioReadLease {
    fn drop(&mut self) {
        // SAFETY: this descriptor is still held by the lease.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OwnedFile {
    relative: String,
    device: u64,
    inode: u64,
    length: u64,
    modified: i64,
    modified_ns: i64,
    parent_device: u64,
    parent_inode: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct Receipt {
    schema: String,
    session_id: String,
    completed_at: u64,
    capture_policy: AudioRetention,
    root_device: u64,
    root_inode: u64,
    files: Vec<OwnedFile>,
    retry_protected: bool,
}

#[derive(Debug)]
struct CaptureFiles {
    files: Vec<OwnedFile>,
    retry_protected: bool,
}

/// Storage lease, not a second capture or transcript identity authority.
#[derive(Debug)]
pub struct CaptureLease {
    root: PathBuf,
    directory: File,
    session_id: String,
    policy: AudioRetention,
    lease: Option<AudioReadLease>,
    files: Mutex<CaptureFiles>,
}

type Captures = HashMap<(PathBuf, String), Arc<CaptureLease>>;
fn captures() -> &'static Mutex<Captures> {
    static CAPTURES: OnceLock<Mutex<Captures>> = OnceLock::new();
    CAPTURES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn root_dir(root: &Path) -> Result<File> {
    let (directory, leaf) = parent(root)?;
    subdir(&directory, &leaf)
}

fn valid_id(id: &str) -> bool {
    (8..=80).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// Called before microphone admission, with the same immutable runtime snapshot
/// as the recorder. Failure refuses capture; it cannot silently opt out.
pub fn begin_capture(root: &Path, id: &str, policy: AudioRetention) -> Result<()> {
    anyhow::ensure!(valid_id(id), "unsafe audio capture id");
    let lease = AudioReadLease::acquire(root)?;
    let directory = lease.0.try_clone()?;
    let mut active = captures()
        .lock()
        .map_err(|_| anyhow::anyhow!("audio capture registry poisoned"))?;
    let key = (root.to_path_buf(), id.to_string());
    anyhow::ensure!(!active.contains_key(&key), "duplicate audio capture lease");
    active.insert(
        key,
        Arc::new(CaptureLease {
            root: root.to_path_buf(),
            directory,
            session_id: id.to_string(),
            policy,
            lease: Some(lease),
            files: Mutex::new(CaptureFiles {
                files: Vec::new(),
                retry_protected: false,
            }),
        }),
    );
    Ok(())
}

/// Clone before queuing archive I/O, so a terminal reset cannot outrun it.
pub fn capture(root: &Path, id: &str) -> Option<Arc<CaptureLease>> {
    captures()
        .lock()
        .ok()?
        .get(&(root.to_path_buf(), id.to_string()))
        .cloned()
}

/// The controller terminal owner releases its capture. Queued archive workers
/// and read leases still hold the shared lock; no deletion runs until they end.
pub fn finish_capture(root: &Path, id: &str) {
    if let Ok(mut active) = captures().lock() {
        active.remove(&(root.to_path_buf(), id.to_string()));
    }
}

impl CaptureLease {
    /// Pin file ownership from the held application root after publication.
    /// Unknown paths and changed parents never enter a deletion receipt.
    pub fn record(&self, paths: &[PathBuf], retry_protected: bool) -> Result<()> {
        let mut state = self
            .files
            .lock()
            .map_err(|_| anyhow::anyhow!("audio receipt poisoned"))?;
        state.retry_protected |= retry_protected;
        for path in paths {
            let relative = path
                .strip_prefix(&self.root)
                .context("external audio is not owned")?;
            let relative = relative.to_str().context("audio path is not UTF-8")?;
            anyhow::ensure!(
                admitted(relative, &self.session_id),
                "unadmitted audio path"
            );
            let (directory, name) = file_parent(&self.directory, relative)?;
            let file = open_at(&directory, &name, libc::O_RDONLY | libc::O_NONBLOCK)?;
            let metadata = file.metadata()?;
            anyhow::ensure!(metadata.is_file(), "owned audio must be regular");
            let parent = directory.metadata()?;
            let owned = OwnedFile {
                relative: relative.to_string(),
                device: metadata.dev(),
                inode: metadata.ino(),
                length: metadata.len(),
                modified: metadata.mtime(),
                modified_ns: metadata.mtime_nsec(),
                parent_device: parent.dev(),
                parent_inode: parent.ino(),
            };
            if !state
                .files
                .iter()
                .any(|file| file.relative == owned.relative)
            {
                state.files.push(owned);
            }
        }
        Ok(())
    }

    /// Failed capture/storage remains recovery evidence, including under Off.
    pub fn protect_retry(&self) {
        if let Ok(mut state) = self.files.lock() {
            state.retry_protected = true;
        }
    }
}

impl Drop for CaptureLease {
    fn drop(&mut self) {
        let Some(lease) = self.lease.take() else {
            return;
        };
        let state = match self.files.get_mut() {
            Ok(state) => state,
            Err(_) => {
                error!("audio retention receipt poisoned; preserving audio");
                return;
            }
        };
        if state.files.is_empty() {
            return;
        }
        let metadata = match self.directory.metadata() {
            Ok(metadata) => metadata,
            Err(error) => {
                error!(%error, "audio completion root unavailable; preserving audio");
                return;
            }
        };
        let completed_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|time| time.as_secs())
            .unwrap_or(0);
        let receipt = Receipt {
            schema: SCHEMA.to_string(),
            session_id: self.session_id.clone(),
            completed_at,
            capture_policy: self.policy,
            root_device: metadata.dev(),
            root_inode: metadata.ino(),
            files: std::mem::take(&mut state.files),
            retry_protected: state.retry_protected,
        };
        let directory = match self.directory.try_clone() {
            Ok(directory) => directory,
            Err(error) => {
                error!(%error, "audio completion descriptor unavailable");
                return;
            }
        };
        let root = self.root.clone();
        // No receipt write or directory walk runs on a UI/capture callback.
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                let _completion = runtime.spawn_blocking(move || {
                    if let Err(error) = publish_receipt(&directory, &receipt) {
                        error!(%error, "audio completion not recorded; preserving audio");
                    }
                    drop(lease);
                    run_maintenance(&root);
                });
            }
            Err(_) => error!("audio completion has no runtime worker; preserving audio"),
        }
    }
}

fn admitted(relative: &str, id: &str) -> bool {
    let parts: Vec<_> = relative.split('/').collect();
    match parts.as_slice() {
        ["takes", name] => name
            .strip_prefix("codescribe_recording_")
            .and_then(|value| value.strip_suffix(".wav"))
            .is_some_and(|value| {
                !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
            }),
        ["sessions", name] => *name == format!("{id}.wav"),
        ["transcriptions", date, name] => {
            chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok()
                && name.len() > 7
                && name.as_bytes()[..6].iter().all(u8::is_ascii_digit)
                && name.as_bytes()[6] == b'_'
                && (name.ends_with(".m4a") || name.ends_with(".wav"))
        }
        _ => false,
    }
}

fn file_parent(root: &File, relative: &str) -> Result<(File, CString)> {
    let path = Path::new(relative);
    anyhow::ensure!(
        path.components()
            .all(|part| matches!(part, Component::Normal(_))),
        "unsafe receipt path"
    );
    let mut directory = root.try_clone()?;
    let mut parts = relative.split('/').peekable();
    while let Some(part) = parts.next() {
        let name = CString::new(part)?;
        if parts.peek().is_none() {
            return Ok((directory, name));
        }
        directory = open_at(&directory, &name, libc::O_RDONLY | libc::O_DIRECTORY)?;
    }
    anyhow::bail!("empty receipt path")
}

fn publish_receipt(root: &File, receipt: &Receipt) -> Result<()> {
    let sessions = subdir(root, c"sessions")?;
    let temporary = CString::new(format!(".retention-{}.tmp", uuid::Uuid::new_v4()))?;
    let name = CString::new(format!("{}{SUFFIX}", receipt.session_id))?;
    let result = (|| -> Result<()> {
        let mut file = open_at(
            &sessions,
            &temporary,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
        )?;
        file.write_all(&serde_json::to_vec(receipt)?)?;
        file.sync_all()?;
        // SAFETY: single owned names relative to the pinned sessions directory.
        if unsafe {
            libc::linkat(
                sessions.as_raw_fd(),
                temporary.as_ptr(),
                sessions.as_raw_fd(),
                name.as_ptr(),
                0,
            )
        } < 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        sessions.sync_all()?;
        Ok(())
    })();
    // SAFETY: only this operation's staging name is removed.
    unsafe { libc::unlinkat(sessions.as_raw_fd(), temporary.as_ptr(), 0) };
    result
}

/// Maintenance result remains inspectable by the integrator and log consumers.
#[derive(Debug, Default)]
pub struct MaintenanceReport {
    /// Completion receipts visited during this bounded pass.
    pub examined: usize,
    /// Takes whose admitted audio names all expired.
    pub expired: usize,
    /// Receipts or a busy pass conservatively preserved.
    pub deferred: usize,
    /// Exact pending deletion/admission errors; empty does not prove a full scan.
    pub failures: Vec<String>,
}

/// Cursor owns a directory stream, not a pathname. A pass visits at most 128
/// entries and leaves its position for the next idle/timer pass (no starvation).
struct Cursor(*mut libc::DIR);
// SAFETY: accessed only under the cursors mutex; never used concurrently.
unsafe impl Send for Cursor {}
impl Drop for Cursor {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { libc::closedir(self.0) };
        }
    }
}
struct StoreCursor {
    device: u64,
    inode: u64,
    cursor: Cursor,
}
fn cursors() -> &'static Mutex<HashMap<PathBuf, StoreCursor>> {
    static CURSORS: OnceLock<Mutex<HashMap<PathBuf, StoreCursor>>> = OnceLock::new();
    CURSORS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// One bounded pass. Never scans take/daily directories or infers completion.
/// Call only from a blocking worker. Forever still settles prospective Off takes.
pub fn maintain(root: &Path, policy: AudioRetention, now: SystemTime) -> Result<MaintenanceReport> {
    let _data_io = crate::config::storage_reset::begin_app_data_io()?;
    let directory = root_dir(root)?;
    let lock = directory.try_clone()?;
    // SAFETY: nonblocking exclusive lease on this owned descriptor.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::WouldBlock {
            return Ok(MaintenanceReport {
                deferred: 1,
                ..Default::default()
            });
        }
        return Err(error.into());
    }
    let sessions = match open_at(&directory, c"sessions", libc::O_RDONLY | libc::O_DIRECTORY) {
        Ok(sessions) => sessions,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(MaintenanceReport::default());
        }
        Err(error) => return Err(error.into()),
    };
    let metadata = sessions.metadata()?;
    let mut all = cursors()
        .lock()
        .map_err(|_| anyhow::anyhow!("audio cursor poisoned"))?;
    let entry = all
        .entry(root.to_path_buf())
        .or_insert_with(|| StoreCursor {
            device: 0,
            inode: 0,
            cursor: Cursor(std::ptr::null_mut()),
        });
    if entry.device != metadata.dev() || entry.inode != metadata.ino() {
        // SAFETY: dup transfers an independent descriptor to fdopendir.
        let descriptor = unsafe { libc::dup(sessions.as_raw_fd()) };
        anyhow::ensure!(descriptor >= 0, "audio cursor descriptor unavailable");
        let stream = unsafe { libc::fdopendir(descriptor) };
        if stream.is_null() {
            unsafe { libc::close(descriptor) };
            return Err(std::io::Error::last_os_error().into());
        }
        entry.cursor = Cursor(stream);
        entry.device = metadata.dev();
        entry.inode = metadata.ino();
    }
    let mut report = MaintenanceReport::default();
    let now = now
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_secs())
        .unwrap_or(0);
    for _ in 0..PASS_ENTRIES {
        // SAFETY: mutex exclusively owns the live stream; copy name before next read.
        let Some(name) = read_cursor(&entry.cursor)? else {
            // SAFETY: the mutex owns this live stream exclusively.
            unsafe { libc::rewinddir(entry.cursor.0) };
            break;
        };
        let Some(receipt_name) = name.to_str().ok() else {
            continue;
        };
        let receipt_name = receipt_name
            .strip_prefix(".expiry-")
            .and_then(|name| name.strip_suffix(".tmp"))
            .unwrap_or(receipt_name);
        let Some(id) = receipt_name.strip_suffix(SUFFIX) else {
            continue;
        };
        if !valid_id(id) {
            continue;
        }
        report.examined += 1;
        match expire_receipt(&directory, &sessions, &name, id, policy, now) {
            Ok(true) => report.expired += 1,
            Ok(false) => report.deferred += 1,
            Err(error) => report
                .failures
                .push(format!("{}: {error:#}", name.to_string_lossy())),
        }
    }
    Ok(report)
}

fn read_cursor(cursor: &Cursor) -> Result<Option<CString>> {
    #[cfg(target_vendor = "apple")]
    let errno = unsafe { libc::__error() };
    #[cfg(target_os = "linux")]
    let errno = unsafe { libc::__errno_location() };
    #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
    anyhow::bail!("audio retention directory cursor requires a supported errno contract");
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    {
        // SAFETY: errno is thread-local; readdir's returned name belongs to this
        // mutex-owned stream and is copied before its next use.
        unsafe {
            *errno = 0;
            let next = libc::readdir(cursor.0);
            if next.is_null() {
                if *errno != 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
                return Ok(None);
            }
            Ok(Some(CStr::from_ptr((*next).d_name.as_ptr()).to_owned()))
        }
    }
}

fn expire_receipt(
    root: &File,
    sessions: &File,
    name: &CStr,
    id: &str,
    policy: AudioRetention,
    now: u64,
) -> Result<bool> {
    let file = open_at(sessions, name, libc::O_RDONLY | libc::O_NONBLOCK)?;
    let metadata = file.metadata()?;
    anyhow::ensure!(
        metadata.is_file() && metadata.len() <= MAX_RECEIPT_BYTES,
        "invalid audio receipt"
    );
    let mut bytes = Vec::new();
    file.take(MAX_RECEIPT_BYTES + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_RECEIPT_BYTES,
        "oversized audio receipt"
    );
    let receipt: Receipt = serde_json::from_slice(&bytes)?;
    let root_metadata = root.metadata()?;
    anyhow::ensure!(
        receipt.schema == SCHEMA
            && receipt.session_id == id
            && receipt.root_device == root_metadata.dev()
            && receipt.root_inode == root_metadata.ino()
            && receipt.files.len() <= 16,
        "unadmitted audio receipt"
    );
    if receipt.retry_protected || receipt.completed_at == 0 || now < receipt.completed_at {
        return Ok(false);
    }
    let due = receipt.capture_policy == AudioRetention::Off
        || policy
            .duration_seconds()
            .is_some_and(|seconds| now - receipt.completed_at >= seconds);
    if !due {
        return Ok(false);
    }
    // Prevalidate every member before removing any. Missing members are expected
    // after a partially successful pass; a changed inode protects the entire take.
    for file in &receipt.files {
        anyhow::ensure!(
            admitted(&file.relative, id),
            "receipt includes unowned audio"
        );
        verify_file(root, file)?;
    }
    remove_alias(root, id)?;
    for file in &receipt.files {
        remove_owned(root, file)?;
    }
    // Keep the receipt on any failure; retry reports the exact remaining members.
    let canonical_name = CString::new(format!("{id}{SUFFIX}"))?;
    remove_entry(
        sessions,
        &canonical_name,
        metadata.dev(),
        metadata.ino(),
        false,
    )?;
    info!(
        session_id = id,
        "completed take audio expired; text history retained"
    );
    Ok(true)
}

fn verify_file(root: &File, owned: &OwnedFile) -> Result<()> {
    let (directory, name) = file_parent(root, &owned.relative)?;
    let parent = directory.metadata()?;
    anyhow::ensure!(
        parent.dev() == owned.parent_device && parent.ino() == owned.parent_inode,
        "audio parent changed"
    );
    let pending = quarantine_name(&name)?;
    for name in [name.as_c_str(), pending.as_c_str()] {
        match open_at(&directory, name, libc::O_RDONLY | libc::O_NONBLOCK) {
            Ok(file) => {
                let metadata = file.metadata()?;
                anyhow::ensure!(
                    metadata.is_file()
                        && metadata.dev() == owned.device
                        && metadata.ino() == owned.inode
                        && metadata.len() == owned.length
                        && metadata.mtime() == owned.modified
                        && metadata.mtime_nsec() == owned.modified_ns,
                    "audio object changed; preserving retry evidence"
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn remove_owned(root: &File, owned: &OwnedFile) -> Result<()> {
    verify_file(root, owned)?;
    let (directory, name) = file_parent(root, &owned.relative)?;
    remove_entry(&directory, &name, owned.device, owned.inode, false)
}

fn stat_entry(directory: &File, name: &CStr) -> std::io::Result<libc::stat> {
    // SAFETY: initialized output and live directory/name, no symlink following.
    let mut metadata = unsafe { std::mem::zeroed::<libc::stat>() };
    if unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            name.as_ptr(),
            &mut metadata,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } < 0
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok(metadata)
}

fn quarantine_name(name: &CStr) -> Result<CString> {
    Ok(CString::new(format!(".expiry-{}.tmp", name.to_str()?))?)
}

fn matching_stat(metadata: &libc::stat, device: u64, inode: u64, kind: libc::mode_t) -> bool {
    metadata.st_dev as u64 == device
        && u128::from(metadata.st_ino) == u128::from(inode)
        && metadata.st_mode & libc::S_IFMT == kind
}

fn unlink_matching(
    directory: &File,
    name: &CStr,
    device: u64,
    inode: u64,
    kind: libc::mode_t,
) -> Result<()> {
    let metadata = stat_entry(directory, name)?;
    anyhow::ensure!(
        matching_stat(&metadata, device, inode, kind),
        "expiration entry changed; preserving evidence"
    );
    // SAFETY: one admitted leaf in the pinned parent; unlink never follows it.
    if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } < 0 {
        let error = std::io::Error::last_os_error();
        anyhow::bail!(
            "audio deletion pending at {}: {error}",
            name.to_string_lossy()
        );
    }
    Ok(())
}

fn remove_entry(
    directory: &File,
    name: &CStr,
    device: u64,
    inode: u64,
    symlink: bool,
) -> Result<()> {
    let kind = if symlink {
        libc::S_IFLNK
    } else {
        libc::S_IFREG
    };
    let quarantine = quarantine_name(name)?;
    match stat_entry(directory, &quarantine) {
        Ok(_) => unlink_matching(directory, &quarantine, device, inode, kind)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let metadata = match stat_entry(directory, name) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    anyhow::ensure!(
        matching_stat(&metadata, device, inode, kind),
        "expiration entry changed"
    );
    // SAFETY: move the leaf inside its held directory. Inspect the moved object
    // before unlinking; a substituted object is preserved under quarantine.
    rename_exclusive(directory, name, &quarantine)?;
    unlink_matching(directory, &quarantine, device, inode, kind)
}

fn rename_exclusive(directory: &File, source: &CStr, destination: &CStr) -> Result<()> {
    // SAFETY: single-component names and the pinned directory stay alive; the
    // platform call refuses an existing destination, including raced entries.
    #[cfg(target_vendor = "apple")]
    let result = unsafe {
        libc::renameatx_np(
            directory.as_raw_fd(),
            source.as_ptr(),
            directory.as_raw_fd(),
            destination.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::renameat2(
            directory.as_raw_fd(),
            source.as_ptr(),
            directory.as_raw_fd(),
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
    anyhow::bail!("audio expiration requires an exclusive rename contract");
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    {
        if result < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
}

fn remove_alias(root: &File, id: &str) -> Result<()> {
    // A failed alias deletion keeps a deterministic quarantine name. Check it
    // even when a successor has installed a different latest-take alias.
    let pending = quarantine_name(c"last_session.wav")?;
    for name in [pending.as_c_str(), c"last_session.wav"] {
        let metadata = match stat_entry(root, name) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if metadata.st_mode & libc::S_IFMT != libc::S_IFLNK {
            continue;
        }
        let mut target = [0u8; 256];
        // SAFETY: read one symlink leaf in the pinned root, without following it.
        let length = unsafe {
            libc::readlinkat(
                root.as_raw_fd(),
                name.as_ptr(),
                target.as_mut_ptr().cast(),
                target.len(),
            )
        };
        if length < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        if target.get(..length as usize) != Some(format!("sessions/{id}.wav").as_bytes()) {
            continue;
        }
        if name == pending.as_c_str() {
            let inode = u64::try_from(u128::from(metadata.st_ino))
                .context("audio alias inode exceeds receipt identity width")?;
            unlink_matching(root, name, metadata.st_dev as u64, inode, libc::S_IFLNK)?;
        } else {
            let inode = u64::try_from(u128::from(metadata.st_ino))
                .context("audio alias inode exceeds receipt identity width")?;
            remove_entry(root, name, metadata.st_dev as u64, inode, true)?;
        }
    }
    Ok(())
}

fn run_maintenance(root: &Path) {
    let result = (|| -> Result<MaintenanceReport> {
        let snapshot = Config::load_runtime_snapshot_without_keychain()?;
        anyhow::ensure!(
            snapshot.repair_receipt().unrepairable.is_empty(),
            "settings snapshot refused; preserving audio"
        );
        maintain(root, snapshot.values().audio_retention, SystemTime::now())
    })();
    match result {
        Ok(report) if !report.failures.is_empty() => {
            error!(failures = ?report.failures, "audio retention remains pending")
        }
        Ok(report) => info!(
            examined = report.examined,
            expired = report.expired,
            deferred = report.deferred,
            "audio retention maintenance"
        ),
        Err(error) => warn!(%error, "audio retention maintenance refused; preserving audio"),
    }
}

/// Startup and periodic bounded maintenance on the existing history owner.
/// Settings are read only through the canonical snapshot loader on the worker.
pub fn start_maintenance() {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let _maintenance = runtime.spawn(async {
        loop {
            let root = Config::config_dir();
            if let Err(error) = tokio::task::spawn_blocking(move || run_maintenance(&root)).await {
                error!(%error, "audio retention maintenance worker failed; preserving audio");
            }
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        }
    });
}

#[cfg(test)]
mod acceptance_tests {
    use super::*;
    use std::time::Duration;
    const ID: &str = "retention-take-1";

    fn isolated_child(name: &str) -> bool {
        if std::env::var("CS_PRIVATE_RETENTION_CHILD").ok().as_deref() == Some(name) {
            return false;
        }
        let root = tempfile::tempdir().unwrap();
        let test = format!(
            "{}::{name}",
            module_path!().strip_prefix("codescribe_core::").unwrap()
        );
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([test, "--exact".into(), "--test-threads=1".into()])
            .env("CS_PRIVATE_RETENTION_CHILD", name)
            .env("CODESCRIBE_DATA_DIR", root.path())
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&result.stdout);
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(
            result.status.success(),
            "isolated retention failed: {stdout}\n{stderr}"
        );
        assert!(
            stdout.contains("1 passed; 0 failed"),
            "the selected test must execute: {stdout}"
        );
        true
    }

    fn owned_take(policy: AudioRetention) -> (PathBuf, Arc<CaptureLease>, Vec<PathBuf>) {
        let root = Config::config_dir();
        begin_capture(&root, ID, policy).unwrap();
        std::fs::create_dir_all(root.join("takes")).unwrap();
        std::fs::create_dir_all(root.join("sessions")).unwrap();
        std::fs::create_dir_all(root.join("transcriptions/2026-10-02")).unwrap();
        let take = root.join("takes/codescribe_recording_1.wav");
        let mut writer = hound::WavWriter::create(
            &take,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for sample in [i16::MIN, 0, 123, i16::MAX] {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
        let session = root.join(format!("sessions/{ID}.wav"));
        std::fs::hard_link(&take, &session).unwrap();
        let daily = root.join("transcriptions/2026-10-02/120000_probe_raw.wav");
        std::fs::copy(&take, &daily).unwrap();
        std::fs::write(
            root.join("transcriptions/2026-10-02/120000_probe_raw.txt"),
            b"preserve text",
        )
        .unwrap();
        std::os::unix::fs::symlink(format!("sessions/{ID}.wav"), root.join("last_session.wav"))
            .unwrap();
        let files = vec![take, session, daily];
        let lease = capture(&root, ID).unwrap();
        lease.record(&files, false).unwrap();
        (root, lease, files)
    }

    async fn completed_at(root: &Path) -> SystemTime {
        let path = root.join(format!("sessions/{ID}{SUFFIX}"));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            if let Ok(bytes) = std::fs::read(&path) {
                let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                return UNIX_EPOCH + Duration::from_secs(value["completed_at"].as_u64().unwrap());
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "completion receipt did not publish"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    async fn expire_when_idle(
        root: &Path,
        policy: AudioRetention,
        now: SystemTime,
    ) -> MaintenanceReport {
        // Completion writes its receipt before dropping the producer lease.
        // Nonwaiting maintenance may also meet another bounded pass. Require
        // actual eventual expiry once idle, rather than treating deferred as
        // a deletion failure or dictating one-pass scheduling.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let report = maintain(root, policy, now).unwrap();
            assert!(report.failures.is_empty(), "{report:?}");
            if report.expired > 0 {
                return report;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "idle expiry never completed: {report:?}"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn alias_input_is_pinned_under_a_read_lease_until_the_owner_finishes() {
        if isolated_child("alias_input_is_pinned_under_a_read_lease_until_the_owner_finishes") {
            return;
        }
        let (root, capture_lease, files) = owned_take(AudioRetention::Off);
        let (pinned, reader) =
            AudioReadLease::acquire_for_path(&root, &root.join("last_session.wav")).unwrap();
        assert_eq!(pinned, files[1].canonicalize().unwrap());
        assert!(
            reader.is_some(),
            "an alias into owned storage must hold the root lock"
        );
        finish_capture(&root, ID);
        drop(capture_lease);
        let completed = completed_at(&root).await;
        let report = maintain(&root, AudioRetention::Forever, completed).unwrap();
        assert_eq!(report.expired, 0);
        assert!(report.deferred > 0);
        tokio::task::yield_now().await;
        assert_eq!(hound::WavReader::open(&pinned).unwrap().len(), 4);
        drop(reader);
        assert_eq!(
            expire_when_idle(&root, AudioRetention::Forever, completed)
                .await
                .expired,
            1
        );
        assert!(!pinned.exists());
    }

    #[test]
    fn external_inputs_never_hold_or_create_an_owned_audio_root() {
        let dir = tempfile::tempdir().unwrap();
        let external = dir.path().join("external.wav");
        std::fs::write(&external, b"external source remains independent").unwrap();
        let absent_root = dir.path().join("absent-store");
        let (resolved, lease) = AudioReadLease::acquire_for_path(&absent_root, &external).unwrap();
        assert_eq!(resolved, external.canonicalize().unwrap());
        assert!(lease.is_none());
        assert!(!absent_root.exists());
        let root = dir.path().join("existing-store");
        std::fs::create_dir(&root).unwrap();
        let (resolved, lease) = AudioReadLease::acquire_for_path(&root, &external).unwrap();
        assert_eq!(resolved, external.canonicalize().unwrap());
        assert!(lease.is_none());
        let maintenance = maintain(&root, AudioRetention::Off, SystemTime::now()).unwrap();
        assert_eq!(
            maintenance.deferred, 0,
            "external input cannot keep the owned root locked"
        );
        assert_eq!(
            std::fs::read(external).unwrap(),
            b"external source remains independent"
        );
    }

    #[tokio::test]
    async fn finite_policy_expires_every_owned_audio_copy_at_the_boundary_and_keeps_text() {
        if isolated_child(
            "finite_policy_expires_every_owned_audio_copy_at_the_boundary_and_keeps_text",
        ) {
            return;
        }
        let (root, lease, files) = owned_take(AudioRetention::Forever);
        finish_capture(&root, ID);
        drop(lease);
        let completed = completed_at(&root).await;
        let before = maintain(
            &root,
            AudioRetention::Hours24,
            completed + Duration::from_secs(86_399),
        )
        .unwrap();
        assert!(before.failures.is_empty());
        assert!(files.iter().all(|path| path.is_file()));
        let due = expire_when_idle(
            &root,
            AudioRetention::Hours24,
            completed + Duration::from_secs(86_400),
        )
        .await;
        assert!(due.failures.is_empty(), "{due:?}");
        assert_eq!(due.expired, 1);
        assert!(
            files.iter().all(|path| !path.exists()),
            "all hardlinks and copies must expire"
        );
        assert!(std::fs::symlink_metadata(root.join("last_session.wav")).is_err());
        assert_eq!(
            std::fs::read(root.join("transcriptions/2026-10-02/120000_probe_raw.txt")).unwrap(),
            b"preserve text"
        );
    }

    #[tokio::test]
    async fn current_off_never_erases_a_take_that_started_with_forever() {
        if isolated_child("current_off_never_erases_a_take_that_started_with_forever") {
            return;
        }
        let (root, lease, files) = owned_take(AudioRetention::Forever);
        let active = maintain(&root, AudioRetention::Off, SystemTime::now()).unwrap();
        assert_eq!(active.expired, 0);
        assert!(active.deferred > 0);
        finish_capture(&root, ID);
        drop(lease);
        let completed = completed_at(&root).await;
        let report = maintain(
            &root,
            AudioRetention::Off,
            completed + Duration::from_secs(365 * 86_400),
        )
        .unwrap();
        assert!(report.failures.is_empty());
        assert_eq!(report.expired, 0);
        assert!(files.iter().all(|path| path.is_file()));
    }

    #[tokio::test]
    async fn an_off_take_waits_for_path_based_readers_and_expires_after_the_last_reader() {
        if isolated_child(
            "an_off_take_waits_for_path_based_readers_and_expires_after_the_last_reader",
        ) {
            return;
        }
        let (root, lease, files) = owned_take(AudioRetention::Off);
        let reader = AudioReadLease::acquire(&root).unwrap();
        finish_capture(&root, ID);
        drop(lease);
        let completed = completed_at(&root).await;
        let deferred = maintain(&root, AudioRetention::Forever, completed).unwrap();
        assert_eq!(deferred.expired, 0);
        assert!(deferred.deferred > 0);
        tokio::task::yield_now().await;
        let pcm: Vec<i16> = hound::WavReader::open(&files[1])
            .unwrap()
            .into_samples()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            pcm,
            [i16::MIN, 0, 123, i16::MAX],
            "the path must remain reopenable throughout the reader lease"
        );
        drop(reader);
        let due = expire_when_idle(&root, AudioRetention::Forever, completed).await;
        assert!(due.failures.is_empty(), "{due:?}");
        assert_eq!(due.expired, 1);
        assert!(files.iter().all(|path| !path.exists()));
    }

    #[tokio::test]
    async fn a_substituted_symlink_never_deletes_foreign_audio_or_other_take_members() {
        if isolated_child("a_substituted_symlink_never_deletes_foreign_audio_or_other_take_members")
        {
            return;
        }
        let (root, lease, files) = owned_take(AudioRetention::Forever);
        finish_capture(&root, ID);
        drop(lease);
        let completed = completed_at(&root).await;
        let outside = tempfile::tempdir().unwrap();
        let foreign = outside.path().join("foreign.wav");
        std::fs::write(&foreign, b"foreign bytes").unwrap();
        std::fs::remove_file(&files[1]).unwrap();
        std::os::unix::fs::symlink(&foreign, &files[1]).unwrap();
        let report = maintain(
            &root,
            AudioRetention::Hours24,
            completed + Duration::from_secs(86_400),
        )
        .unwrap();
        assert!(
            !report.failures.is_empty(),
            "a substituted audio member must refuse the entire deletion"
        );
        assert_eq!(report.expired, 0);
        assert_eq!(std::fs::read(foreign).unwrap(), b"foreign bytes");
        assert!(files[0].is_file());
        assert!(files[2].is_file());
    }
}
