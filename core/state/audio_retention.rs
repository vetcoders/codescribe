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
        && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// Called before microphone admission, with the same immutable runtime snapshot
/// as the recorder. Failure refuses capture; it cannot silently opt out.
pub fn begin_capture(root: &Path, id: &str, policy: AudioRetention) -> Result<()> {
    anyhow::ensure!(valid_id(id), "unsafe audio capture id");
    let lease = AudioReadLease::acquire(root)?;
    let directory = lease.0.try_clone()?;
    let mut active = captures().lock().map_err(|_| anyhow::anyhow!("audio capture registry poisoned"))?;
    let key = (root.to_path_buf(), id.to_string());
    anyhow::ensure!(!active.contains_key(&key), "duplicate audio capture lease");
    active.insert(key, Arc::new(CaptureLease {
        root: root.to_path_buf(), directory, session_id: id.to_string(), policy,
        lease: Some(lease),
        files: Mutex::new(CaptureFiles { files: Vec::new(), retry_protected: false }),
    }));
    Ok(())
}

/// Clone before queuing archive I/O, so a terminal reset cannot outrun it.
pub fn capture(root: &Path, id: &str) -> Option<Arc<CaptureLease>> {
    captures().lock().ok()?.get(&(root.to_path_buf(), id.to_string())).cloned()
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
        let mut state = self.files.lock().map_err(|_| anyhow::anyhow!("audio receipt poisoned"))?;
        state.retry_protected |= retry_protected;
        for path in paths {
            let relative = path.strip_prefix(&self.root).context("external audio is not owned")?;
            let relative = relative.to_str().context("audio path is not UTF-8")?;
            anyhow::ensure!(admitted(relative, &self.session_id), "unadmitted audio path");
            let (directory, name) = file_parent(&self.directory, relative)?;
            let file = open_at(&directory, &name, libc::O_RDONLY | libc::O_NONBLOCK)?;
            let metadata = file.metadata()?;
            anyhow::ensure!(metadata.is_file(), "owned audio must be regular");
            let parent = directory.metadata()?;
            let owned = OwnedFile {
                relative: relative.to_string(), device: metadata.dev(), inode: metadata.ino(),
                length: metadata.len(), modified: metadata.mtime(), modified_ns: metadata.mtime_nsec(),
                parent_device: parent.dev(), parent_inode: parent.ino(),
            };
            if !state.files.iter().any(|file| file.relative == owned.relative) {
                state.files.push(owned);
            }
        }
        Ok(())
    }

    /// Failed capture/storage remains recovery evidence, including under Off.
    pub fn protect_retry(&self) {
        if let Ok(mut state) = self.files.lock() { state.retry_protected = true; }
    }
}

impl Drop for CaptureLease {
    fn drop(&mut self) {
        let Some(lease) = self.lease.take() else { return };
        let state = match self.files.get_mut() {
            Ok(state) => state,
            Err(_) => { error!("audio retention receipt poisoned; preserving audio"); return; }
        };
        if state.files.is_empty() { return; }
        let metadata = match self.directory.metadata() {
            Ok(metadata) => metadata,
            Err(error) => { error!(%error, "audio completion root unavailable; preserving audio"); return; }
        };
        let completed_at = SystemTime::now().duration_since(UNIX_EPOCH).map(|time| time.as_secs()).unwrap_or(0);
        let receipt = Receipt {
            schema: SCHEMA.to_string(), session_id: self.session_id.clone(), completed_at,
            capture_policy: self.policy, root_device: metadata.dev(), root_inode: metadata.ino(),
            files: std::mem::take(&mut state.files), retry_protected: state.retry_protected,
        };
        let directory = match self.directory.try_clone() {
            Ok(directory) => directory,
            Err(error) => { error!(%error, "audio completion descriptor unavailable"); return; }
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
        ["takes", name] => name.strip_prefix("codescribe_recording_")
            .and_then(|value| value.strip_suffix(".wav"))
            .is_some_and(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())),
        ["sessions", name] => *name == format!("{id}.wav"),
        ["transcriptions", date, name] => chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok()
            && name.len() > 7 && name.as_bytes()[..6].iter().all(u8::is_ascii_digit)
            && name.as_bytes()[6] == b'_' && (name.ends_with(".m4a") || name.ends_with(".wav")),
        _ => false,
    }
}

fn file_parent(root: &File, relative: &str) -> Result<(File, CString)> {
    let path = Path::new(relative);
    anyhow::ensure!(path.components().all(|part| matches!(part, Component::Normal(_))), "unsafe receipt path");
    let mut directory = root.try_clone()?;
    let mut parts = relative.split('/').peekable();
    while let Some(part) = parts.next() {
        let name = CString::new(part)?;
        if parts.peek().is_none() { return Ok((directory, name)); }
        directory = open_at(&directory, &name, libc::O_RDONLY | libc::O_DIRECTORY)?;
    }
    anyhow::bail!("empty receipt path")
}

fn publish_receipt(root: &File, receipt: &Receipt) -> Result<()> {
    let sessions = subdir(root, c"sessions")?;
    let temporary = CString::new(format!(".retention-{}.tmp", uuid::Uuid::new_v4()))?;
    let name = CString::new(format!("{}{SUFFIX}", receipt.session_id))?;
    let result = (|| -> Result<()> {
        let mut file = open_at(&sessions, &temporary, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL)?;
        file.write_all(&serde_json::to_vec(receipt)?)?;
        file.sync_all()?;
        // SAFETY: single owned names relative to the pinned sessions directory.
        if unsafe { libc::linkat(sessions.as_raw_fd(), temporary.as_ptr(), sessions.as_raw_fd(), name.as_ptr(), 0) } < 0 {
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
    fn drop(&mut self) { if !self.0.is_null() { unsafe { libc::closedir(self.0) }; } }
}
struct StoreCursor { device: u64, inode: u64, cursor: Cursor }
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
            return Ok(MaintenanceReport { deferred: 1, ..Default::default() });
        }
        return Err(error.into());
    }
    let sessions = match open_at(&directory, c"sessions", libc::O_RDONLY | libc::O_DIRECTORY) {
        Ok(sessions) => sessions,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(MaintenanceReport::default()),
        Err(error) => return Err(error.into()),
    };
    let metadata = sessions.metadata()?;
    let mut all = cursors().lock().map_err(|_| anyhow::anyhow!("audio cursor poisoned"))?;
    let entry = all.entry(root.to_path_buf()).or_insert_with(|| StoreCursor {
        device: 0, inode: 0, cursor: Cursor(std::ptr::null_mut()),
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
        entry.device = metadata.dev(); entry.inode = metadata.ino();
    }
    let mut report = MaintenanceReport::default();
    let now = now.duration_since(UNIX_EPOCH).map(|time| time.as_secs()).unwrap_or(0);
    for _ in 0..PASS_ENTRIES {
        // SAFETY: mutex exclusively owns the live stream; copy name before next read.
        let Some(name) = read_cursor(&entry.cursor)? else {
            // SAFETY: the mutex owns this live stream exclusively.
            unsafe { libc::rewinddir(entry.cursor.0) };
            break;
        };
        let Some(receipt_name) = name.to_str().ok() else { continue };
        let receipt_name = receipt_name.strip_prefix(".expiry-")
            .and_then(|name| name.strip_suffix(".tmp")).unwrap_or(receipt_name);
        let Some(id) = receipt_name.strip_suffix(SUFFIX) else { continue };
        if !valid_id(id) { continue; }
        report.examined += 1;
        match expire_receipt(&directory, &sessions, &name, id, policy, now) {
            Ok(true) => report.expired += 1,
            Ok(false) => report.deferred += 1,
            Err(error) => report.failures.push(format!("{}: {error:#}", name.to_string_lossy())),
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
                if *errno != 0 { return Err(std::io::Error::last_os_error().into()); }
                return Ok(None);
            }
            Ok(Some(CStr::from_ptr((*next).d_name.as_ptr()).to_owned()))
        }
    }
}

fn expire_receipt(root: &File, sessions: &File, name: &CStr, id: &str, policy: AudioRetention, now: u64) -> Result<bool> {
    let file = open_at(sessions, name, libc::O_RDONLY | libc::O_NONBLOCK)?;
    let metadata = file.metadata()?;
    anyhow::ensure!(metadata.is_file() && metadata.len() <= MAX_RECEIPT_BYTES, "invalid audio receipt");
    let mut bytes = Vec::new();
    file.take(MAX_RECEIPT_BYTES + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() as u64 <= MAX_RECEIPT_BYTES, "oversized audio receipt");
    let receipt: Receipt = serde_json::from_slice(&bytes)?;
    let root_metadata = root.metadata()?;
    anyhow::ensure!(receipt.schema == SCHEMA && receipt.session_id == id
        && receipt.root_device == root_metadata.dev() && receipt.root_inode == root_metadata.ino()
        && receipt.files.len() <= 16, "unadmitted audio receipt");
    if receipt.retry_protected || receipt.completed_at == 0 || now < receipt.completed_at { return Ok(false); }
    let due = receipt.capture_policy == AudioRetention::Off
        || policy.duration_seconds().is_some_and(|seconds| now - receipt.completed_at >= seconds);
    if !due { return Ok(false); }
    // Prevalidate every member before removing any. Missing members are expected
    // after a partially successful pass; a changed inode protects the entire take.
    for file in &receipt.files {
        anyhow::ensure!(admitted(&file.relative, id), "receipt includes unowned audio");
        verify_file(root, file)?;
    }
    remove_alias(root, id)?;
    for file in &receipt.files { remove_owned(root, file)?; }
    // Keep the receipt on any failure; retry reports the exact remaining members.
    let canonical_name = CString::new(format!("{id}{SUFFIX}"))?;
    remove_entry(sessions, &canonical_name, metadata.dev(), metadata.ino(), false)?;
    info!(session_id = id, "completed take audio expired; text history retained");
    Ok(true)
}

fn verify_file(root: &File, owned: &OwnedFile) -> Result<()> {
    let (directory, name) = file_parent(root, &owned.relative)?;
    let parent = directory.metadata()?;
    anyhow::ensure!(parent.dev() == owned.parent_device && parent.ino() == owned.parent_inode, "audio parent changed");
    let pending = quarantine_name(&name)?;
    for name in [name.as_c_str(), pending.as_c_str()] {
        match open_at(&directory, name, libc::O_RDONLY | libc::O_NONBLOCK) {
            Ok(file) => {
                let metadata = file.metadata()?;
                anyhow::ensure!(metadata.is_file() && metadata.dev() == owned.device && metadata.ino() == owned.inode
                    && metadata.len() == owned.length && metadata.mtime() == owned.modified
                    && metadata.mtime_nsec() == owned.modified_ns, "audio object changed; preserving retry evidence");
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
    if unsafe { libc::fstatat(directory.as_raw_fd(), name.as_ptr(), &mut metadata, libc::AT_SYMLINK_NOFOLLOW) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(metadata)
}

fn quarantine_name(name: &CStr) -> Result<CString> {
    Ok(CString::new(format!(".expiry-{}.tmp", name.to_str()?))?)
}

fn matching_stat(metadata: &libc::stat, device: u64, inode: u64, kind: libc::mode_t) -> bool {
    metadata.st_dev as u64 == device && metadata.st_ino as u64 == inode
        && metadata.st_mode & libc::S_IFMT == kind
}

fn unlink_matching(directory: &File, name: &CStr, device: u64, inode: u64, kind: libc::mode_t) -> Result<()> {
    let metadata = stat_entry(directory, name)?;
    anyhow::ensure!(matching_stat(&metadata, device, inode, kind), "expiration entry changed; preserving evidence");
    // SAFETY: one admitted leaf in the pinned parent; unlink never follows it.
    if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } < 0 {
        let error = std::io::Error::last_os_error();
        anyhow::bail!("audio deletion pending at {}: {error}", name.to_string_lossy());
    }
    Ok(())
}

fn remove_entry(directory: &File, name: &CStr, device: u64, inode: u64, symlink: bool) -> Result<()> {
    let kind = if symlink { libc::S_IFLNK } else { libc::S_IFREG };
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
    anyhow::ensure!(matching_stat(&metadata, device, inode, kind), "expiration entry changed");
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
        libc::renameatx_np(directory.as_raw_fd(), source.as_ptr(), directory.as_raw_fd(), destination.as_ptr(), libc::RENAME_EXCL)
    };
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::renameat2(directory.as_raw_fd(), source.as_ptr(), directory.as_raw_fd(), destination.as_ptr(), libc::RENAME_NOREPLACE)
    };
    #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
    anyhow::bail!("audio expiration requires an exclusive rename contract");
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    {
        if result < 0 { return Err(std::io::Error::last_os_error().into()); }
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
        if metadata.st_mode & libc::S_IFMT != libc::S_IFLNK { continue; }
        let mut target = [0u8; 256];
        // SAFETY: read one symlink leaf in the pinned root, without following it.
        let length = unsafe { libc::readlinkat(root.as_raw_fd(), name.as_ptr(), target.as_mut_ptr().cast(), target.len()) };
        if length < 0 { return Err(std::io::Error::last_os_error().into()); }
        if target.get(..length as usize) != Some(format!("sessions/{id}.wav").as_bytes()) { continue; }
        if name == pending.as_c_str() {
            unlink_matching(root, name, metadata.st_dev as u64, metadata.st_ino as u64, libc::S_IFLNK)?;
        } else {
            remove_entry(root, name, metadata.st_dev as u64, metadata.st_ino as u64, true)?;
        }
    }
    Ok(())
}

fn run_maintenance(root: &Path) {
    let result = (|| -> Result<MaintenanceReport> {
        let snapshot = Config::load_runtime_snapshot_without_keychain()?;
        anyhow::ensure!(snapshot.repair_receipt().unrepairable.is_empty(), "settings snapshot refused; preserving audio");
        maintain(root, snapshot.values().audio_retention, SystemTime::now())
    })();
    match result {
        Ok(report) if !report.failures.is_empty() => error!(failures = ?report.failures, "audio retention remains pending"),
        Ok(report) => info!(examined = report.examined, expired = report.expired, deferred = report.deferred, "audio retention maintenance"),
        Err(error) => warn!(%error, "audio retention maintenance refused; preserving audio"),
    }
}

/// Startup and periodic bounded maintenance on the existing history owner.
/// Settings are read only through the canonical snapshot loader on the worker.
pub fn start_maintenance() {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) { return; }
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
